# Painter project (`.spp`) format notes

These notes back the read-only converter in `crates/reetched-spp`. Everything
here was worked out by reading projects the project owner saved in Painter
themselves (document versions 67, 76, 78 and 82), unless a line says
otherwise. No Painter program files were opened, and no Adobe code or assets
were copied. Painter projects are never added to this repository: tests that
need them read a local folder named by `REETCHED_SPP_DIR`.

## Container

An `.spp` is an HDF5 file of byte datasets:

| Dataset | Contents |
|---|---|
| `projectsettings.ini`, `editor/*`, `baking/baking.ini` | settings (Qt formats, JSON) |
| `paint/document.bin` | the project: texture sets, channels, layer stack (below) |
| `paint/scene.assbin` | the mesh: an Assimp binary dump in Painter's own layout (see `reetched-io/src/assbin.rs`), normalized to about unit size |
| `resources/<sha1>[.ext]` | embedded files: images (below), `.sbsar`, shaders |
| `resources/.alg_meta/<name>` | per-resource metadata: a Qt `QVariantMap` (origin, usage) |
| `texture-cache/<material uid>/<stack>/<channel bit>` | Painter's final computed channel textures |

**Images** (`resources/…`, `.image`): a big-endian Qt `QDataStream`
`QVariantMap` header (`width`, `height`, `format`, `pitch`, `compression`,
`dataSizeRaw`, `colorSpace`, …) followed by the pixels. `compression` is
`lz4` (a raw LZ4 block of `dataSizeRaw` bytes) or, in one autosave, `jp2`
(64 JPEG 2000 tiles; not decoded yet). Formats seen: `RGBA16`, `LUM16`,
`RGBA16F`, `RGBA32F`, `BGRA8`, `LUM8`. Rows run top to bottom.

**Texture cache entries**: the same kind of header (`internalFormat` is an
OpenGL format: `SRGB8_ALPHA8`, `RG8`, `RG16F`, `RGBA16F`), then a `QByteArray`
(big-endian length + an LZ4 block). Rows run bottom to top. Two-channel
formats hold value and coverage.

## `paint/document.bin`

Header: `u32 0x1B7C2FDD`, `u32 encoding`, `u32 document version`, little-endian.
The decoder picks the encoding by the second word, not the version.

**Type codes** (shared by both encodings): 0x01 f32; 0x02–0x04 2–4 × f32;
0x05 i32; 0x06 2 × i32; 0x09 enum (8 bytes inline, 4 bytes registry); 0x0a
bool byte; 0x0b u32; 0x0c u64; 0x0d 3×3 f32; 0x0e 4×4 f32; 0x0f, 0x15 8-byte
integers; 0x10 string (u32 length + UTF-8); 0x11 / 0x13 lists; 0x12 / 0x14
objects.

**Inline encoding (0, versions up to 78)**: every value is a `u8` type code
then its payload. Object: `u8` null flag (0xFF = null), `u32` byte size, type
name, `u16` field count, then (field name, value) pairs. List: `u32` byte
size, `u32` count, values.

**Registry encoding (1, version 82)**: `u32` root code, `u32` number of
declared types, then the root object. A type reference is an `i32`: -1
declares a type (name, `u32` field count, then (field name, `u32` code)
pairs, appended to a table); anything else indexes the table. Objects: `u32`
absolute end offset (0 = null), type reference, field values in declaration
order (no tags). 0x13 lists: count, then objects with their own headers.
0x11 lists: count, one type reference, then packed records.

Both decode to the same tree: `DataDocument.materials[DataMaterial]` (one per
texture set: `sceneMaterialName`, `sceneMaterialUID`, `resolutionLog2`,
`additionalMaps` / `normalMapBitmap` for baked maps) → `stacks[0]`
(`channels[DataChannel]`, `stack.items`). Layers are `DataLayerColor` and
`DataLayerGroup` (`subStack`), listed **bottom first**, with `label`,
`enabled`, `perChannelBlending[DataBlending]`, `maskActions`, `maskInitial`
(0 black, 1 white), `actions`. Actions: `DataActionFill` (sources
`DataSourceUniform` / `DataSourceBitmap` / `DataSourceProcedural` /
`DataSourceText`), `DataActionPaint` / `Eraser` / `Smudge` (brush settings and
`strokes`), `DataActionGroup` (`subStack`), filters, decals. Missing fields
mean defaults (fields differ between versions).

## Codes matched up

- **Channel bits** (`DataChannel.type`, `channelTypes` masks, cache names):
  1 base color, 2 height, 128 roughness, 8192 metallic, 4194304 normal (from
  pixel formats, and metallic reading 255 on a chrome part). All bits set =
  every channel.
- **Blend modes** (`blendingMode`): 0 Disable, 1 Replace (filters' default),
  2 Normal (opacity-only overrides), 25 Passthrough (groups), 4 Multiply (a
  baked AO image at 20 % over a whole model). 9, 10 and 11 are matched only
  from layer names (highlights → Screen, edge highlight / height noise →
  Linear Dodge, shadows → Multiply) and need confirming in Painter.
- **Fill projection**: 0 UV, 2 tri-planar, 7 planar; 6 appears for
  generators in masks (treated as UV). `uvSamplingWrap` 0 = the image
  appears once. UV transform: Painter scales and rotates about the image
  center, which `translation` moves from (0.5, 0.5). Planar: `dddSize` holds
  half-extents (non-square images), `dddEulerAngles` are radians, and
  `backfaceCulling` keeps the image off surfaces facing away.
- **Strokes**: `DataStroke.points` are normalized view positions (**y up**)
  with the view's `viewMatrix` / `projectionMatrix` (column-major, square
  aspect), in the normalized mesh space. `duplicateMatrix` (with
  `duplicateCount`) is the symmetry copy, an object-space mirror even for
  2D-view strokes. 2D-view strokes (`viewType` 0) store all-zero matrices and
  their points are UVs. Brushes are Painter's round "shape" brush with a
  `hardness` tweak; `brushSize` (size space 1) is relative to the normalized
  scene (radius ≈ half the size) or, in the 2D view, to the UV square.
- **Polygon Fill** (`DataActionDecal`): `hits` are clicks
  (`DataDecalHitPunctual.position`) and rectangles (`DataDecalHitRect`
  `position0/1`), in the same coordinates as stroke points, with the inverse
  view-projection in `projViewInvMatrix` (all zero for 2D-view hits, whose
  positions are UVs). `granularity`: 0 triangle, 1 polygon, 2 mesh, 3 UV
  island; `color` is the fill value. A rectangle takes the faces whose
  center falls inside it on screen, including faces turned away from the
  camera.
- **Mask generators**: Painter's Light generator (`light.sbsar`, the most
  used) takes the baked world normal and position maps and `Horizontal_Angle`
  (turns), `Vertical_Angle` (-1..1), `light_attenuation`, `invert`. Baked
  maps named `…normalobj` are world-space normals. Fitted against Painter's
  output on a project whose highlight mask is almost pure Light: the angles
  give the direction *away* from the light, and the mask is
  ((1 + n·L) / 2)² × (1 − attenuation / 2) (correlation 0.83).
- **Mask contents** are listed bottom first; a fill at Normal and full
  opacity covers the paint below it.
- **Brush footprints**: Painter paints whatever the brush disk overlaps on
  screen, even when its center misses the mesh (close-up views). Projections
  use OpenGL depth (near plane at z = -1).
- **Painter's result**: `texture-cache` holds the final channels per texture
  set; the converter puts them in a top "Painter result" layer.

## Provenance

- Container layout, images, texture cache, the inline encoding, all codes
  above: worked out from the owner's projects (byte inspection, then checked
  on every project).
- The registry encoding: worked out from one owner project, then confirmed on
  all 14 version-82 projects. The public, MIT-licensed
  [Universal-Painter-Files](https://github.com/Yeusepe/Universal-Painter-Files)
  project confirmed that two encodings exist (Painter 8.1–10 inline, 11+
  registry); none of its code was used.
- Qt `QDataStream` and Assimp's binary dump are public formats (Assimp's
  loader, BSD-3-Clause, is credited in `assbin.rs`).
- Blend mode names and their order: Painter's public Python API documentation
  (the stored numbers follow a different order).
