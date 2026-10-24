# ReEtched Bridge (Substance 3D Painter plugin)

Exports the project open in Substance 3D Painter so ReEtched can import it
with its layer stack intact.

Needs Substance 3D Painter **10.0 or later**. Open older projects in 10+ first.

## Install

1. Copy `reetched_bridge.py` into Painter's Python plugin folder:
   - Windows: `Documents\Adobe\Adobe Substance 3D Painter\python\plugins`
   - macOS: `~/Documents/Adobe/Adobe Substance 3D Painter/python/plugins`
   - Linux: `~/Documents/Adobe/Adobe Substance 3D Painter/python/plugins`
2. In Painter: **Python > Reload plugins folder**, then enable **reetched_bridge**.

## Use

1. In Painter: **File > Export to ReEtched...** and pick a folder. The plugin
   writes `<project>_reetched/` there. It takes a while: each paint layer, mask
   and non-uniform fill is exported twice. Your project is put back as it was.
2. In ReEtched: **File > Import Painter bridge...** and pick that folder (or
   drop it onto the window).

ReEtched then re-composites the project and compares every channel with
Painter's own export; the log says which channels match.

## What comes over

| Painter | ReEtched |
|---|---|
| Layer stack structure, names, visibility | Exact |
| Per-channel blend modes and opacities | Exact |
| Fill layers with uniform colors | Exact, editable fills |
| Paint layers, bitmap/Substance fills, effects, instances, anchors | Pixel captures in editable paint layers (strokes can't be exported by Painter's API) |
| Masks with effects or paint | Pixel captures in editable painted masks |
| Substance parameters | Recorded in the bundle (not used yet) |
| Baked mesh maps | Imported |
| Base color, metallic, roughness, normal, height | Imported; other channels are listed in the log |

Not supported yet: UDIM texture sets, more than one material layer per
texture set (only the first is exported).

## Status

The plugin is written against Painter's documented Python API but **has not
been run in Painter yet**. `tests/test_bridge.py` runs it against a fake API
(`tests/fake`) and checks the bundle and that the project is restored:

```
python tools/painter-bridge/tests/test_bridge.py
```

With `--fixture crates/reetched-substance/tests/data/bridge` it also rewrites
the bundle ReEtched's importer tests use.
