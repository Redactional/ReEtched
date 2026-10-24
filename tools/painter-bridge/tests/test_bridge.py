"""Runs the ReEtched Bridge plugin against a fake Painter API.

    python tools/painter-bridge/tests/test_bridge.py
    python tools/painter-bridge/tests/test_bridge.py --fixture crates/reetched-substance/tests/data/bridge

The second form also (re)writes the bundle the Rust importer test reads.
"""

import json
import os
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "fake"))
sys.path.insert(0, os.path.dirname(HERE))

import substance_painter._core as fake  # noqa: E402
import reetched_bridge  # noqa: E402

CT = fake.ChannelType
N = 4  # texture size


def build_scene(mesh_path):
    ts = fake.TextureSet("Body", N, list(CT))
    chans = list(CT)

    scratches = fake.Node(fake.NodeType.PaintLayer, "Scratches", chans)
    scratches.blend[CT.BaseColor] = [fake.BlendingMode.Multiply, 0.8]
    scratches.content[CT.BaseColor] = [
        ((0.2, 0.4, 0.6), 1.0) if p % N < 2 else ((0.0, 0.0, 0.0), 0.0) for p in range(N * N)
    ]
    scratches.content[CT.Roughness] = [((0.3, 0.3, 0.3), 0.5)] * (N * N)
    scratches.mask = {
        "background": fake.MaskBackground.Black,
        "enabled": True,
        "paint": [1.0 if p < N else 0.5 for p in range(N * N)],
    }

    base = fake.Node(fake.NodeType.GroupLayer, "Base", chans)
    base.blend[CT.Roughness] = [fake.BlendingMode.Normal, 0.9]
    steel = fake.Node(fake.NodeType.FillLayer, "Steel", chans)
    steel.active_channels = {CT.BaseColor, CT.Metallic, CT.Roughness}
    steel.set_source(CT.BaseColor, fake.Color(0.8, 0.8, 0.8))
    steel.set_source(CT.Metallic, fake.Color(1.0, 1.0, 1.0))
    steel.set_source(CT.Roughness, fake.Color(0.25, 0.25, 0.25))
    base.children = [steel]

    rust = fake.Node(fake.NodeType.FillLayer, "Rust", chans)
    rust.visible = False
    rust.active_channels = {CT.BaseColor}
    rust.set_source(
        CT.BaseColor,
        fake.SourceSubstance(
            "resource://starter_assets/rust", {"age": 0.7}, {CT.BaseColor: [(0.5, 0.25, 0.1)] * (N * N)}
        ),
    )

    ts.stack.roots = [scratches, base, rust]
    ts.mesh_maps["ambient_occlusion"] = [(0.9, 0.9, 0.9)] * (N * N)
    fake.scene.texture_sets = [ts]
    fake.scene.mesh_path = mesh_path
    return ts


def snapshot(nodes):
    """Everything the plugin may touch, to check it was put back."""
    out = []
    for n in nodes:
        out.append(
            (
                n.uid(),
                n.visible,
                sorted((ct.name, m.name, o) for ct, (m, o) in n.blend.items()),
                dict(n.mask, paint=None) if n.mask else None,
                len(n.effects),
                snapshot(n.children),
            )
        )
    return out


def main():
    fixture = sys.argv[2] if len(sys.argv) > 2 and sys.argv[1] == "--fixture" else None
    work = tempfile.mkdtemp()
    mesh = os.path.join(work, "quad.obj")
    with open(mesh, "w") as f:
        f.write(
            "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\n"
            "vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\n"
            "usemtl Body\nf 1/1/1 2/2/1 3/3/1 4/4/1\n"
        )
    ts = build_scene(mesh)
    before = snapshot(ts.stack.roots)

    out = fixture or os.path.join(work, "bundle")
    if fixture and os.path.exists(fixture):
        shutil.rmtree(fixture)
    bundle = reetched_bridge.Bridge(out).run()

    assert snapshot(ts.stack.roots) == before, "the project is restored"
    with open(os.path.join(out, "bridge.json"), encoding="utf-8") as f:
        assert json.load(f) == json.loads(json.dumps(bundle))

    assert bundle["mesh"] == "mesh.obj"
    [entry] = bundle["texture_sets"]
    assert entry["resolution"] == [N, N]
    assert set(entry["mesh_maps"]) == {"ambient_occlusion"}, entry["mesh_maps"]
    assert set(entry["reference"]) == {"basecolor", "metallic", "roughness", "normal", "height", "emissive", "opacity"}
    scratches, base, rust = entry["layers"]
    assert scratches["type"] == "paint" and set(scratches["content"]) == {"black", "white"}
    assert scratches["blend"]["basecolor"] == {"mode": "Multiply", "opacity": 0.8}
    assert scratches["mask"]["background"] == "black" and "capture" in scratches["mask"]
    assert base["type"] == "group" and base["blend"]["roughness"]["opacity"] == 0.9
    [steel] = base["children"]
    assert steel["fill"] == {
        "basecolor": [0.8, 0.8, 0.8],
        "metallic": [1.0, 1.0, 1.0],
        "roughness": [0.25, 0.25, 0.25],
    }
    assert "content" not in steel
    assert rust["visible"] is False and "content" in rust
    assert rust["substance"] == {"resource://starter_assets/rust": {"age": 0.7}}
    assert any("Specular" in line for line in bundle["log"]), bundle["log"]

    shutil.rmtree(work)
    print("ok: %d exports" % fake.scene.exports)


if __name__ == "__main__":
    main()
