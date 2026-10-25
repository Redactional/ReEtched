# Coming from Substance 3D Painter

ReEtched is laid out like Painter, so most of what you know carries over. This page covers what's different.

## Your projects

- **File > Import Painter project (.spp)** (or drag the file onto the window) converts a Painter project into a ReEtched project. The `.spp` is only read, never changed. Save the result as a `.retx`.
- Each texture set gets a top **Painter result** layer with Painter's own final channels, so the project looks right straight away. Hide it to work on the rebuilt layers below it.
- The rebuilt layers keep the layer stack, folders, fills, blending, opacity and masks. Paint is replayed from Painter's stored brush strokes, so it's close but not pixel-exact. Procedural (`.sbsar`) fills, most filters, smudge strokes and text aren't rebuilt yet. The import log lists everything that was skipped.
- Baked mesh maps come across with the project. Maps you baked and exported from Painter can also be imported by name (`TextureSetName_MeshMapName`).
- Export presets (`.spexp`) can be imported in the export window.

## Navigation and shortcuts

The default keymap follows Painter's:

| | |
|---|---|
| Orbit | Alt+left drag |
| Pan | Middle drag |
| Zoom | Alt+right drag, or the wheel |
| Frame the model | F |
| Brush size | `[` and `]` |
| Tools | 1 paint, 2 eraser, 3 projection, 4 polygon fill, 5 smudge, 6 clone, 7 path |
| Undo / redo | Ctrl+Z / Ctrl+Y |

Prefer Blender's navigation? Pick the Blender keymap in **Settings > Keyboard and mouse** (middle drag orbits, Shift+middle pans, Ctrl+middle zooms, Home frames). Any shortcut can be rebound there: click it, then press the new key.

## Things that work differently

- **Smart materials** are `.rsm` files. Painter's `.spsm` files can't be read.
- **Substance graphs:** `.sbs` files are rendered by ReEtched itself. `.sbsar` files can't be rendered yet; in converted projects their fills show Painter's computed result.
- **Shelf content:** Adobe's starter assets aren't included. Point **Settings > Asset libraries** at your own folders of textures, alphas, materials and environments.
- **Scripting** uses your own Python 3.8+ install with ReEtched's API, not Painter's `substance_painter` module (see [Python scripting](../crates/reetched-script/README.md)).
