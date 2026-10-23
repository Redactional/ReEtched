# Rust Ecosystem & Prior Art

As of 2026-10-23. **[v]** = version confirmed on crates.io/docs.rs/GitHub this session. **[u]** = unverified. Nothing was test-built.

## Recommended stack

| Area | Pick | Notes |
|---|---|---|
| GPU | **wgpu 30.0.1** [v] | Compute, native binding arrays, experimental ray query. **No hardware sparse textures.** Breaking release ~quarterly — pin versions. |
| Shaders | **WGSL** (naga_oil / WESL for modules) | GLSL import: own preprocessor → glslang (`shaderc`) → SPIR-V → naga `spv-in`. naga `glsl-in` only handles GLSL 440+ Vulkan semantics. |
| Windowing | **winit 0.30.13** stable [v] → **0.31** (beta) for pen input | 0.31 adds Pointer events with pen on Windows/Wayland/Web; macOS desktop pen unclear [u]. |
| UI | **egui 0.37** [v] + **egui_dock 0.21** [v] (lags egui by a minor) + **egui-snarl 0.12** (node editor) | `egui_tiles` as docking fallback. Curves widget: write (~300 lines). Rerun proves egui scales to pro tools. |
| Engine | **Custom wgpu renderer, not Bevy** | Bevy 0.19 [v]: game-oriented render graph, ECS extraction, continuous redraw, quarterly breakage. Use `egui-wgpu` + `egui-winit` (not eframe) to own the frame loop. Borrow Bevy's PBR WGSL (MIT/Apache). |
| FBX | **ufbx 0.11.3** [v] | Official binding of robust C lib. |
| OBJ / glTF | `tobj`, `gltf` | Mature. |
| USD | **openusd 0.6.0** [v] | Pure Rust, usda/usdc/usdz, UsdGeom/UsdShade. Pre-1.0. |
| Alembic | none maintained [u] | Low priority; bindgen later. |
| PLY | `ply-rs` forks [u] | |
| Images | `image`, `exr`, `tiff`, `ddsfile`/`image_dds`, `ktx2`, `intel_tex_2`/`basis-universal` | PSD write: write own minimal layered PSD writer (`ag-psd` is AI-generated, unaudited). |
| Color | Hardcode sRGB/linear/ACEScg in WGSL first; OCIO via own bindgen later | No maintained OCIO binding [u]. |
| CPU BVH | **obvhs 0.3.x** [v] | PLOC BVH2/CWBVH, SIMD, rayon. `embree4-rs` stale — skip. |
| GPU RT | wgpu `EXPERIMENTAL_RAY_QUERY`; fallback: upload CWBVH, traverse in WGSL compute | |
| Denoise | **oidn 2.5.x** [v] (`bundled` feature) | For low-sample AO/thickness bakes. |
| Math/mesh | `glam`, `bevy_mikktspace` (exact MikkTSpace is mandatory), `meshopt`; xatlas via own `cc`+bindgen (xatlas-rs is from 2019) | |
| Archives | `sevenz-rust2` [v] (maintained fork), `zip`, `quick-xml`/`roxmltree`, `serde`+`serde_json`/`ron`, `zstd`, `lz4_flex` | HDF5: `hdf5` crate (needs libhdf5) or pure-Rust reader [u]. |
| Watching | `notify` + `notify-debouncer-full` | Hot-reload shaders/resources. |
| Scripting | **pyo3** (Python, matches Painter's API audience); WASM (`wasmtime`) later for sandboxed nodes; `mlua` alternative | |
| Tablet | winit 0.31 Pointer + `octotablet` fallback; WinTab via small `windows-sys` FFI later | Krita supports both Ink & WinTab — some Wacom/Huion setups need WinTab. |

### wgpu details
- Store layers as **rgba16float** (filterable + blendable everywhere); rgba32float for bake intermediates. `FLOAT32_FILTERABLE` on Vulkan/DX12/Metal [v].
- `read_write` storage textures are only portable for r32 formats → design kernels to **ping-pong**.
- `max_texture_dimension_2d` default 8192; desktop GPUs 16384 [u]. Tile everything.
- No sparse resources → **software virtual texturing** (page table + physical tile atlas) from day one.

### Large textures
- Each layer channel = sparse map of 128²/256² tiles `(udim, tx, ty) → GPU slot`; untouched tiles stay constant.
- Physical GPU tile atlas (`texture_2d_array`) + indirection table. Composite only dirty tiles. Undo = copy-on-write tiles (Krita's KisTiledDataManager, on GPU).
- Viewport: composited mip chain or feedback-driven virtual texture (Barrett GDC 2008; van Waveren id Tech 5, 2009).
- Export: stream tiles to EXR/PNG.

## Prior art

| Project | License | Lessons |
|---|---|---|
| **ArmorPaint** | zlib (source); binaries sold | 1.0 (Sep 2026) rewritten Haxe→C, own engine. Paint/mask/fill layers, node materials, bake, BC7 compression. Pre-1.0 design [u]: layer packed into 3 RGBA textures; paint by rasterizing mesh in UV space and projecting each texel into brush screen space with depth test. Node graphs compile to GLSL. Closest template; zlib lets us borrow code. |
| **Material Maker** | MIT | Godot 4, ~250 nodes, each a GLSL snippet with typed ports; graph code-generated into one shader; expensive nodes buffered. No `.sbs` import. Good MIT node set to learn from. |
| **Blender** texture paint | GPL — study, don't copy | `paint_image_proj.cc`: CPU projection painting, occlusion, seam bleed. Cycles bake UX (cage/extrusion). |
| **Mari** | Proprietary | **Paint buffer** model: paint into screen buffer, project onto UDIMs. UDIM + virtual texturing pioneer. Recommended mental model. |
| **3DCoat**, **Quixel Mixer**, **InstaMAT** | Proprietary | Ptex shows UV-less painting hurts interop; Mixer's height-blended scan layers; InstaMAT "Atoms" graph-of-graphs. |
| **Krita** | GPL-3 — design only | Brush engine: dabs from tip + dynamic sensors (pressure, speed, tilt, fade, angle) each mapped through a curve; tiled COW pixel storage; Ink + WinTab. |
| **Graphite** | MIT/Apache-2.0 | Most relevant Rust node-graph architecture: graph-as-program, typed ports resolved at compile, per-node cache keyed by input hash, GPU nodes on wgpu. |
| **Darkly** | FOSS [u license] | 2D wgpu paint engine; each brush graph compiles to one GPU pipeline; 16 blend modes; serializable undo ops. |
| `texture-synthesis` (Embark) | MIT/Apache, archived | By-example synthesis. |
| `noise`, `fastnoise-lite` | MIT/Apache | CPU references; port to WGSL. |
| Rust Substance Painter clone | — | **None found.** The niche is open. |

## Algorithms & papers

- **Projection painting**: Hanrahan & Haeberli 1990; Carr & Hart 2004. Standard GPU technique: rasterize mesh in UV space (`clip = uv*2-1`) carrying world pos/normal → project texel into view/brush frame → depth-test vs viewport depth → facing falloff → evaluate stamp → blend into tile. Use conservative raster or dilate after.
- **Geodesic/mesh-space brushes, decals**: Schmidt et al. 2006 (discrete exponential maps); Crane et al. 2013 (Geodesics in Heat).
- **Seams/padding**: Sander et al. 2001; Liu et al. 2017 (Seamless); **Jump Flooding** (Rong & Tan 2006) for fast infinite dilation.
- **Baking**: cage/ray-distance + MikkTSpace (Mikkelsen 2008); AO (Zhukov 1998, Landis 2002); thickness = AO from inverted normals; curvature (Rusinkiewicz 2004) or from normal-map derivatives.
- **Triplanar**: GPU Gems 3 ch.1; Ben Golus 2017 (triplanar normal mapping).
- **Compositing**: Porter-Duff 1984; W3C Compositing & Blending L1. Composite linear premultiplied; normals via RNM/UDN; height/roughness channel-specific rules.
- **Noise**: Perlin 1985/2002; Gustavson 2005; Worley 1996; Lagae 2009/2010; Heitz & Neyret 2018 (histogram-preserving tile blending); Inigo Quilez.
- **FX-Map**: no paper; Designer docs only. Recursive quadtree splatting → GPU instanced-splat compute pass. Related: Lefebvre & Neyret 2003.
- **Virtual texturing**: Barrett 2008; van Waveren 2009.

## Risks
1. No hardware sparse textures in wgpu → software VT from day one.
2. wgpu breaks quarterly; egui_dock lags egui.
3. Stale C++ wrappers (xatlas, embree), no OCIO/Alembic bindings → own small bindgen crates.
4. Pen input not stable (winit 0.31 beta); WinTab needs FFI.
5. Substance shader-API / `.sbsar` / `.spp` compatibility is a legal + RE question, not just technical.

## Sources
wgpu https://github.com/gfx-rs/wgpu/releases · naga https://docs.rs/crate/naga/30.0.1 · winit https://github.com/rust-windowing/winit/releases · egui_dock https://docs.rs/crate/egui_dock/0.21.0 · egui-snarl https://docs.rs/crate/egui-snarl/0.12.0 · Bevy https://bevy.org/news/ · ufbx https://docs.rs/crate/ufbx/0.11.3 · openusd https://docs.rs/crate/openusd/0.6.0 · obvhs https://lib.rs/crates/obvhs · oidn https://docs.rs/oidn · sevenz-rust2 https://docs.rs/sevenz-rust2 · octotablet https://docs.rs/octotablet · ArmorPaint 1.0 https://digitalproduction.com/2026/09/12/armorpaint-1-0-swaps-haxe-for-c/ · Material Maker https://github.com/RodZill4/material-maker · Graphite https://github.com/GraphiteEditor/Graphite · Darkly https://github.com/darkly-art/darkly
