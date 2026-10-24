"""Regenerates the USD test fixtures with Pixar's own library.

    python -m venv venv && venv/bin/pip install usd-core
    venv/bin/python make_fixtures.py

Writes scene.usda, scene.usdc and scene.usdz (holding scene.usdc) next to this
script. The Rust tests in tests/usd.rs hard-code what this scene contains, so
change both together.
"""

import os
from pxr import Gf, Sdf, Usd, UsdGeom, UsdShade, UsdUtils, Vt

HERE = os.path.dirname(os.path.abspath(__file__))


def build(path):
    stage = Usd.Stage.CreateNew(path)
    UsdGeom.SetStageUpAxis(stage, UsdGeom.Tokens.z)
    UsdGeom.SetStageMetersPerUnit(stage, 0.01)  # ignored by the importer

    world = UsdGeom.Xform.Define(stage, "/World")
    world.AddTranslateOp().Set(Gf.Vec3d(10, 0, 0))
    stage.SetDefaultPrim(world.GetPrim())

    looks = {name: UsdShade.Material.Define(stage, f"/World/Looks/{name}") for name in ["MatA", "MatB"]}

    # Box: a quad and a pentagon, under a uniform scale of 2.
    a = UsdGeom.Xform.Define(stage, "/World/A")
    a.AddScaleOp().Set(Gf.Vec3f(2, 2, 2))
    box = UsdGeom.Mesh.Define(stage, "/World/A/Box")
    pts = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0),
           (2, 0, 0), (3, 0, 0), (3.5, 1, 0), (2.5, 2, 0), (1.5, 1, 0)]
    fvi = [0, 1, 2, 3, 4, 5, 6, 7, 8]
    box.CreatePointsAttr(pts)
    box.CreateFaceVertexCountsAttr([4, 5])
    box.CreateFaceVertexIndicesAttr(fvi)
    pv = UsdGeom.PrimvarsAPI(box)
    # Indexed faceVarying normals: every corner points at value 1 (+Z).
    n = pv.CreatePrimvar("normals", Sdf.ValueTypeNames.Normal3fArray, UsdGeom.Tokens.faceVarying)
    n.Set([(0, 0, -1), (0, 0, 1)])
    n.SetIndices(Vt.IntArray([1] * len(fvi)))
    # Indexed faceVarying st: corner uv = (x / 4, y / 2) of its point.
    st = pv.CreatePrimvar("st", Sdf.ValueTypeNames.TexCoord2fArray, UsdGeom.Tokens.faceVarying)
    st.Set([(x / 4, y / 2) for x, y, _ in pts])
    st.SetIndices(Vt.IntArray(fvi))
    # Second set, per vertex: (y / 2, x / 4).
    st1 = pv.CreatePrimvar("st1", Sdf.ValueTypeNames.TexCoord2fArray, UsdGeom.Tokens.vertex)
    st1.Set([(y / 2, x / 4) for x, y, _ in pts])
    for name, faces, look in [("partA", [0], "MatA"), ("partB", [1], "MatB")]:
        sub = UsdGeom.Subset.Define(stage, f"/World/A/Box/{name}")
        sub.CreateElementTypeAttr("face")
        sub.CreateFamilyNameAttr("materialBind")
        sub.CreateIndicesAttr(faces)
        UsdShade.MaterialBindingAPI.Apply(sub.GetPrim()).Bind(looks[look])

    # Tri: left-handed, no normals or UVs, bound as a whole to MatB.
    b = UsdGeom.Xform.Define(stage, "/World/B")
    b.AddTranslateOp().Set(Gf.Vec3d(0, 0, 5))
    b.AddRotateZOp().Set(90)
    tri = UsdGeom.Mesh.Define(stage, "/World/B/Tri")
    tri.CreatePointsAttr([(0, 0, 0), (0, 1, 0), (1, 0, 0)])
    tri.CreateFaceVertexCountsAttr([3])
    tri.CreateFaceVertexIndicesAttr([0, 1, 2])
    tri.CreateOrientationAttr(UsdGeom.Tokens.leftHanded)
    UsdShade.MaterialBindingAPI.Apply(tri.GetPrim()).Bind(looks["MatB"])
    return stage


build(os.path.join(HERE, "scene.usda")).Save()
build(os.path.join(HERE, "scene.usdc")).Save()
usdz = os.path.join(HERE, "scene.usdz")
if os.path.exists(usdz):
    os.remove(usdz)
assert UsdUtils.CreateNewUsdzPackage(Sdf.AssetPath(os.path.join(HERE, "scene.usdc")), usdz)
