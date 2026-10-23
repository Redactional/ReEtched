# Substance File Formats — Interoperability Research

Research date: 2026-10-23. **VERIFIED** = read in a primary/near-primary source (Adobe docs, source code, statute). **[unverified]** = recollection, weak source, or inference. Not legal advice.

## Summary table

| Format | Container | Documented? | Open-source parser | Difficulty |
|---|---|---|---|---|
| `.spp` | **HDF5**; many datasets hold proprietary "HBO" binary object streams | No | Partial: [Yeusepe/Universal-Painter-Files](https://github.com/Yeusepe/Universal-Painter-Files) (MIT, Python/h5py) | Very hard. Container trivial; HBO object graphs are version-specific (8.1→12.x); painted-pixel encoding undocumented |
| `.sbsar` | **7-Zip**: `.xml` metadata + `.sbsasm` compiled graph + PNG icon | XML partly (via pysbs) | XML: several. `.sbsasm`: **none found** | XML easy. Rendering without Adobe's engine = RE bytecode + bit-exact atomic nodes: extremely hard |
| `.sbs` | Plain XML | Only via Adobe pysbs object model | DiffMat (MIT/Adobe **non-commercial**) | Parse: moderate. Evaluate at parity: hard but tractable |
| `.spsm` / `.spmsk` / `.sppr` / `.spt` | [unverified] HDF5 + HBO like `.spp` | No | None | Same blocker as `.spp` |
| `.spexp` | [unverified] JSON-like text | Python export JSON schema is documented | None | Easy |
| `.pkfx` | PopcornFX (third-party, proprietary) | No | None | Hard (needs PopcornFX runtime) |
| `.glsl` shaders | GLSL + Painter `//:` directives | **Yes** (Adobe shader API docs) | n/a | Moderate |
| `.sbsprs` | XML [unverified] | No | None | Easy |

## 1. `.spp` — project file

VERIFIED (from Universal-Painter-Files source/README):
- An `.spp` is an **HDF5 container**: groups, datasets, attributes, settings, embedded binary streams, shaders, baked data, checksums.
- Known datasets: `projectsettings.ini` (version keys `versionAtCreation/*`, `versionAtLastSave/*`); `texture-cache*` (regenerable cache, Painter recomputes on open); `paint/document.bin`, `paint/default_material.bin` (HBO streams).
- **HBO header**: 12 bytes LE — magic `0x1B7C2FDD`, `version` u32, `data version` u32. v10 `paint/document.bin` uses data version 81. Painter 11/12 use a "registry-style binary format"; 8.1/9/10 use an "older inline tagged format".
- Changed datasets need fresh `m3_x64_128` attributes (MurmurHash3 x64-128, seed `0xF13A0239`). Any writer must reproduce these.
- Compression = standard HDF5 filters per dataset. No custom zlib/LZ4/zstd layer found.
- Resources referenced by keys like `urlToBitmapRes`, `urlToSbsRes`; may live under GUID-named groups. Embedded substances are 7z archives.
- Universal SPP up/downgrades 12.1 ↔ 8.1 by transforming HBO trees via learned `v*_schema.json` files (learned from a corpus — unseen types can't be handled). It does **not** decode painted raster data natively; it ships a Painter-side "raster capture" plugin. Status: "still being tested". Its schemas are the best existing map of HBO types.

Difficulty: HDF5 from Rust is easy; the HBO type system is multi-version and undocumented; paint data is a black box. Person-years for robust read; write is harder.

## 2. `.sbsar`

VERIFIED:
- 7-Zip archive. Layout: `assemblies/content/0000/{icon<N>.png, Name.xml, Name.sbsasm}` ([blog](https://blog.jdboyd.net/2018/09/substance-designer-sbsprs-sbsar-file-format-notes/); confirmed by [SBSARManager](https://github.com/petersvp/SBSARManager)).
- `.sbsasm` magic: ASCII `SBAM` (`53 42 41 4D 00 00`) ([Wikidata/TrID](https://www.wikidata.org/wiki/Q105852715)).
- XML describes graphs, inputs (types, ranges, widgets, groups), outputs (usages like baseColor/normal), presets.
- **No public RE of `.sbsasm`.** Every open tool that renders `.sbsar` delegates to Adobe binaries (`sbsrender`, Substance plugins).
- Options: (a) call a **user-installed** `sbsrender`; (b) RE `.sbsasm` + implement atomics bit-exactly; (c) prefer `.sbs` sources.

## 3. `.sbs`

VERIFIED:
- XML; only "documentation" is Adobe's pysbs (SAT, proprietary, Enterprise-gated; Designer bundles `sbsbaker`, `sbscooker`, `sbsrender`, `sbsupdater`).
- **28 atomic nodes** ([Adobe reference](https://experienceleague.adobe.com/en/docs/substance-3d-designer/using/substance-graphs/nodes-reference-for-substance-graphs/atomic-nodes/atomic-nodes)): Bitmap, Blend, Blur, Curve, Directional Blur, Directional Warp, Emboss, Distance, Gradient (Dynamic), Gradient Map, FX-Map, Grayscale Conversion, HSL, Input Color, Input Grayscale, Input Value, Levels, Normal, Output, Pixel Processor, Sharpen, Channels Shuffle, SVG, Text, Transformation 2D, Uniform Color, Value Processor, Warp.
- Function graphs add a separate atomic function-node set (constants, get/set vars, math, vector, cast, compare/logic, swizzle, samplers `samplelum`/`samplecol`, FX-Map sequence/iterate). Index: https://helpx.adobe.com/substance-3d-designer/function-graphs/nodes-reference-for-function-graphs/function-node-library.html — [unverified] full list; build from pysbs enums.
- **DiffMat** (https://github.com/mit-gfx/diffmat) converts `.sbs` → differentiable PyTorch, includes FX-Map engine. **Non-commercial license** — read for reference only, don't copy.
- Material Maker (MIT): no `.sbs` import found.
- Library nodes (`sbs://` packages) are Adobe's proprietary `.sbs` files shipped with Designer: parse from user's install, [unverified] cannot redistribute.

## 4. Painter resource/preset formats

| Ext | What | Container | Notes |
|---|---|---|---|
| `.spsm` | Smart material (saved layer stack) | [unverified] HDF5 | Likely HBO like `.spp` |
| `.spmsk` | Smart mask | [unverified] HDF5 | Same |
| `.sppr` | Brush/tool/particle/material preset | [unverified] | Unknown |
| `.spt` | Project template | [unverified] `.spp`-like | Unknown |
| `.spexp` | Export preset | [unverified] JSON-like | Python export JSON is documented: https://experienceleague.adobe.com/en/docs/substance-3d-dev/painter-python/api/substancepainter-package/export |
| `.pkfx` | PopcornFX emitter/receiver pair | proprietary | Needs PopcornFX runtime |
| `.glsl` | Shaders with `//:` metadata, libs `lib-pbr`, `lib-sampler`, … | text | Documented: https://helpx.adobe.com/substance-3d-painter/scripting-and-development/api-reference/shader-api.html |
| `.sbsprs` | Substance presets | XML [unverified] | Easy |
| Shelf | Folder-based usage; IDs `resource://context/name?version=<hash>`; usages ALPHA, BASE_MATERIAL, BRUSH, COLOR_LUT, EMITTER, ENVIRONMENT, EXPORT, FILTER, FONT, GENERATOR, PARTICLE, PROCEDURAL, RECEIVER, SHADER, SMART_MASK, SMART_MATERIAL, TEXTURE, TOOL | filesystem | Easy |

**Action item:** collect real sample files and check magic bytes (`\x89HDF`, `7z\xBC\xAF`, `<?xml`, `{`) to settle the [unverified] cells.

## 5. Substance Engine / SDK / SAT licensing

VERIFIED:
- **Adobe General Terms §17(B)** forbid reverse engineering, incl. "monitoring or tracking the inputs and outputs" and decompiling; interop carve-out only where local law grants it, and "you must first request such information from us". https://www.adobe.com/legal/terms.html
- Unity/Unreal/Blender plugins are free to use (informal community-manager reply). Blender add-on needs Adobe's "Substance 3D Integration Tools" / Substance Remote Engine (separate process).
- SAT is Enterprise-gated. No license found that lets an open-source project redistribute Substance Engine / SRE / SAT.
- Only clean pattern: **optional, user-supplied integration** — detect a user-installed `sbsrender` and invoke out-of-process. Never ship or link Adobe binaries. Driving SRE's IPC would need RE → get counsel first.

## 6. Painter Python API as an exporter

Docs: https://experienceleague.adobe.com/en/docs/substance-3d-dev/painter-python/api/api-overview (API 0.3.x; changelog to 0.3.7 / Painter 12.1).

VERIFIED it can read:
- **`layerstack`** (since API 0.3.0 / Painter 10.0): node types PaintLayer, PaintEffect, FillLayer, FillEffect, GeneratorEffect, FilterEffect, LevelsEffect, CompareMaskEffect, ColorSelectionEffect, GroupLayer, InstanceLayer, AnchorPointEffect. Per node: uid, type, name, visibility, parent/siblings, `get_blending_mode(channel)`, `get_opacity(channel)`, `is_in_mask_stack()`. Per layer: content/mask effects, mask presence/background/enabled, geometry mask, instances. Projection/symmetry queries (0.3.5).
- **`source`**: SourceUniformColor, SourceBitmap, SourceVectorial, SourceSubstance, SourceReference, SourceFont. `SourceSubstance` exposes `resource_id`, `get_parameters()`, presets, image inputs/outputs, output mapping, resolution → **sbsar refs + parameter values fully exportable**.
- `resource`, `textureset`, `export` (documented JSON), `baking`, `project`, `layerstack.export_as_smart_material()/export_as_smart_mask()` (0.3.6), `js.evaluate()` bridge to legacy JS API.

VERIFIED it cannot:
- **"Strokes are not accessible from the Python API"** (Paint layer/effect docs). No documented per-layer pixel readback. Export is per channel (composited), not per layer.

Workaround [unverified, this is what Universal SPP's raster capture appears to do]: isolate one paint layer/mask (others hidden, Normal blend, opacity 1), export channels, restore. Edge cases: blend modes, passthrough groups, effects above paint layers, anchor points, UDIMs. Output is raster (strokes lost) — enough for our purposes.

## 7. Legal landscape (brief, not legal advice)

- **US DMCA §1201(f)**: circumvention allowed solely to analyse elements needed for interoperability of an independently created program. Relevant only if there's a protection measure — plain HDF5/HBO likely has none [unverified].
- **US case law**: *Sega v. Accolade* (1992), *Sony v. Connectix* (2000) — interop RE can be fair use; *Google v. Oracle* (2021) — reimplementing interfaces was fair use. **But** *Bowers v. Baystate* (2003) enforced a contractual RE ban → Adobe §17 matters for US contributors who accepted it.
- **EU Directive 2009/24/EC**: Art. 5(3) observe/study/test; Art. 6 decompilation for interop; Art. 8 contrary contract terms **null and void**. *SAS Institute v. WPL* (C-406/10, 2012): functionality and **data file formats** aren't protected by software copyright.
- Adobe asks you to **request the information first** — cheap to do, strengthens an Art. 6 position.
- **Clean room**: separate "dirty" analysts (study files, write spec) from "clean" implementers (see only spec). Prefer black-box analysis of *your own saved files* over disassembling Painter. Never copy Adobe code, shaders, or library assets. Keep provenance records.
- Precedents: LibreOffice `.doc`/`.xls` filters; Samba (SMB); Universal SPP itself.
