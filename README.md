# ReEtched

An open-source 3D texture painter written in Rust, in the spirit of Adobe Substance 3D Painter — with the goal of opening your existing Painter projects and assets instead of making you start over.

> **Status: early development (Phase 6 of 9 done).** You can bake mesh maps, paint and texture a model (UDIM tiles included) with layers, procedural and smart materials, shade it with metal/rough, Adobe Standard Material, OpenPBR or your own Painter GLSL shaders under OCIO color management, open Substance Designer `.sbs` graphs and Painter projects (through a Painter plugin), save projects and export game-ready textures, PSDs and USD/OBJ meshes. See the [roadmap](docs/PLAN.md).

## What works today

- Load **FBX, OBJ, glTF/GLB** meshes; each material becomes a texture set, like in Painter. Meshes without UVs are unwrapped automatically
- **Reimport mesh** (File menu): swap in a changed mesh and keep the paint, re-projected onto the new UVs
- Painter-style interface: tool strip, Assets, 3D and 2D (UV) views, Texture Set List, Layers, Properties — every panel can be dragged, re-docked, closed and restored from the **Window** menu
- PBR (metal/rough) 3D view with HDR image-based lighting, emissive, opacity (cutout or blended), parallax displacement from height, ACES/AgX/Khronos Neutral tone mapping, bloom, vignette, progressive anti-aliasing, wireframe, perspective/orthographic cameras and solo-channel view modes
- 2D UV view of the selected texture set
- **Painting** on the model or in the 2D view: paint layers and paint inside masks, eraser, size/flow/stroke opacity/spacing/hardness/angle, image alphas, backface culling, mirror and radial symmetry, lazy mouse, Shift+click lines, pen pressure (**1** paint, **2** eraser, **[ ]** size)
- **Projection** (**3**): load an image, place it over the 3D view (Ctrl+drag, size, rotation) and paint it onto the model
- **Polygon Fill** (**4**): fill a triangle, flat polygon, UV island or whole mesh with a click, or every front face in a dragged rectangle (Ctrl erases); **Smudge** (**5**); **Clone** (**6**, Ctrl+click sets the source); **Path** (**7**): click points, Enter paints along a smooth curve through them
- **Layer stack** like Painter's: paint layers, fill layers and folders, every blend mode, blend mode and opacity per channel, white/black/image/painted masks, drag to reorder
- Channels: base color, metallic, roughness, normal, height, emissive, opacity
- Fill layers with a color or image per channel (per-image sRGB/linear color space) and UV, tri-planar, planar, spherical or cylindrical projection, with optional symmetry
- **Baking** (Texture Set Settings → Mesh maps → Bake…): normal (high to low, with cage distances), world-space normal, ID, ambient occlusion, curvature, position, thickness, height and opacity, with antialiasing and dilation; or import maps you baked in Painter (`TextureSetName_MeshMapName` naming)
- **Mask effects**: generators driven by the mesh maps (edge wear, cavity, AO, position gradient, thickness, noise), Levels and Color Selection (pick ID colors on the model)
- **Smart materials**: built-ins (Painted Steel Worn, Rusty Iron, Dusty Plastic, Gold Grime), save any layer or folder as a `.rsm` file and import it into other projects
- View modes for every baked mesh map
- **Procedural materials**: a built-in library (brushed steel, plastic, wood planks, fabric, concrete, rust, leather, bricks) with editable parameters, rendered in the background at any resolution
- **Painter projects**: the [ReEtched Bridge](tools/painter-bridge) plugin exports a project from Painter 10+; File > Import Painter bridge rebuilds its layer stack (blend modes, opacities, masks, fills) with paint as editable pixels, then checks the result against Painter's own export. The plugin has not been tried in Painter yet
- **Substance Designer `.sbs` graphs**: import them (File menu, Assets panel or drag and drop) and use them as fill layers with their own parameters — no Adobe software needed. Graphs that use Designer's `sbs://` library need Substance Designer installed; anything missing is reported
- Built-in material presets — double-click or drag them onto the model, a texture set or the layer stack to add a fill layer
- **Projects** (`.retx`): save and reopen everything, including the mesh and images; undo/redo
- **Texture export** with presets for Unreal, Unity HDRP/URP, glTF, Blender and Godot; channel packing, padding, PNG/TGA/TIFF/EXR; import and save Painter export presets (`.spexp`)
- **Mesh export** as USD (`.usda`, UsdPreviewSurface materials) or OBJ + MTL, with its textures and optional displacement from the height channel
- **Layered PSD export**: one Photoshop file per texture set and channel, keeping layers, folders, masks, blend modes and opacities
- Load your own **.hdr / .exr** environment maps
- Painter-style navigation: **Alt+LMB** orbit, **Alt+MMB** pan, **Alt+RMB** zoom, wheel zoom, **F** to frame

## Building

You need a recent stable [Rust toolchain](https://rustup.rs) (1.88+) and a C compiler (the FBX loader is C):

- **Windows:** Visual Studio Build Tools with "Desktop development with C++"
- **macOS:** `xcode-select --install`
- **Linux:** `gcc` or `clang`

```bash
cargo run --release -p reetched-app -- path/to/mesh.fbx
```

The mesh argument is optional — without it you get a preview sphere. You can also drag meshes and HDRIs onto the window.

```bash
cargo test --workspace
```

## Project layout

```
crates/
  reetched-io/     mesh import (FBX, OBJ, glTF, USD), mesh export (USD, USDZ, OBJ), topology, unwrapping
  reetched-bake/   CPU mesh map baker (BVH ray casting)
  reetched-graph/  procedural graph engine: Designer-style atomic nodes, function graphs, material library
  reetched-substance/ Substance file readers (.sbs) and the Painter bridge importer
  reetched-color/  pure-Rust OpenColorIO v2 subset: configs, transforms, LUTs, ACES 1.x / 2.x
  reetched-shader/ Painter custom shader (.glsl) translation, with our own implementation of its shader API
  reetched-doc/    the document: texture sets, layer stack, undo, .retx projects, smart materials, texture export
  reetched-app/    the application: UI, GPU layer compositor, viewport renderer, shaders
tools/
  painter-bridge/  the Painter plugin that exports projects for ReEtched
docs/
  PLAN.md          architecture, phases and the Substance compatibility strategy
  research/        sourced research on Painter's features, file formats and the Rust ecosystem
```

More crates (Painter project import, …) arrive with their phases — see the plan.

## Substance Painter compatibility

Bringing Painter projects over cleanly is a core goal. The short version of what research shows is possible (details and sources in [docs/PLAN.md](docs/PLAN.md#0-the-compatibility-plan-in-plain-terms)):

- **Open formats** (`.sbs` graphs, `.sbsar` parameters, export presets, shaders) — read natively.
- **Painter projects (`.spp`)** — via a small plugin that runs inside Painter and exports the full layer stack through Adobe's official API. Paint layers come over as full-resolution pixels.
- **Reading `.spp` directly, without Painter** — long-term research.

ReEtched is not affiliated with or endorsed by Adobe. "Substance" and "Adobe" are trademarks of Adobe Inc.

## Contributing

Contributions are welcome — please read [CONTRIBUTING.md](CONTRIBUTING.md) first, especially the rules about Adobe code and assets.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
