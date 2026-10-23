# ReEtched — Project Plan

An open-source, Rust-native 3D texture painter in the class of Adobe Substance 3D Painter, designed from day one so existing Painter users can bring their projects and assets with them.

- **Written:** 2026-10-23
- **Target:** feature parity with Painter 12.2 (released 2026-10-21)
- **Research behind this plan** (with sources) is in [research/](research/):
  - [01 — Painter feature inventory](research/01-painter-feature-inventory.md)
  - [02 — Substance file formats & legal](research/02-substance-file-formats.md)
  - [03 — Rust ecosystem & prior art](research/03-rust-ecosystem-and-prior-art.md)

---

## 0. The compatibility plan in plain terms

The goal is: **open my Painter project in ReEtched and get the same layers, masks, materials and paint, editable, not a flattened texture dump.** Here is what research says is possible, ranked by how reliable each path is.

### What the Painter files really are

| File | What it is | How open is it |
|---|---|---|
| `.spp` (project) | An **HDF5** container (a standard open scientific format). Inside are Adobe's own undocumented binary object streams ("HBO"), and the format changes between Painter versions. | The outer container is easy to read. The inside has to be reverse engineered. A community project ([Universal-Painter-Files](https://github.com/Yeusepe/Universal-Painter-Files), MIT) has already mapped much of it for Painter 8.1–12.1, but it has **not** decoded painted pixels. |
| `.sbsar` (materials, filters, generators) | A **7-Zip** archive holding XML metadata, a thumbnail, and a compiled binary graph (`.sbsasm`). | The metadata (parameters, outputs, presets) is easy to read. The compiled graph is undocumented, and **no open-source tool can render it today**. All existing tools call Adobe's own renderer. |
| `.sbs` (Designer source graphs) | Plain **XML**. | Readable. Rendering it means re-implementing Designer's 28 "atomic" nodes plus its function-graph language. Hard, but well defined. |
| `.spsm` / `.spmsk` / `.sppr` / `.spt` | Smart materials, smart masks, presets, templates. Probably the same HDF5+HBO format as `.spp` [unverified]. | Same difficulty as `.spp`. |
| `.spexp`, `.sbsprs` | Export presets, Substance presets. Likely JSON / XML text. | Easy. |
| `.glsl` shaders | GLSL with Painter's documented `//:` annotations. | Documented. Moderate work. |
| `.pkfx` | PopcornFX particle brushes (a third-party product). | Closed. Treat as out of scope. |

### The four import routes

Ranked by how reliable each one is. The plan builds all four, in this order.

| # | Route | Fidelity | Needs Painter installed? | Phase |
|---|---|---|---|---|
| **A** | **Native readers for the open formats**: `.sbs`, `.sbsar` metadata, `.spexp`, `.sbsprs`, Painter GLSL shaders, all meshes and images. | Exact | No | 1–5 |
| **B** | **"ReEtched Bridge"**: a small Python plugin that runs *inside* Painter, uses Adobe's official API, and saves the whole project to our open format. | Layer stack, blend modes, opacities, masks, effects, fill settings and Substance parameter values are **exact**. Paint layers come across as **full-resolution pixels** (Painter's API does not expose brush strokes). | Yes, once, to export | 5 |
| **C** | **Optional `sbsrender` hookup**: if the user has Substance Designer installed, we call Adobe's own renderer to render `.sbsar` materials live. | Exact | Designer | 5 |
| **D** | **Direct `.spp` / `.spsm` reading** without Painter. | Goal: same as B. Builds on Universal-Painter-Files' findings, version by version. | No | 8 (long-term R&D) |

### Limits

These are fundamental, so users should hear them up front:

1. **Brush strokes don't survive the trip. Pixels do.** Painter stores strokes and re-renders them. Its API doesn't expose them, and their encoding isn't decoded. Imported paint layers become editable pixel layers at the same resolution. Users can keep painting on them, but can't change the resolution of an old stroke without quality loss.
2. **`.sbsar` files can't be re-rendered with new parameters unless the user has Adobe's renderer installed**, or until we decode the compiled format (route D, uncertain). Without either, an imported Substance fill keeps:
   - all of its parameter values,
   - a cached render made by the Bridge, so it looks identical.
   Editing its sliders becomes possible once one of those renderers is available. `.sbs` source graphs have no such limit.
3. **Adobe's bundled library content is not ours to redistribute.** That covers the starter materials, smart materials and Designer's `sbs://` library. If a project references them:
   - we load them from the user's own Painter or Designer install when present;
   - otherwise we use the cached render from the Bridge.
   ReEtched ships its **own** material library.
4. **Writing `.spp` back** (saving a project that opens in Painter) is **not planned**. It would need full write support for the undocumented binary format plus its checksums. Our export path to Painter is textures plus `.sbsar`, the way every other tool does it.

### Legal ground rules (not legal advice; get real counsel before Phase 8)

- Adobe's General Terms §17 forbid reverse engineering, except where local law allows it for interoperability. EU law (Directive 2009/24/EC, Arts. 5, 6 and 8, and *SAS v. WPL*) protects studying file formats for interoperability and voids contract terms that forbid it.
- **Before any `.spp` reverse engineering, send Adobe a written request for format documentation and keep the reply.** Their own terms ask for this, and it strengthens the legal position.
- **Clean-room process for routes D and .sbsasm:**
  - "Analysts" study files *they saved themselves* and write a format spec.
  - "Implementers" write code from that spec only.
  - Never disassemble Painter's binaries. Never copy Adobe code, shaders, or library assets.
  - Record where every piece of format knowledge came from.
- **Licenses we can copy code from:**
  - ArmorPaint (zlib)
  - Material Maker (MIT)
  - Graphite (MIT/Apache)
  - Bevy (MIT/Apache)
  - Universal-Painter-Files (MIT; still follow the clean-room rule for its RE-derived knowledge)
- **Licenses we study but never copy from:** Blender and Krita (GPL), DiffMat (non-commercial).
- **Proposed license for ReEtched:** **MIT OR Apache-2.0**, the Rust norm, which keeps us compatible with everything above that we're allowed to borrow.

---

## 1. Architecture

### 1.1 Technology stack

All versions verified on 2026-10-23. Pin them, because wgpu ships a breaking release roughly every quarter.

| Concern | Choice | Why |
|---|---|---|
| GPU | **wgpu 30** | Cross-platform (Vulkan, DX12, Metal) with compute shaders. Painter itself runs on Vulkan and Metal. |
| Shaders | **WGSL** | Painter GLSL is imported through our preprocessor, then `shaderc`, then SPIR-V, then naga. |
| Windowing and input | **winit 0.31** | Its new Pointer API gives pen pressure and tilt. Add a WinTab backend later for older tablets. |
| UI | **egui 0.37** + `egui_tiles` (docking; `egui_dock` still targets egui 0.36) + `egui-snarl` (node graphs) | Fast to build dense pro panels. Rerun shows egui scales to this. Drive it through `egui-wgpu` and `egui-winit` so we own the frame loop. |
| Engine | **Our own wgpu renderer, not Bevy** | A painter needs on-demand, compute-heavy texture pipelines and exact memory control. Bevy's game-oriented frame loop would fight that. We borrow Bevy's PBR shader code instead. |
| Mesh import | `ufbx` (FBX + OBJ), `gltf`, `openusd` (USD), PLY fork; Alembic later | |
| Tangents | `bevy_mikktspace` | MikkTSpace must match exactly, or imported normal maps look wrong. |
| UV unwrap | xatlas through our own small binding | |
| Baking | `obvhs` (CPU BVH) + wgpu ray query (GPU, experimental) + `oidn` (denoiser) | |
| Image IO | `image`, `exr`, `tiff`, `ddsfile`/`image_dds`, `ktx2`, BCn encoders; our own minimal layered-PSD writer | |
| Formats | `sevenz-rust2` (sbsar), `quick-xml` (sbs), `hdf5` (spp, route D), `zip`, `serde_json`, `zstd`, `lz4_flex` | |
| Scripting | `pyo3` (Python) | Painter's users already script in Python, so our API should feel familiar. |

### 1.2 Repository layout

Start with **few crates** and split one out only when compile times or ownership demand it.

```
ReEtched/
├─ Cargo.toml                 (workspace)
├─ crates/
│  ├─ reetched-doc/            document model, undo/redo, native file format (.retx)
│  ├─ reetched-gpu/            wgpu context, tile atlas, compositor, brush engine, viewport renderer
│  ├─ reetched-graph/          procedural node engine (Substance-compatible atomic nodes, function graphs)
│  ├─ reetched-bake/           mesh-map bakers (CPU + GPU)
│  ├─ reetched-io/             mesh import/export, image import/export, export presets
│  ├─ reetched-substance/      .sbs / .sbsar / .spexp / Painter GLSL / Bridge import / (later) .spp
│  └─ reetched-app/            egui UI, tools, panels; the binary
├─ bridge/                    ReEtched Bridge: Python plugin that runs inside Substance Painter
├─ assets/                    our own materials, alphas, environments, shaders (all original/CC0)
├─ tests/corpus/              sample Painter projects WE created, for compatibility tests
└─ docs/
```

### 1.3 Document model: mirror Painter on purpose

This is the most important design decision. If our layer model is a **superset of Painter's**, importing is a 1:1 mapping rather than a guess. Every Painter concept gets a direct equivalent:

```text
Project
 ├─ meshes[]            (source path + embedded copy, UV sets, material IDs)
 ├─ settings            normal format (DX/GL), tangent-per-fragment, UDIM mode, physical units, color management
 ├─ texture_sets[]
 │   ├─ name, resolution (per UV tile), uv_tiles[] (with custom names)
 │   ├─ channels[]      type (all Painter channels + OpenPBR + user0..N), format (sRGB8..RGB32F), color-managed flag
 │   ├─ mixing          normal mix (replace/combine), height→normal (sharp/sobel), AO mix, padding mode
 │   ├─ mesh_maps{}     normal, ws_normal, id, ao, curvature, position, thickness, height, bent_normals, opacity
 │   ├─ shader_instance
 │   └─ stack: [Node]   (top-down)
 ├─ resources[]         content-hashed, embedded or linked; remembers original Painter `resource://` id
 ├─ environment, camera, display, post-fx
 ├─ bake_settings, export_presets[]
 └─ anchors, instances  (cross-references by node UID)

Node
 ├─ uid, name, visible
 ├─ per_channel: { blend_mode, opacity }       ← every layer, every channel
 ├─ kind: Paint{tiles} | Fill{sources per channel, projection} | Group{passthrough?} | Instance{source_uid}
 ├─ content_effects: [Effect]
 ├─ mask: Option<Mask{ enabled, background(white/black), effects:[Effect] }>
 └─ geometry_mask: by mesh names | by UV tiles, include/exclude

Effect = Paint{tiles} | Fill{source, projection} | Generator{source} | Filter{source}
       | Levels{…} | CompareMask{…} | ColorSelection{colors, tolerance} | AnchorPoint{name}

Source = UniformColor | Bitmap{resource, colorspace} | Vector{svg} | Text{font, string}
       | Substance{resource, parameters{}, output_mapping, image_inputs{}, resolution, cached_outputs?}
       | Graph{our native node graph}
       | AnchorRef{uid}

Projection = UV | Fill(per tile) | Triplanar | Planar | Spherical | Cylindrical | Warp{grid} | UVSetToUVSet
           + transform (tiling/rotation/offset or physical size), filtering, wrap, 3D gizmo, symmetry
```

**Blend modes**: implement Painter's full list, in linear space, with the same names and defaults:
- Normal, Passthrough, Disable, Replace
- Multiply, Divide, Inverse Divide, Darken, Lighten
- Linear Dodge, Subtract, Inverse Subtract, Difference, Exclusion, Signed Addition
- Overlay, Screen, Linear Burn, Color Burn, Color Dodge, Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light
- Tint, Saturation, Color, Value
- Normal Combine, Normal Detail (RNM), Normal Inverse Detail
- Defaults: Height uses Linear Dodge; Normal uses Normal Detail.

Matching their math is what makes imported projects *look* identical. We verify it with golden-image tests against exports from Painter (see §3).

### 1.4 Pixel storage: tiles from day one

wgpu has no hardware sparse textures, and Painter projects reach 4K–8K per channel across many UDIM tiles. Storage therefore works like this:

- Every paint buffer (a paint layer, or a paint effect inside a mask) is a **sparse map of 256×256 tiles**: `(uv_tile, tx, ty) → slot`.
- Tiles that were never painted cost nothing.
- GPU side: a **physical tile atlas** (`texture_2d_array`, rgba16float) plus an indirection table. This is software virtual texturing.
- **Compositing** walks the stack bottom-up **only for dirty tiles**, then writes one composited texture per channel (with mips) for the viewport.
- **Undo** is copy-on-write tiles: a stroke saves only the tiles it touched (lz4-compressed in RAM, spilled to disk past a budget).
- Export and save stream tile by tile, so 8K and 16K exports never need the whole image in memory.

### 1.5 Painting pipeline (how a brush stroke becomes pixels)

1. **Input**: winit pointer samples (position, pressure, tilt, time). Then lazy-mouse smoothing, then spacing, which produces **dabs**.
2. **Dab placement**: raycast the cursor onto the mesh (CPU BVH). Build the dab's frame from the alignment mode (Camera, Tangent|Wrap, Tangent|Planar, UV) and its size space (Object, Viewport, Texture). Apply jitter, then create symmetry copies.
3. **Splat** (GPU compute or raster):
   - Rasterize the mesh triangles under the dab **in UV space**, then project each texel into the dab frame.
   - Depth-test against the viewport depth buffer (no painting through the model).
   - Cull backfaces by angle.
   - Sample the alpha, stencil and material.
   - Write into the layer's tiles for every enabled channel.
   - This is the ArmorPaint / Mari / Blender technique.
4. **Seams**: paint in 3D space so a dab crosses UV seams naturally. Padding uses jump-flood dilation, with "3D Space Neighbor" for seams.
5. **Recomposite** the dirty tiles and redraw.

Smudge, Clone, Eraser, Projection, Polygon Fill and Path are all variants of the same splat step with different sources and placement.

### 1.6 Procedural engine (`reetched-graph`)

This one engine powers three things:
- our own materials, generators and filters;
- native rendering of `.sbs` graphs;
- possibly `.sbsar` later (route D).

How it works:
- The graph is the program. Each node has typed ports and is cached by a hash of its inputs, following Graphite's architecture.
- Image nodes run as WGSL compute passes.
- **Function graphs** (Pixel Processor, Value Processor, FX-Map, parameter expressions) are **compiled to WGSL**.
- Node set phase 1: our own nodes, covering noises, patterns, blur, warp, levels, gradient map, and so on.
- Node set phase 2: **the 28 Designer atomic nodes**, with semantics matched against Designer renders:
  - Bitmap, Blend, Blur, Curve, Directional Blur, Directional Warp, Emboss, Distance
  - Gradient (Dynamic), Gradient Map, FX-Map, Grayscale Conversion, HSL
  - Input Color, Input Grayscale, Input Value, Levels, Normal, Output
  - Pixel Processor, Sharpen, Channels Shuffle, SVG, Text, Transformation 2D
  - Uniform Color, Value Processor, Warp
- Mesh-map inputs (curvature, AO, position, world normal, thickness, ID, hard edges, UV neighbor) are exposed the same way Painter exposes them to generators.

---

## 2. Phases

Each phase ends in something **usable and testable**. Estimates assume 2–3 experienced developers. They are rough, and Phases 4 and 8 carry the most risk.

### Phase 0: Foundations (≈1–2 months)
**Goal:** a window that shows a mesh nicely.
- Cargo workspace, CI (Windows, Linux, macOS), `cargo clippy`/`fmt`, and a crash reporter.
- winit + wgpu + egui docked layout: viewport, layer stack, properties, assets, texture set list.
- Mesh import for OBJ, glTF and FBX. MikkTSpace tangents. Multiple UV sets and material IDs, which become texture sets.
- PBR viewport: metal/rough, HDR environment (IBL prefiltering), orbit/pan/zoom with Painter's Alt-mouse bindings, grid, wireframe, tone mapping.
- **Exit:** load an FBX, and see it lit by an HDRI with a flat material, at 60 fps.

### Phase 1: Document model, fill layers, export (≈2–3 months)
**Goal:** the minimum useful app: texture a model with fill layers and export game-ready textures.
- The full document model from §1.3, even where the UI doesn't expose every part yet.
- The tile storage and compositor from §1.4, with **all blend modes** and per-channel opacity.
- Fill layers with uniform colors and bitmaps per channel. Projections: UV, Triplanar, Planar.
- Groups/folders, including Passthrough.
- Texture set settings: channels, formats, resolution, normal/AO mixing.
- Undo/redo, plus the **native project format `.retx`**:
  - a zip container;
  - `project.json` for the whole document model;
  - layer tiles as zstd-compressed blobs;
  - embedded resources stored by content hash;
  - a format version and migration code from the very first release.
- **Export**:
  - output templates with channel packing and name tokens;
  - padding: none, infinite dilation, plus transparent/background/diffusion variants;
  - PNG, TGA, TIFF and EXR at 8, 16 and 32F bits;
  - bundled presets for PBR MR/SG, Unreal, Unity HDRP/URP, glTF, Blender and Godot.
- Read and write `.spexp` export presets (route A).
- **Exit:** a user can make a rust-over-paint material with two fills and a mask, save, reopen, and export Unreal-packed textures.

### Phase 2: Painting (≈3–4 months)
**Goal:** it feels like a painter.
- The brush engine from §1.5:
  - size, flow, stroke opacity, spacing, angle, follow path, jitter;
  - alignment and size-space modes;
  - backface culling;
  - lazy mouse and Shift-click lines.
- **Pen pressure and tilt** through the winit Pointer API, with a curve per parameter (the Krita sensor model).
- Paint layers, Eraser, **masks** (white/black/bitmap) with paint inside masks, and smart-mask save/load (native format).
- Alphas (bitmap), stencils (S + mouse transform, tiling), per-channel material on the brush.
- Symmetry: mirror and radial.
- 2D UV view, which is paintable.
- Seam handling, plus jump-flood padding with 3D-neighbor mode.
- Our own starter alpha and brush library.
- **Exit:** paint across a UV seam with a tablet and get no visible seam. Undo 100 strokes on a 4K set with modest memory use.

### Phase 3: Baking and mesh-aware effects (≈3 months)
**Goal:** smart-material workflows.
- **Bakers:**
  - Normal (cage by distance or custom cage; match always or by mesh name), World Space Normal, ID (vertex color, material, mesh ID)
  - AO, Curvature, Position, Thickness, Height, Bent Normals, Opacity
  - Antialiasing and dilation.
  - CPU (obvhs) first, then GPU ray query with a compute fallback, plus OIDN denoising.
- Import pre-baked maps using Painter's naming convention (`TextureSetName_MeshMapName`).
- **Effects:**
  - Levels, Compare Mask, Color Selection, Anchor Points with anchor references, Fill effects, Paint effects.
  - Geometry masks by mesh or UV tile.
  - Layer instancing across texture sets, with cycle detection.
- Generators, using our own node graphs for now: edge wear, cavity dirt, AO dirt, position gradient, all driven by mesh maps.
- **Smart materials**: save a group as a reusable asset in the native format.
- **Golden-image test harness**: render our compositor and compare it against textures exported from Painter for the same simple setups. Painter is the reference oracle (see §3).
- **Exit:** bake a high-to-low asset, drop on a smart material, and get edge wear on the right edges.

### Phase 4: Procedural engine and Substance graphs (≈4–6 months, highest technical risk)
**Goal:** native procedural materials, and **`.sbs` support with no Adobe software needed**.
- `reetched-graph` core: typed graph, caching, GPU execution, resolution/seed/tiling semantics, and a node editor UI (egui-snarl).
- Function-graph compiler to WGSL.
- **The 28 atomic nodes**, starting with the most-used: Blend, Levels, Transform 2D, Blur, Warp, Directional Warp, Gradient Map, Uniform Color, Bitmap, Normal, Pixel Processor, Value Processor. Then **FX-Map**, then the rest.
- **`.sbs` reader** (route A): parse the XML and resolve dependencies.
  - Load `sbs://` library packages from the user's Designer install when present.
  - When they're missing, flag the dependency clearly instead of failing silently.
- **`.sbsar` reader for metadata** (route A): the parameter UI (types, ranges, groups, presets), outputs and thumbnail. This lets `.sbsar` files sit on the shelf and in layers, show correct parameters, and use cached renders.
- **Optional `sbsrender` integration** (route C): detect a user-installed Designer and render `.sbsar` with it out-of-process. Never bundle it.
- Our own **material library**, built natively: metals, paints, plastics, woods, fabrics, concrete, grunges. All original or CC0.
- **Exit:**
  - a corpus of simple Designer-authored `.sbs` graphs renders within a set tolerance of `sbsrender` output;
  - our own library makes the app useful with zero Adobe content.

### Phase 5: Substance Painter project import (≈2–3 months)
**Goal:** a Painter user can move a real project over.
- **ReEtched Bridge** (route B), a Python plugin for Painter 10.0 or later. It needs the `layerstack` API, which arrived in 10.0. Projects from older Painter versions can be opened in 10 or later first. The plugin:
  - walks every texture set and layer stack, and records node types, names, visibility, **per-channel blend and opacity**, mask stacks, effects, geometry masks, instances, anchors and projections;
  - exports every `SourceSubstance`: resource ID, **all parameter values**, output mapping and image inputs;
  - exports embedded resources (bitmaps, `.sbsar`, fonts, SVGs), mesh maps, texture set and channel config, shader parameters, bake settings and export presets;
  - **captures paint pixels**: for each paint layer or paint effect, it isolates that layer, sets it to Normal blend and opacity 1, exports its channels at the highest resolution Painter allows (up to 8K), then restores the stack. Unknown edge cases (passthrough groups, effects above paint, anchors) get logged and the user gets a report;
  - **caches each Substance source's rendered outputs**, so `.sbsar` fills look identical even without a renderer;
  - writes everything as an `.retx` (our native format), plus a log of anything it could not capture.
- Importer verification: re-composite the imported project and compare it per channel against Painter's own final export. Show the user a pass/fail report per texture set.
- Painter **GLSL shader** import: our `//:` directive preprocessor, then shaderc, then naga, mapping engine parameters to ours (route A).
- Map Painter's built-in library `resource://` IDs to the user's local Painter install, or fall back to the cached renders.
- Bridge for smart materials and smart masks: load the `.spsm`/`.spmsk` in Painter and save it through `export_as_smart_material`, or apply it to a temporary project and dump it.
- **Exit:** the test corpus of 20+ varied projects, all created by us, imports with zero structural differences and pixel differences under tolerance.

### Phase 6: Advanced tools and rendering parity (≈4–6 months)
- **Tools**:
  - Polygon Fill (triangle, polygon, mesh, UV chunk; marquee and loop selection)
  - Smudge, Clone
  - Projection tool
  - Path tools (paint/erase/smudge along path, filled path, ribbon)
  - Text and SVG resources
  - Dynamic strokes (graph-driven per-dab behavior)
- **More fill projections**: Spherical, Cylindrical, Warp grid, UV-set to UV-set, fill symmetry.
- **UDIM**: paint across tiles, per-tile resolution, named tiles, geometry mask by tile.
- **Project configuration**: reimport mesh, and **preserve strokes on reimport**. We store dab history in 3D, so paint can be re-projected onto new UVs. Painter can do this; most clones can't.
- Auto-unwrap (xatlas).
- **Rendering**:
  - ASM and **OpenPBR 1.1** shader models, now Painter's default;
  - SSS, alpha test/blend, OIT, displacement/tessellation;
  - TAA, post FX (DoF, bloom, tone mapping, LUT);
  - solo-channel and mesh-map view modes.
- Color management: OCIO v2 through our own binding (ACES 1.x and 2.0 configs), per-bitmap color space, display transform.
- USD import/export with materials. Mesh export with displacement. Layered PSD export.

### Phase 7: Ecosystem (≈3 months, can overlap Phase 6)
- **Python scripting API** (pyo3), modelled on `substance_painter` concepts (project, layerstack, textureset, export, resource, baking, event). Porting existing Painter scripts should feel familiar. Use our own names where needed; don't clone Adobe's docs.
- Plugin system for Python plugins, with WASM for sandboxed custom nodes later.
- Asset browser: libraries, tags, search, favorites, auto-update on disk changes.
- Autosave, crash recovery, "save and compact", project templates.
- Path-traced preview renderer (our answer to Iray; a GPU compute path tracer reusing the bake BVH).
- Performance work: shader compile caching, parallel stroke compute, VRAM budgets, simplified thumbnails.

### Phase 8: Direct Painter file reading (long-term R&D, clean-room)
**Goal:** open `.spp`, `.spsm` and `.spmsk` **without Painter installed**.
- Prerequisites:
  - Adobe has been asked in writing for documentation;
  - counsel has signed off;
  - the clean-room process (§0) is in place.
- **Step 1, the container:** read HDF5 (`hdf5` crate), list datasets, read `projectsettings.ini` and the version keys, extract embedded resources.
- **Step 2, structure:** decode HBO streams (12-byte header, magic `0x1B7C2FDD`), one Painter version family at a time: 11/12 registry format first, then 8.1–10 inline format. Universal-Painter-Files' public schemas are the starting point; the clean-room spec is ours. Map them onto the same document model the Bridge fills. **The Bridge is the oracle**: for every test project, the direct reader must produce the same document the Bridge does.
- **Step 3, paint data:** the black box. Investigate whether paint is stored as stroke records (best case: we replay strokes with our brush engine) or as cached rasters (we load the pixels). Either way it is gated on research results. **Not promised.**
- **`.sbsasm` decoding**: lowest priority, highest risk. Only attempt it if Phase 4's atomic-node implementation is mature, since the compiled graph would run on those same nodes.

### Phase 9: 1.0 polish
- UI/UX pass with Painter-familiar shortcuts as the default keymap, and an optional Blender-style keymap.
- Localization, accessibility (keyboard navigation, UI scaling), docs site, tutorials and migration guide.
- Installers (MSI, DMG, AppImage/Flatpak) and auto-update.

---

## 3. Testing strategy

| What | How |
|---|---|
| Blend modes, compositor | Unit tests per blend mode on known values. Golden images compared against **Painter exports of the same setup** (Painter is the reference oracle). |
| Brush engine | Deterministic replay: record pointer input, replay it headless, and compare tiles bit-exactly between commits. |
| Bakers | Golden images from known meshes, compared against Painter and Marmoset bakes with a tolerance. |
| Procedural nodes | Per-node golden images compared against `sbsrender` output from a dev's Designer install, with a tolerance. |
| Import | A corpus of `.spp`/`.sbs`/`.sbsar` files **we made ourselves**, not downloaded third-party files, for copyright and clean-room reasons. Bridge export → import → re-composite → compare with Painter's final export. |
| Format stability | Every `.retx` version ever shipped stays in `tests/corpus` and must keep opening. |
| Performance | Benchmarks: 4K and 8K stroke latency, composite time per dirty tile, bake time, VRAM use. Tracked in CI. |

---

## 4. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Painted strokes can't be recovered as strokes | Imported paint can't be re-rasterized | Import at max resolution. Be explicit with users. Phase 8 research. |
| `.sbsar` can't render without Adobe's engine | Imported Substance fills are "frozen" until a renderer exists | Cached renders from the Bridge; optional `sbsrender`; native `.sbs`; our own library. |
| Adobe changes the Python API or `.spp` format | Bridge or direct reader breaks | Bridge is small and version-tested; corpus per Painter version; HBO reader is version-gated. |
| Legal challenge over reverse engineering | Project risk | Route D only after the written Adobe request and counsel; clean room; no Adobe assets or code ever. |
| Blend/compositing math differs subtly from Painter | Imports "look off" | Golden-image tests against Painter from Phase 1 onward. |
| wgpu breaking changes, egui add-on crates lagging egui | Upgrade churn | Pin versions; budget one upgrade per quarter. |
| Pen input immature on some platforms | Bad painting feel | Abstract input; WinTab backend; test on Wacom, Huion and XP-Pen. |
| Scope: Painter is ~10 years of work | Never ships | Every phase ships a usable build. Phases 1–3 alone make a useful free tool. |

---

## 5. First steps

1. Create the workspace and CI, and pick the license (MIT OR Apache-2.0).
2. Phase 0: window, egui dock layout, FBX/glTF/OBJ import, PBR viewport.
3. **In parallel, starting now:** have someone with a Painter license build the **test corpus**:
   - 20+ small projects covering each layer type, blend mode, effect, projection and UDIM;
   - for each one, also save a Painter texture export.
   These are the reference oracle for every compatibility phase.
4. Collect sample `.spsm`, `.spmsk`, `.sppr`, `.spt`, `.spexp` and `.sbsprs` files and check their headers, to settle the [unverified] format details in the research notes.
5. Send Adobe a written request for `.spp` and `.sbsasm` interoperability documentation. It costs nothing, and the reply matters for Phase 8.
