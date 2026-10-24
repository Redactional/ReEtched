"""Generates tests/data/reference.json with PyOpenColorIO as the oracle.

    python -m venv <scratch>/venv && <scratch>/venv/Scripts/pip install opencolorio
    <scratch>/venv/Scripts/python crates/reetched-color/tests/gen_reference.py

Evaluates fixed RGB inputs through color space, display/view (with and without a look) and
BuiltinTransform processors for every vendored config plus a synthetic config that exercises
the remaining transform types and LUT formats. Processors use OPTIMIZATION_NONE so OCIO does
not fold or approximate ops.
"""

import json
import math
import os
import re
import tempfile

import PyOpenColorIO as ocio

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "..", "data")
OUT = os.path.join(HERE, "data", "reference.json")

VENDORED = [
    "cg-config-v2.2.0_aces-v1.3_ocio-v2.4",
    "cg-config-v4.0.0_aces-v2.0_ocio-v2.5",
    "cg-config-v5.0.0_aces-v2.1_ocio-v2.6",
]
LOOK = "ACES 1.3 Reference Gamut Compression"

HDR = [
    [0, 0, 0], [0.18, 0.18, 0.18], [0.001, 0.002, 0.003], [0.5, 0.4, 0.3], [1, 1, 1],
    [2, 1.5, 1], [10, 8, 6], [100, 90, 80], [50, 5, 0.5], [1, 0, 0], [0, 1, 0], [0, 0, 1],
    [0.8, 0.1, 0.05], [0.05, 0.6, 0.9], [-0.1, 0.2, 0.5], [0.3, -0.05, 0.1], [4, 0.2, 0.1],
    [0.7, 0.7, 0.05],
]
UNIT = [
    [0, 0, 0], [0.18, 0.18, 0.18], [0.01, 0.02, 0.03], [0.5, 0.4, 0.3], [1, 1, 1],
    [0.9, 0.1, 0.05], [0.05, 0.8, 0.2], [0.1, 0.2, 0.95], [-0.05, 0.3, 0.6], [0.7, 0.7, 0.05],
    [0.25, 0.5, 0.75], [1.2, 0.9, 0.4],
]


def clean(v):
    # f32 results: 7 significant digits are far below the test tolerances.
    return float(f"{v:.7g}") if math.isfinite(v) else None


def run(proc, inputs):
    cpu = proc.getOptimizedCPUProcessor(ocio.OPTIMIZATION_NONE)
    return [[clean(v) for v in cpu.applyRGB([float(x) for x in rgb])] for rgb in inputs]


def cs_cases(cfg, pairs):
    out = []
    for src, dst, inputs in pairs:
        out.append({"src": src, "dst": dst, "inputs": inputs,
                    "out": run(cfg.getProcessor(src, dst), INPUTS[inputs])})
    return out


INPUTS = {"hdr": HDR, "unit": UNIT}


def display_case(cfg, src, display, view, looks, inputs):
    dvt = ocio.DisplayViewTransform(src=src, display=display, view=view)
    if looks is None:
        proc = cfg.getProcessor(dvt)
    else:
        pipe = ocio.LegacyViewingPipeline()
        pipe.setDisplayViewTransform(dvt)
        pipe.setLooksOverrideEnabled(True)
        pipe.setLooksOverride(looks)
        proc = pipe.getProcessor(cfg)
    return {"src": src, "display": display, "view": view, "looks": looks, "inputs": inputs,
            "out": run(proc, INPUTS[inputs])}


def config_cases(cfg):
    working = "ACEScg"
    names = list(cfg.getColorSpaceNames(ocio.SEARCH_REFERENCE_SPACE_ALL, ocio.COLORSPACE_ACTIVE))
    pairs = []
    for n in names:
        pairs.append((working, n, "hdr"))
        pairs.append((n, working, "unit"))
    for n in cfg.getNamedTransformNames():
        pairs.append((n, working, "unit"))
        pairs.append((working, n, "unit"))
    displays = []
    for d in cfg.getDisplays():
        for v in cfg.getViews(d):
            displays.append(display_case(cfg, working, d, v, None, "hdr"))
    default = cfg.getDefaultDisplay()
    for d in {default, "sRGB - Display", "Rec.1886 Rec.709 - Display"}:
        for v in cfg.getViews(d):
            displays.append(display_case(cfg, working, d, v, LOOK, "hdr"))
    displays.append(display_case(cfg, "sRGB Encoded Rec.709 (sRGB)", default,
                                 cfg.getDefaultView(default), None, "unit"))
    return {
        "default_display": default,
        "views": {d: list(cfg.getViews(d)) for d in cfg.getDisplays()},
        "color_spaces": names,
        "cs": cs_cases(cfg, pairs),
        "display": displays,
    }


def builtin_cases():
    styles = set()
    for name in ocio.BuiltinConfigRegistry():
        text = ocio.Config.CreateFromBuiltinConfig(name).serialize()
        styles.update(re.findall(r"!<BuiltinTransform> \{style: ([^,}]+)", text))
    raw = ocio.Config.CreateRaw()
    out = []
    for style in sorted(styles):
        inputs = "hdr" if style.startswith("ACES-OUTPUT") else "unit"
        for inverse in (False, True):
            bt = ocio.BuiltinTransform(style)
            if inverse:
                bt.setDirection(ocio.TRANSFORM_DIR_INVERSE)
            ins = "unit" if inverse else inputs
            out.append({"style": style, "inverse": inverse, "inputs": ins,
                        "out": run(raw.getProcessor(bt), INPUTS[ins])})
    return out


# ---- synthetic config: transform types and LUT formats the ACES configs don't use ----------

def lut_files():
    files = {}

    def cube1d():
        rows = []
        for i in range(17):
            t = i / 16
            rows.append(f"{0.1 + 0.8 * t ** 0.7:.6f} {t ** 1.8:.6f} {1 - 0.5 * t:.6f}")
        return "TITLE \"1d\"\nLUT_1D_SIZE 17\nDOMAIN_MIN -0.1 -0.1 -0.1\nDOMAIN_MAX 1.5 1.5 1.5\n" + "\n".join(rows) + "\n"

    def f3(r, g, b):
        return (r ** 0.9 * 0.9 + 0.05 * g, g * 0.95 + 0.03 * b * b, b * 0.8 + 0.1 * r)

    def grid(n, order="rgb"):
        rows = []
        for b in range(n):
            for g in range(n):
                for r in range(n):
                    v = f3(r / (n - 1), g / (n - 1), b / (n - 1))
                    rows.append((r, g, b, v))
        return rows

    def cube3d():
        rows = [f"{v[0]:.6f} {v[1]:.6f} {v[2]:.6f}" for _, _, _, v in grid(9)]
        return "LUT_3D_SIZE 9\n" + "\n".join(rows) + "\n"

    def resolve():
        shaper = [f"{(i / 32) ** 0.5:.6f} {(i / 32) ** 0.5:.6f} {(i / 32) ** 0.5:.6f}" for i in range(33)]
        rows = [f"{v[0]:.6f} {v[1]:.6f} {v[2]:.6f}" for _, _, _, v in grid(5)]
        return ("LUT_1D_SIZE 33\nLUT_1D_INPUT_RANGE 0 2\nLUT_3D_SIZE 5\n"
                + "\n".join(shaper) + "\n" + "\n".join(rows) + "\n")

    def spi1d():
        rows = [f"{(i / 31) ** 2.2:.6f} {(i / 31) ** 0.45:.6f} {i / 31 * 0.8:.6f}" for i in range(32)]
        return "Version 1\nFrom 0 1\nLength 32\nComponents 3\n{\n" + "\n".join(rows) + "\n}\n"

    def spi3d():
        rows = [f"{r} {g} {b} {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}" for r, g, b, v in grid(5)]
        return "SPILUT 1.0\n3 3\n5 5 5\n" + "\n".join(rows) + "\n"

    def csp():
        pre = "3\n0 0.5 1\n0 0.7 1\n2\n0 1\n0 1\n4\n0 0.2 0.6 1\n0 0.1 0.5 1\n"
        rows = [f"{v[0]:.6f} {v[1]:.6f} {v[2]:.6f}" for _, _, _, v in grid(5)]
        return "CSPLUTV100\n3D\n\n" + pre + "\n5 5 5\n" + "\n".join(rows) + "\n"

    def clf():
        n = 5
        vals = []
        for r in range(n):
            for g in range(n):
                for b in range(n):
                    vals.append(" ".join(f"{x:.6f}" for x in f3(r / (n - 1), g / (n - 1), b / (n - 1))))
        lut1d = " ".join(str(round(1023 * (i / 15) ** 0.8)) for i in range(16))
        return f"""<?xml version="1.0" encoding="UTF-8"?>
<ProcessList id="synthetic" compCLFversion="3">
  <Matrix inBitDepth="32f" outBitDepth="32f">
    <Array dim="3 4">0.9 0.1 0 0.01  0.05 0.9 0.05 0  0 0.1 0.9 -0.01</Array>
  </Matrix>
  <Range inBitDepth="32f" outBitDepth="32f">
    <minInValue>0</minInValue><maxInValue>1.2</maxInValue><minOutValue>0</minOutValue><maxOutValue>1</maxOutValue>
  </Range>
  <LUT1D inBitDepth="32f" outBitDepth="10i"><Array dim="16 1">{lut1d}</Array></LUT1D>
  <Range inBitDepth="10i" outBitDepth="32f">
    <minInValue>0</minInValue><maxInValue>1023</maxInValue><minOutValue>0</minOutValue><maxOutValue>1</maxOutValue>
  </Range>
  <Exponent inBitDepth="32f" outBitDepth="32f" style="monCurveRev"><ExponentParams exponent="2.4" offset="0.055"/></Exponent>
  <Log inBitDepth="32f" outBitDepth="32f" style="cameraLinToLog"><LogParams base="10" logSideSlope="0.25" logSideOffset="0.5" linSideSlope="5" linSideOffset="0.05" linSideBreak="0.01"/></Log>
  <ASC_CDL inBitDepth="32f" outBitDepth="32f" style="Fwd"><SOPNode><Slope>1.1 0.9 1</Slope><Offset>0.01 0 -0.01</Offset><Power>1.1 1 0.9</Power></SOPNode><SatNode><Saturation>0.9</Saturation></SatNode></ASC_CDL>
  <LUT3D inBitDepth="32f" outBitDepth="32f" interpolation="tetrahedral"><Array dim="{n} {n} {n} 3">{" ".join(vals)}</Array></LUT3D>
</ProcessList>
"""

    files["luts/curve.cube"] = cube1d()
    files["luts/grid.cube"] = cube3d()
    files["luts/resolve.cube"] = resolve()
    files["luts/curve.spi1d"] = spi1d()
    files["luts/grid.spi3d"] = spi3d()
    files["luts/grid.csp"] = csp()
    files["luts/chain.clf"] = clf()
    return files


SYNTHETIC = """ocio_profile_version: 2.4
search_path: luts
roles:
  scene_linear: ref
  data: raw
file_rules:
  - !<Rule> {name: Default, colorspace: ref}
displays:
  sRGB:
    - !<View> {name: Raw, colorspace: raw}
    - !<View> {name: Curve, colorspace: exp_mirror}
colorspaces:
  - !<ColorSpace> {name: ref}
  - !<ColorSpace> {name: raw, isdata: true}
  - !<ColorSpace>
    name: mtx
    to_scene_reference: !<MatrixTransform> {matrix: [0.9, 0.1, 0, 0, 0.05, 0.9, 0.05, 0, 0, 0.1, 0.9, 0, 0, 0, 0, 1], offset: [0.01, -0.02, 0.03, 0]}
  - !<ColorSpace>
    name: exp_basic
    to_scene_reference: !<ExponentTransform> {value: [2.2, 2.4, 2.6, 1]}
  - !<ColorSpace>
    name: exp_mirror
    to_scene_reference: !<ExponentTransform> {value: [2.2, 2.4, 2.6, 1], style: mirror}
  - !<ColorSpace>
    name: exp_passthru
    to_scene_reference: !<ExponentTransform> {value: 1.8, style: pass_thru}
  - !<ColorSpace>
    name: ewl_linear
    to_scene_reference: !<ExponentWithLinearTransform> {gamma: 2.4, offset: 0.055}
  - !<ColorSpace>
    name: ewl_mirror
    to_scene_reference: !<ExponentWithLinearTransform> {gamma: [2.2, 2.4, 2.6, 1], offset: [0.099, 0.055, 0.03, 0], style: mirror}
  - !<ColorSpace>
    name: log2
    to_scene_reference: !<LogTransform> {base: 2, direction: inverse}
  - !<ColorSpace>
    name: log_affine
    to_scene_reference: !<LogAffineTransform> {base: 10, log_side_slope: 0.3, log_side_offset: 0.6, lin_side_slope: 2, lin_side_offset: 0.1, direction: inverse}
  - !<ColorSpace>
    name: log_camera
    to_scene_reference: !<LogCameraTransform> {base: 2, log_side_slope: [0.07, 0.08, 0.09], log_side_offset: 0.5, lin_side_slope: 1, lin_side_offset: 0.0075, lin_side_break: 0.003, linear_slope: 10.4, direction: inverse}
  - !<ColorSpace>
    name: range_clamp
    to_scene_reference: !<RangeTransform> {min_in_value: 0.1, max_in_value: 0.9, min_out_value: 0, max_out_value: 1.5}
  - !<ColorSpace>
    name: range_noclamp
    to_scene_reference: !<RangeTransform> {min_in_value: 0.1, max_in_value: 0.9, min_out_value: 0, max_out_value: 1.5, style: noClamp}
  - !<ColorSpace>
    name: range_min
    to_scene_reference: !<RangeTransform> {min_in_value: 0.05, min_out_value: 0.05}
  - !<ColorSpace>
    name: cdl
    to_scene_reference: !<CDLTransform> {slope: [1.1, 0.9, 1], offset: [0.01, -0.02, 0], power: [1.2, 0.8, 1], sat: 0.8}
  - !<ColorSpace>
    name: cdl_noclamp
    to_scene_reference: !<CDLTransform> {slope: [1.1, 0.9, 1], offset: [0.01, -0.02, 0], power: [1.2, 0.8, 1], sat: 1.2, style: noClamp}
  - !<ColorSpace>
    name: ec_linear
    to_scene_reference: !<ExposureContrastTransform> {style: linear, exposure: 0.5, contrast: 1.2, gamma: 1.1, pivot: 0.18}
  - !<ColorSpace>
    name: ec_video
    to_scene_reference: !<ExposureContrastTransform> {style: video, exposure: -0.5, contrast: 0.9, pivot: 0.18}
  - !<ColorSpace>
    name: ec_log
    to_scene_reference: !<ExposureContrastTransform> {style: log, exposure: 0.25, contrast: 1.3, pivot: 0.18, log_exposure_step: 0.088, log_midway_gray: 0.435}
  - !<ColorSpace>
    name: ff_hsv
    to_scene_reference: !<FixedFunctionTransform> {style: RGB_TO_HSV}
  - !<ColorSpace>
    name: ff_redmod03
    to_scene_reference: !<FixedFunctionTransform> {style: ACES_RedMod03}
  - !<ColorSpace>
    name: ff_glow03
    to_scene_reference: !<FixedFunctionTransform> {style: ACES_Glow03}
  - !<ColorSpace>
    name: ff_darktodim
    to_scene_reference: !<FixedFunctionTransform> {style: ACES_DarkToDim10}
  - !<ColorSpace>
    name: alloc_lg2
    to_scene_reference: !<AllocationTransform> {allocation: lg2, vars: [-8, 5, 0.00390625], direction: inverse}
  - !<ColorSpace>
    name: alloc_uniform
    to_scene_reference: !<AllocationTransform> {allocation: uniform, vars: [-0.5, 2], direction: inverse}
  - !<ColorSpace>
    name: group
    to_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {src: mtx, dst: ref}
        - !<GroupTransform>
          direction: inverse
          children:
            - !<ExponentTransform> {value: 2.2, style: mirror}
            - !<MatrixTransform> {matrix: [1.2, 0, 0, 0, 0, 0.8, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]}
  - !<ColorSpace>
    name: file_cube1d
    to_scene_reference: !<FileTransform> {src: curve.cube}
  - !<ColorSpace>
    name: file_cube3d_linear
    to_scene_reference: !<FileTransform> {src: grid.cube, interpolation: linear}
  - !<ColorSpace>
    name: file_cube3d_tetra
    to_scene_reference: !<FileTransform> {src: grid.cube, interpolation: tetrahedral}
  - !<ColorSpace>
    name: file_resolve
    to_scene_reference: !<FileTransform> {src: resolve.cube}
  - !<ColorSpace>
    name: file_spi1d
    to_scene_reference: !<FileTransform> {src: curve.spi1d}
  - !<ColorSpace>
    name: file_spi3d
    to_scene_reference: !<FileTransform> {src: grid.spi3d}
  - !<ColorSpace>
    name: file_csp
    to_scene_reference: !<FileTransform> {src: grid.csp}
  - !<ColorSpace>
    name: file_clf
    to_scene_reference: !<FileTransform> {src: chain.clf}
"""

# Transforms OCIO can invert exactly (3D LUTs are only checked forward).
INVERTIBLE = {"mtx", "exp_basic", "exp_mirror", "exp_passthru", "ewl_linear", "ewl_mirror", "log2",
              "log_affine", "log_camera", "range_clamp", "range_noclamp", "range_min", "cdl",
              "cdl_noclamp", "ec_linear", "ec_video", "ec_log", "ff_hsv", "ff_redmod03", "ff_glow03",
              "ff_darktodim", "alloc_lg2", "alloc_uniform", "group", "file_cube1d", "file_spi1d"}


def synthetic_cases():
    files = lut_files()
    tmp = tempfile.mkdtemp()
    for path, text in files.items():
        os.makedirs(os.path.join(tmp, os.path.dirname(path)), exist_ok=True)
        with open(os.path.join(tmp, path), "w", newline="\n") as f:
            f.write(text)
    cfg = ocio.Config.CreateFromStream(SYNTHETIC)
    # Camera input spaces of the ACES studio config: LogCameraTransforms and camera builtins.
    studio = ocio.Config.CreateFromBuiltinConfig("studio-config-v4.0.0_aces-v2.0_ocio-v2.5")
    cameras = []
    for cs in studio.getColorSpaces():
        if cs.getFamily().startswith("Input/") and cs.getTransform(ocio.COLORSPACE_DIR_TO_REFERENCE):
            copy = ocio.ColorSpace(name=cs.getName())
            copy.setTransform(cs.getTransform(ocio.COLORSPACE_DIR_TO_REFERENCE), ocio.COLORSPACE_DIR_TO_REFERENCE)
            cfg.addColorSpace(copy)
            cameras.append(cs.getName())
    yaml = cfg.serialize()
    cfg = ocio.Config.CreateFromStream(yaml)
    cfg.setWorkingDir(tmp)
    pairs = []
    for n in cfg.getColorSpaceNames():
        if n in ("ref", "raw"):
            continue
        pairs.append((n, "ref", "unit"))
        if n in INVERTIBLE or n in cameras:
            pairs.append(("ref", n, "unit"))
    pairs.append(("raw", "mtx", "unit"))
    return {"yaml": yaml, "files": files, "cs": cs_cases(cfg, pairs),
            "display": [display_case(cfg, "ref", "sRGB", v, None, "unit") for v in ("Raw", "Curve")]}


def main():
    configs = {}
    for name in VENDORED:
        with open(os.path.join(DATA, name + ".ocio")) as f:
            vendored = f.read()
        assert vendored == ocio.Config.CreateFromBuiltinConfig(name).serialize(), name
        configs[name] = config_cases(ocio.Config.CreateFromStream(vendored))
    ref = {
        "ocio_version": ocio.__version__,
        "inputs": INPUTS,
        "configs": configs,
        "synthetic": synthetic_cases(),
        "builtins": builtin_cases(),
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", newline="\n") as f:
        json.dump(ref, f, separators=(",", ":"))
    print("wrote", OUT, os.path.getsize(OUT), "bytes")


if __name__ == "__main__":
    main()
