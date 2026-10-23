# Substance 3D Painter — Feature Inventory (v12.2.0, Oct 2026)

Current version **12.2.0 (2026-10-21)**. Line: 9.0 (Jun 23) → 9.1 → 10.0 (May 24) → 10.1 → 11.0 (Mar 25) → 11.1 → 12.0 (Mar 26) → 12.1 (Jun 26) → 12.2.
Main sources: release notes https://helpx.adobe.com/substance-3d-painter/release-notes/all-changes.html · Python API https://experienceleague.adobe.com/en/docs/substance-3d-dev/painter-python/api/api-overview · legacy doc URLs `helpx.adobe.com/substance-3d-painter/<section>/<page>.html` still resolve. [unverified] marks unconfirmed items.

## 1. Project setup
- **New Project**: mesh file; template `.spt` (texture set settings, display, bake settings, shader + textures, environment); default resolution (in-app max **4096²**, export up to **8192²**; non-square allowed).
- **USD**: scope/variants (modeling variant only), subdivision level, frame; `*.geo.usd` (12.0).
- **Normal map format**: DirectX (Y−) / OpenGL (Y+). **Compute tangent space per fragment** (bitangent in pixel shader; changing it needs re-bake).
- **UDIM**: use UV tile workflow; "preserve UV tile layout per material + paint across tiles"; legacy "tiles → texture sets". Fixed at creation.
- Import cameras; **auto-unwrap**; import baked maps by `TextureSetName_MeshMapName` (suffixes `ambient_occlusion`, `curvature`, `normal_base`, `world_space_normals`, `id`, `position`, `thickness`).
- Physical size (mesh unit scale or custom). Color management: Legacy / OCIO / Adobe ACE.
- **Mesh formats**: import FBX, OBJ, DAE, PLY, glTF/GLB, USD/USDz, ABC; export all except ABC. Also SBSAR, MDL, GLSL, EXR/HDR, PSD, ABR, images, SVG, Illustrator `.ai`.
- One texture set per material ID (unless UDIM workflow).
- **Project Configuration / Reimport mesh**: change topology/UVs/texture sets; **Preserve strokes positions on mesh** (strokes stored in 3D world space and re-projected — 2D-view strokes can't be); Texture Set Reassignment window.

## 2. Texture sets & channels
- Per-set: name, description, size (dynamic, non-destructive), shader instance, per-UV-tile resolution. Removing a channel keeps its data.
- **Channel formats**: sRGB8, L8, RGB8, L16, RGB16, L16F, RGB16F, L32F, RGB32F (+ "color channel" flag).
- **Channels**: Base Color, Height, Specular, Specular Edge Color, Specular Level, Opacity, Emissive, Displacement, Glossiness, Roughness, Metallic, Normal, Ambient Occlusion, Diffuse, Reflection, IOR, Anisotropy Level/Angle, Transmissive, Translucency, Scattering, Scattering Color, Absorption Color, Blending Mask, Sheen Opacity/Color/Roughness, Coat Opacity/Color/Roughness/Specular Level/Normal, User channels (count version-dependent: 8 / 16 / 32). **OpenPBR 1.1 is default since 12.1** (exact channel names [unverified]).
- **Mixing**: Normal mixing Replace/Combine; height→normal Sharp/Smooth(Sobel); AO Replace/Multiply; **UV padding** 3D Space Neighbor (default) / 2D Space Neighbor.
- **Mesh map slots**: Normal, World Space Normal, ID, AO, Curvature, Position, Thickness, Height, Bent Normals, Opacity.
- Texture set list: visibility/focus, shader instances, import shader params (JSON), reassign, sub-stacks (Dynamic Material Layering), named UV tiles (11.0).

## 3. Layer stack
- **Layers**: Paint, Fill (material/colors/bitmaps per channel + projection), Folder/Group (can have effects), Smart Material (folder), Instance. **Flatten** (12.0).
- **Per-channel blend mode + opacity** for every layer; copy/paste blending options.
- **Blend modes** (linear space): Normal, Passthrough, Disable, Replace, Multiply, Divide, Inverse Divide, Darken (Min), Lighten (Max), Linear Dodge (Add), Subtract, Inverse Subtract, Difference, Exclusion, Signed Addition (AddSub), Overlay, Screen, Linear Burn, Color Burn, Color Dodge, Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Tint, Saturation, Color, Value, Normal Map Combine (Whiteout), Normal Map Detail (RNM; default for normal), Normal Map Inverse Detail. Height defaults to Linear Dodge.
- **Masks** (grayscale): white, black, bitmap, color selection, height combination (Compare Mask). Isolate/disable/invert background; **smart masks**.
- **Geometry mask**: by mesh name or by UV tile, include/exclude.
- **Effects** (content stack + mask stack, each with blend/opacity): Generator (SBSAR fed by mesh maps), Paint, Fill, Levels (histogram, clamp), **Compare Mask** (sources: layers below / this layer / this mask / constant; lesser / within tolerance / greater; hardness), Filter (SBSAR), **Anchor Point** (referenced from fills/effects/substance inputs; below only, same set), Color Selection.
- **Instancing**: across texture sets; only source editable; cycle detection.

## 4. Painting tools
- **Tools**: Paint, Eraser, Path, Projection (incl. physical/particle), Polygon Fill, Smudge, Clone (V sets source), Material Picker, Quick Mask. Smudge/Clone typically on Passthrough layers.
- **Brush**: size & flow (pressure), stroke opacity, spacing, angle, follow path, jitter (size/flow/angle/position), **alignment** (Camera, Tangent|Wrap, Tangent|Planar, UV), backface culling angle, **size space** (Object/Viewport/Texture), alpha (bitmap or SBSAR w/ `hardness`), physics, **stencil** (S+mouse transform, tiling), per-channel material toggles, material mode, additive blending (12.2), **dynamic strokes** (Substance-driven per-stamp), lazy mouse, Shift-click lines, ABR import.
- **Symmetry**: mirror (axis/position), radial (axis, count, flip). Fill symmetry (11.1).
- **Fill projections**: Fill (per UV tile), UV, Tri-planar, Planar, Spherical, Cylindrical, Warp (grid), UV-set→UV-set, auto Warp-to-geometry (12.0). Filtering, wrap, crop, depth/culling, hardness, UV transform or **physical size**, 3D gizmo.
- **Polygon Fill**: marquee, loop, partial loop (12.2; quads); modes Triangle/Polygon/Mesh/UV chunk; hit-test intersection/center; visible only.
- **Path tools** (9.0+): paint/erase/smudge along path, **Ribbon** (11.1, corner joins, tiling, 75 presets), **Filled path** (11.0); per-vertex pressure, tangents, snapping. 3D only.
- **Text resource** (fonts), **SVG**, Illustrator art boards.
- Normal/height/AO channel painting.

## 5. Baking
- **Maps**: Normal, World Space Normal, ID, AO, Curvature, Position, Thickness, Height, Bent Normals, Opacity.
- **Common**: size, dilation width, diffusion, low-as-high, multiple high meshes, **cage** (distance frontal/rear, automatic (11.0 exp.), custom file), ignore backface, **match** always/by mesh name (suffixes), antialiasing.
- **Per baker**: ID source (vertex color, material color, file ID, mesh ID/polygroup; random/hue/gray); AO (secondary rays, min/max distance, spread, distribution, self occlusion, attenuation, ground plane); curvature (method, radius, tonemapping); position (axis, normalization); thickness; height; bent normals.
- **Skew** painting & auto-rebake (12.1), **cage painting** (12.2). GPU raytracing (incl. AMD since 11.1).

## 6. Rendering / viewport
- Shaders: PBR metal/rough, spec/gloss, non-PBR, **ASM** (spec PDF: https://helpx.adobe.com/content/dam/substance-3d-ecosystem-and-plugins/Adobe+Standard+Material+-+Specification.pdf), **OpenPBR 1.1 default (12.1)**; alpha test/blend, SSS, **OIT** (12.2), material layering shader, displacement/tessellation.
- Environment: HDR IBL, exposure, rotation, blur, shadows. Camera: FOV, DoF, **TAA** (16/64).
- Post: DoF, bloom, glare, lens flare, aberration, vignette, sharpen, grain, tone mapping (Sensitometric default, etc.), color correction, LUT. **Rewritten in 12.0.**
- View modes: Material, Solo channel, Mesh maps. **2D UV view** (paintable). Wireframe, grid. Iray path tracer (F10). Vulkan (Win/Linux), Metal (mac).

## 7. Export
- Global + per-set overrides; size 128–8192; **padding**: none, dilation infinite, dilation+transparent, dilation+bg color, dilation+diffusion; dithering.
- Formats: PNG 8/16, TGA 8, TIFF 8/16/32F, EXR 16F/32F, PSD 8/16, JPEG, JPEG-XR, HDR, PFM, others.
- **Output templates**: Gray/RGB/R+G+B/RGB+A/R+G+B+A packing; tokens `$project $mesh $textureset $udim $colorSpace`; sources = channels, mesh maps, converted maps (Normal GL/DX, Mixed AO, Diffuse, Specular, Glossiness, Reflection, 1/IOR, f0, …).
- Bundled presets: PBR MR/SG, Unreal (packed/SSS), Unity HDRP/URP, Arnold, Blender, Corona, Redshift, RenderMan, V-Ray, Keyshot, CryEngine, Roblox, USDz, glTF, …
- USD export (ASM/OpenPBR), SBSAR export, shader param JSON, mesh export (with displacement), Send to Photoshop/Stager/After Effects.

## 8. Assets
- Assets panel: search/tags/favorites; libraries (project/user/custom). Types: materials (SBSAR), smart materials, smart masks, filters, generators, brushes, particles, tools, alphas, textures, environments, shaders, fonts, vectors.
- Substance 3D Assets panel (10.0), auto-update (11.0), resource versioning (`resource://…?version=hash`).

## 9. Scripting
- **Python** (`substance_painter`, API ~0.3.7, Python 3.13): application, baking, colormanagement, display, event, export, js, layerstack, logging, project, properties, resource, source, textureset, ui. Full layer-stack editing since 10.0. `ScopedModification` for batched undo.
- **Legacy JS/QML** API (plugins: Autosave, Resources Updater).
- **Shader API**: GLSL `void shade(V2F inputs)`, `//: param auto <tag>`, `//: param custom {...}`, `//: state`, `//: metadata`, `//: stacks/materials`, libs lib-pbr/sampler/sparse/…; channels sampled as `SamplerSparse` (SVT).

## 10. Other
- History panel; autosave plugin; Save and compact, Save as template, Clean; 16-bit lossless compression.
- Auto-unwrap (seams/islands/packing, margin, texel density tiles, hard-surface 12.1).
- **Color management**: Legacy, **OCIO v2** (ACES 1.x / 2.0), Adobe ACE/ICC; per-bitmap color space; display transform.
- **Sparse Virtual Textures** (HW + SW fallback, disk cache); parallel stroke compute.
- No Firefly/generative-AI features in 9.0–12.2 release notes.
