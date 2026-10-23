# ReEtched

An open-source 3D texture painter written in Rust, in the spirit of Adobe Substance 3D Painter — with the goal of opening your existing Painter projects and assets instead of making you start over.

> **Status: early development (Phase 1 of 9 done).** You can texture a model with layers, save projects and export game-ready textures. Painting and Substance import are not there yet. See the [roadmap](docs/PLAN.md).

## What works today

- Load **FBX, OBJ, glTF/GLB** meshes; each material becomes a texture set, like in Painter
- Painter-style interface: tool strip, Assets, 3D and 2D (UV) views, Texture Set List, Layers, Properties — every panel can be dragged, re-docked, closed and restored from the **Window** menu
- PBR (metal/rough) 3D view with HDR image-based lighting, tone mapping, wireframe, perspective/orthographic cameras and solo-channel view modes
- 2D UV view of the selected texture set
- **Layer stack** like Painter's: fill layers and folders, every blend mode, blend mode and opacity per channel, white/black/image masks, drag to reorder
- Fill layers with a color or image per channel (base color, metallic, roughness, normal, height) and UV, tri-planar or planar projection
- Built-in material presets — double-click or drag them onto the model, a texture set or the layer stack to add a fill layer
- **Projects** (`.retx`): save and reopen everything, including the mesh and images; undo/redo
- **Texture export** with presets for Unreal, Unity HDRP/URP, glTF, Blender and Godot; channel packing, padding, PNG/TGA/TIFF/EXR; import and save Painter export presets (`.spexp`)
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
  reetched-io/     mesh import (FBX, OBJ, glTF), tangent generation
  reetched-doc/    the document: texture sets, layer stack, undo, .retx projects, texture export
  reetched-app/    the application: UI, GPU layer compositor, viewport renderer, shaders
docs/
  PLAN.md          architecture, phases and the Substance compatibility strategy
  research/        sourced research on Painter's features, file formats and the Rust ecosystem
```

More crates (procedural graphs, baking, Substance formats) arrive with their phases — see the plan.

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
