# ReEtched script host protocol

The Python API reaches the app only through `ScriptHost::call(method, args)`.
`args` is always a JSON object; the result is any JSON value (`null` when
nothing is returned). Returning `Err(text)` raises `reetched.ReEtchedError(text)`
in Python, so make the text readable for a script author.

This file is the contract between `crates/reetched-python/python/reetched`
and the app. The mock in `tests/mock/mod.rs` implements all of it.

## Conventions

- **Texture set reference** (`texture_set` fields): a name (string) or an
  index (integer) into `textureset.list`'s `sets`. Python always sends names.
- **Node ids**: `u64`, unique across the whole project (not per texture set),
  stable for the node's lifetime.
- **Channel ids**: `basecolor, metallic, roughness, normal, height, emissive,
  opacity, specularlevel, specularedgecolor, anisotropylevel, anisotropyangle,
  transmissive, scattering, scatteringcolor, coatopacity, coatcolor,
  coatroughness, sheenopacity, sheencolor, sheenroughness, ambientocclusion`.
  Reject anything else with an error.
- **Blend modes**: `Normal, Passthrough, Disable, Replace, Multiply, Divide,
  InverseDivide, Darken, Lighten, LinearDodge, Subtract, InverseSubtract,
  Difference, Exclusion, SignedAddition, Overlay, Screen, LinearBurn,
  ColorBurn, ColorDodge, SoftLight, HardLight, VividLight, LinearLight,
  PinLight, Tint, Saturation, Color, Value, NormalMapCombine, NormalMapDetail,
  NormalMapInverseDetail`.
- **Colors**: linear RGB floats `[r, g, b]`. Grayscale channel values are a
  single float.
- **Paths**: strings, absolute, forward or back slashes.
- **Optional fields**: absent and `null` both mean "unchanged" / "default".
- Every call that changes the document should be its own undo step, and the
  app should emit the matching event afterwards (see Events).
- Methods that need a project fail with an error when none is open.

## Methods

### `log`
`{"level": "info"|"warning"|"error", "text": str}` → `null`

Handled by reetched-script itself: it calls `ScriptHost::log` and never
reaches `ScriptHost::call`. Python's stdout is `info`, stderr is `error`
(except while `Engine::run`/`run_file`/`trigger_action` run, where output is
captured into the `RunResult` instead).

### `project.info`
`{}` → `{"open": bool, "path": str|null, "name": str|null, "dirty": bool}`

`path` is null for a project never saved; `name` is the display name
(file stem, or the mesh name for an unsaved project). Must work with no project.

### `project.open`
`{"path": str}` → `null`

### `project.save`
`{"path": str|null}` → `{"path": str}`

With `path`: save as. Without: save to the current file; error if the project
was never saved. Returns the path written.

### `project.create`
`{"mesh": str, "template": str|null}` → `null`

`template` is a template name or file path; null = the default template.

### `project.close`
`{}` → `null`

### `history.undo`, `history.redo`
`{}` → `{"done": bool}` (`false` when there was nothing to undo/redo)

### `textureset.list`
`{}` →
```json
{
  "active": 0,
  "sets": [
    {"index": 0, "name": "Body", "resolution": [2048, 2048], "shader": "metal_rough",
     "channels": ["basecolor", "metallic", "roughness", "normal", "height"], "udims": [1001]}
  ]
}
```
`active` is an index into `sets` (null if there are none). `shader` is one of
`metal_rough`, `asm`, `openpbr`. `udims` is `[1001]` for a mesh without UDIMs.

### `textureset.update`
`{"texture_set": ref, "resolution": [w, h]?, "shader": str?, "add_channels": [id]?, "remove_channels": [id]?}` → `null`

Applies every field present. Adding an existing channel / removing a missing
one is not an error.

### `textureset.set_active`
`{"texture_set": ref}` → `null`

### `layers.tree`
`{"texture_set": ref}` → `{"nodes": [node, ...]}`

Root layers, top of the stack first. A node:
```json
{
  "id": 7, "kind": "fill"|"paint"|"group", "name": "Rust", "visible": true,
  "blending": {"basecolor": "Multiply", ...},
  "opacity": {"basecolor": 1.0, ...},
  "has_mask": false,
  "active_channels": ["basecolor", "roughness"],
  "fill": {"basecolor": [0.4, 0.1, 0.05], "roughness": 0.8, "normal": {"resource": 101}},
  "children": [node, ...]
}
```
- `blending` / `opacity`: per channel of the texture set (opacity 0..1). A
  channel missing from the map reads as `Normal` / `1.0`.
- `active_channels` and `fill`: fill layers only. `fill` holds each channel's
  value: float, `[r, g, b]`, or `{"resource": id}` for a bitmap.
- `children`: groups only, top first.

### `layers.insert`
`{"texture_set": ref, "kind": "fill"|"paint"|"group", "parent": id|null, "above": id|null, "below": id|null}` → `{"id": id}`

Position (same rules as `layers.move`):
- `above` set: directly above that node, in the same list (root or group).
- else `below` set: directly below that node.
- else `parent` set: top of that group's children (error if not a group).
- else: top of the texture set's root stack.

New nodes: name chosen by the app, visible, Normal blending, opacity 1,
no mask; a fill layer starts with all the texture set's channels active.

### `layers.update`
`{"id": id, "name": str?, "visible": bool?, "blending": {channel: mode}?, "opacity": {channel: float}?, "fill": {channel: value}?, "active_channels": [channel]?, "mask": "white"|"black"|"none"?}` → `null`

Maps are merged (only the listed channels change). `fill` values are as in
`layers.tree`; `fill` and `active_channels` on a non-fill layer are errors.
`mask`: `"white"`/`"black"` adds a mask with that fill (replacing an existing
one), `"none"` removes it.

### `layers.delete`
`{"id": id}` → `null` (deletes a group with its children)

### `layers.move`
`{"id": id, "parent": id|null, "above": id|null, "below": id|null}` → `null`

Position as in `layers.insert`, within the node's own texture set (error if
the target is in another one, or is the node itself / inside it).

### `layers.selection`
`{}` → `{"texture_set": name|null, "ids": [id, ...]}` (selection of the active texture set)

### `layers.select`
`{"ids": [id, ...]}` → `null`

Replaces the selection; all ids must be in one texture set, which becomes active.

### `resource.import`
`{"path": str}` → `{"id": u64, "name": str, "kind": str}`

Imports a file into the project. `kind`: `"texture"`, `"environment"`, ...
(whatever the app derives from the file).

### `resource.list`
`{}` → `{"resources": [{"id", "name", "kind"}, ...]}`

### `export.presets`
`{}` → `{"presets": [name, ...]}`

### `export.textures`
`{"path": str, "preset": name|null, "texture_sets": [name]|null, "format": str|null, "bit_depth": int|null, "padding": str|null, "dilation": int|null}` → `{"files": {texture set name: [path, ...]}}`

Synchronous: returns once files are written. `texture_sets` null = all.
Null fields use the preset's / app's defaults. `format`: file extension
(`png`, `tga`, `exr`, `tif`, `jpg`). `padding`: `"infinite"`, `"dilation"`,
`"transparent"`, `"color"` (Painter's names; `dilation` is in pixels).
Emits `export_started` before and `export_finished` after.

### `export.mesh`
`{"path": str, "format": str|null}` → `null` (`format` null = from the extension)

### `bake`
`{"texture_sets": [name]|null, "resolution": int, "maps": [map], "high_poly": str|null}` → `{"baked": [name, ...]}`

Synchronous. Maps: `normal, world_normal, id, ambient_occlusion, curvature,
position, thickness, height, opacity`. `high_poly` null = bake from the low
poly mesh. Emits `baking_finished` afterwards.

### `display.get`
`{}` → `{"exposure": float, "environment": str|null, "tone_mapping": str}`

`tone_mapping`: `aces`, `agx`, `neutral`, `linear`, `ocio`. `environment`:
path of the environment map (null = built-in).

### `display.set`
`{"exposure": float?, "environment": str?, "tone_mapping": str?}` → `null`

## Events

The app calls `Engine::event(host, name, data)`. Python callbacks connected
with `reetched.event.DISPATCHER.connect` run immediately; every key of `data`
becomes an attribute of the event object. Recommended payloads:

| name | Python class | data |
|---|---|---|
| `project_opened` | `ProjectOpened` | `{"path": str}` |
| `project_created` | `ProjectCreated` | `{"mesh": str}` |
| `project_about_to_save` | `ProjectAboutToSave` | `{"path": str}` |
| `project_saved` | `ProjectSaved` | `{"path": str}` |
| `project_closed` | `ProjectClosed` | `{}` |
| `export_started` | `ExportTexturesStarted` | `{"path": str}` |
| `export_finished` | `ExportTexturesEnded` | `{"ok": bool, "message": str, "files": {texture set: [path]}}` |
| `texture_set_changed` | `TextureSetChanged` | `{"texture_set": name}` (active set changed) |
| `layers_changed` | `LayerStackChanged` | `{"texture_set": name}` |
| `baking_finished` | `BakingEnded` | `{"ok": bool, "texture_sets": [name]}` |
| `shutdown` | `Shutdown` | `{}` — after the callbacks, every started plugin gets `close_plugin()` |

Unknown names still reach callbacks connected to the base `Event` class.
Events caused by a host call (e.g. `project.save` → `project_saved`) cannot be
sent from inside `ScriptHost::call` (the `Engine` is borrowed): queue them and
send them once the `Engine` method that made the call returns.

## Engine ops (C ABI, internal)

`reetched_dispatch(host, op, args_json)` returns `{"ok": result}` or
`{"error": text}`; ops: `init`, `run`, `run_file`, `event`, `plugins`,
`set_plugin_enabled`, `reload_plugin`, `actions`, `trigger_action`, `tick`.
`reetched-script` wraps these; the app never uses them directly.
