# Coming from Substance Painter

ReEtched is laid out like Substance 3D Painter, so most of what you know carries over. This page explains how to bring your Painter projects across, what converts and what doesn't, and where ReEtched works differently.

ReEtched is an independent open-source project. It isn't made by or affiliated with Adobe. Substance 3D Painter is a trademark of Adobe.

## Open a Painter project

1. Choose **File → Import Painter project (.spp)…** and pick the `.spp` file. You can also drag the `.spp` file onto the ReEtched window.
2. Wait while ReEtched converts the project and replays the paint strokes. The log lists what was converted and what was skipped.
3. Choose **File → Save as…** and save the result as a ReEtched project (`.retx`).

The conversion only reads the `.spp` file. It never changes it, so your Painter project stays safe.

The mesh comes from the `.spp` file itself, so you don't need the original mesh file.

## What converts

| Painter | In ReEtched |
|---|---|
| Texture sets and their resolution | Converted |
| Base color, metallic, roughness, normal and height channels | Converted |
| Layer stack: folders, fill layers, paint layers, names, visibility | Converted |
| Per-channel blend modes and opacity | Converted. Layers with a blend mode ReEtched can't confirm are listed in the log. |
| Fill layers with uniform colors or bitmaps | Converted, including UV, tri-planar and planar projections |
| Masks (white or black background) | Converted |
| Paint and eraser strokes on paint layers and in masks, polygon fills in masks | Replayed with ReEtched's tools (see below) |
| Levels effects in masks | Converted |
| Baked mesh maps | Imported |

## What doesn't convert yet

These are skipped. The log tells you how many of each were found in each texture set.

- Channels other than base color, metallic, roughness, normal and height. Each one left out is named in the log.
- Procedural (`.sbsar`) fills. Their channels show Painter's own computed result instead.
- Most filters and generators, in layers and in masks.
- Fill effects inside masks.
- Smudge strokes.
- Paint on fill layers.
- Text fills.
- Additional fill effects inside a layer (only the first fill is used).
- Fill projections other than UV, tri-planar and planar. These fall back to UV projection.
- More than one material layer per texture set. Only the first is converted.
- Texture sets that don't match any part of the mesh.

## Paint is replayed, not copied

Painter stores your brush strokes rather than the painted pixels. ReEtched replays those strokes with its own brush engine, so painted layers come out close to the original but not pixel-exact.

## The "Painter result" layer

When the `.spp` file contains Painter's own computed textures, each texture set gets an extra layer at the top of its stack called **Painter result**. It holds Painter's final channels, so the project looks right straight away even where the rebuilt layers below don't match yet.

The log reminds you of this after the import. To see and edit the rebuilt layers, hide **Painter result**. When you're happy with the rebuilt layers, delete it.

## The conversion check

After converting, ReEtched compares its own result for each channel (with the **Painter result** layer hidden) against Painter's computed textures. The log shows one line per texture set and channel, for example:

```
Body / Base color: matches Painter (mean difference 0.0021, 0.3% of texels off)
```

or **differs from Painter** when the difference is larger. A summary line says how many channels matched, for example "Painter project check: 4 of 5 channels match Painter's output".

Use this to see which channels you can trust and which ones need work. The check only runs when the `.spp` file contains Painter's computed textures.

## The ReEtched Bridge plugin

The ReEtched Bridge is a plugin for Substance 3D Painter (version 10.0 or later). It exports the open project from inside Painter, including pixel captures of layers that a `.spp` conversion can't rebuild, such as procedural fills and effects.

The plugin is new and hasn't yet been tested widely with Substance 3D Painter itself. Use the `.spp` import first, and try the Bridge if layers are missing.

### Install the plugin

1. Find `reetched_bridge.py` in the `tools/painter-bridge` folder of the ReEtched source code.
2. Copy it into Painter's Python plugin folder, which is `Documents/Adobe/Adobe Substance 3D Painter/python/plugins` in your user folder.
3. In Painter, choose **Python → Reload plugins folder**, then enable **reetched_bridge**.

### Export from Painter and import into ReEtched

1. In Painter, choose **File → Export to ReEtched...** and pick a folder. The plugin writes a folder called `<project>_reetched` there. This can take a while. Your Painter project is put back the way it was afterwards.
2. In ReEtched, choose **File → Import Painter bridge…** and pick that folder. You can also drag the folder onto the ReEtched window.

ReEtched compares the result with Painter's export, just like the conversion check above.

### What the Bridge brings across

| Painter | In ReEtched |
|---|---|
| Layer stack structure, names, visibility | Exact |
| Per-channel blend modes and opacities | Exact |
| Fill layers with uniform colors | Editable fill layers |
| Paint layers, bitmap and procedural fills, effects, instances, anchors | Pixel captures in editable paint layers |
| Masks with effects or paint | Pixel captures in editable painted masks |
| Baked mesh maps | Imported |
| Base color, metallic, roughness, normal, height | Imported. Other channels are listed in the log. |

Not supported by the Bridge yet: UDIM texture sets, and more than one material layer per texture set.

## Baked mesh maps

Mesh maps baked in Painter come across with the `.spp` import. If you exported baked maps from Painter as image files, you can also import them by name with **Import Painter maps…** in **Texture set settings**. See [Baking mesh maps](baking.md#import-maps-baked-elsewhere).

## Export presets

In the export window, **Import Painter preset (.spexp)…** brings in your Painter export presets, and **Save preset as .spexp…** writes a ReEtched template in the same format. See [Exporting](exporting.md#painter-export-presets-spexp).

## Things that work differently

- **Smart materials** are `.rsm` files and smart masks are `.rsmk` files. Painter's `.spsm` smart materials can't be read; rebuild them in ReEtched and save them with **Save as smart material…** in the layer stack.
- **Substance graphs:** `.sbs` files can be imported with **File → Import Substance graph (.sbs)…** and are rendered by ReEtched. `.sbsar` files can't be rendered. In converted projects, their fills show Painter's computed result.
- **Generators** are built in: **Edge wear**, **Cavity dirt**, **Ambient occlusion dirt**, **Position gradient**, **Thin areas**, **Noise** and **Light**. See [Baking mesh maps](baking.md#how-mesh-maps-are-used).
- **Shelf content:** Adobe's starter assets aren't included. Add your own folders of textures, alphas, materials and environments under **File → Settings… → ASSET LIBRARIES** with **Add library folder…**. See [Asset library](asset-library.md).
- **Normal maps** use the OpenGL convention inside ReEtched. Export templates convert to DirectX where needed.
- **Viewport shaders:** Painter's `.glsl` viewport shaders can be loaded per texture set. See [Viewport, rendering and color](viewport-and-color.md#painter-glsl-shaders).
- **Scripting** uses your own Python 3.8+ install. ReEtched has its own `reetched` module, and also a `substance_painter` compatibility module so many small Painter scripts run unchanged. Qt/PySide interfaces don't work. See [Python scripting](scripting.md).

## Navigation and shortcuts

The default keymap, **Substance Painter**, follows Painter's:

| Action | Shortcut |
|---|---|
| Orbit | Alt + left drag |
| Pan | Middle drag |
| Zoom | Alt + right drag, or the mouse wheel |
| Frame the model | F |
| Smaller / bigger brush | `[` / `]` |
| Tools | 1 Paint, 2 Eraser, 3 Projection, 4 Polygon fill, 5 Smudge, 6 Clone, 7 Path |
| Undo | Ctrl+Z |
| Redo | Ctrl+Y or Ctrl+Shift+Z |
| Delete layer | Delete |
| Export textures | Ctrl+Shift+E |

Prefer Blender's navigation? In **File → Settings…**, under **KEYBOARD AND MOUSE**, set **Preset** to **Blender**: middle drag orbits, Shift + middle drag pans, Ctrl + middle drag zooms, and Home or `.` frames the model. In that preset, X also deletes a layer and redo is Ctrl+Shift+Z only.

Any shortcut can be changed in the same place: click it, then press the new key (Escape cancels). **Reset changes** goes back to the preset. See [Shortcuts](shortcuts.md) for the full list.
