# Python scripting in ReEtched

ReEtched runs Python scripts and plugins with **your own Python install**
(CPython 3.8 or newer, from python.org, the Microsoft Store, a distro package
or a venv). Without Python, ReEtched works normally and the Python menu shows
"Python 3.8+ not found".

Which Python is used: the `REETCHED_PYTHON` environment variable (a python
executable or an install folder) if set, else `py -3`, `python3` or `python`
from PATH, else (Windows) the registered and standard install locations.
Packages you `pip install` into that Python are importable from scripts.

## The console

**Python ▸ Console** opens an interactive console. Code typed there runs in
one persistent namespace (variables survive between runs), and the value of
a trailing expression is printed like in the Python REPL. Errors show the
full traceback with line numbers. **Python ▸ Run Script…** runs a `.py` file
as `__main__` in a fresh namespace.

`print()` output from plugins, timers and event callbacks goes to the log
panel (stdout as info, stderr as errors).

## Plugins

A plugin is a `.py` file or a package folder (with `__init__.py`) in a
plugin folder (shown under **Python ▸ Plugins**). Names starting with `_` or
`.` are ignored. A plugin may define:

```python
def start_plugin():   # called when the plugin is enabled
    ...

def close_plugin():   # called when it is disabled, reloaded, or the app exits
    ...
```

Menu actions, event connections and timers a plugin registers are removed
automatically when it closes, so `close_plugin` only needs to undo things of
your own. **Reload** re-imports the plugin (and its submodules) from disk.
Plugin folders are on `sys.path`, so a package plugin can import its own
modules normally.

```python
# my_tools.py
from reetched import ui, layerstack, textureset

def add_rust_layer():
    ts = textureset.get_active()
    fill = layerstack.insert_fill(layerstack.InsertPosition.from_texture_set_stack(ts))
    fill.name = "Rust"
    fill.active_channels = ["basecolor", "roughness"]
    fill.set_channel_value("basecolor", (0.4, 0.12, 0.05))
    fill.set_channel_value("roughness", 0.85)

def start_plugin():
    ui.add_action("My Tools/Add Rust Layer", add_rust_layer)
```

## Threads

All `reetched` calls must happen on the app's thread — in the console, a
plugin's start/close, a menu action, an event callback or a timer. Calling
from another thread raises `RuntimeError`; do the background work there and
hand the result back with `reetched.ui.call_later(0, callback)`.

## API overview

| Module | What it does |
|---|---|
| `reetched.project` | `is_open()`, `file_path()`, `name()`, `needs_saving()`, `open(path)`, `save(path=None)`, `create(mesh_path, template=None)`, `close()`, `undo()`, `redo()` |
| `reetched.textureset` | `all_texture_sets()`, `get_active()`, `set_active(ts)`; `TextureSet`: `name`, `resolution` (get/set, int or `(w, h)`), `channels()`, `has_channel()`, `add_channel(id)`, `remove_channel(id)`, `shader` (`"metal_rough"`, `"asm"`, `"openpbr"`), `udim_tiles()` |
| `reetched.layerstack` | `get_root_layer_nodes(ts)`, `get_selected_nodes()`, `set_selected_nodes(nodes)`, `insert_fill/insert_paint/insert_group(position)`, `delete_node(node)`, `move_node(node, position)`, `InsertPosition.from_texture_set_stack(ts)/above_node(n)/below_node(n)/inside_node(group)`, `BlendingMode`; nodes (`FillLayerNode`, `PaintLayerNode`, `GroupLayerNode`): `id`, `name`, `visible`, `blending_mode(ch)`, `set_blending_mode(mode, ch)`, `opacity(ch)`, `set_opacity(v, ch)`, `has_mask()`, `add_mask("white"/"black")`, `remove_mask()`, `children()`, `parent()`; fill: `active_channels`, `set_channel_value(ch, value)`, `set_channel_bitmap(ch, resource)`, `channel_value(ch)` |
| `reetched.resource` | `import_project_resource(path)` → `Resource(id, name)`, `list_project_resources()` |
| `reetched.export` | `list_presets()`, `export_project_textures(config)` → result with `.files` / `.all_files()`, `export_mesh(path, format=None)` |
| `reetched.baking` | `bake(texture_sets=None, resolution=2048, maps=MAPS, high_poly=None)` (waits until done) |
| `reetched.display` | `get_exposure()/set_exposure()`, `get_environment()/set_environment(path)`, `get_tone_mapping()/set_tone_mapping(name)` |
| `reetched.event` | `DISPATCHER.connect(EventClass, callback)` / `disconnect`; `ProjectOpened`, `ProjectCreated`, `ProjectAboutToSave`, `ProjectSaved`, `ProjectClosed`, `ExportTexturesStarted`, `ExportTexturesEnded`, `TextureSetChanged`, `LayerStackChanged`, `BakingEnded`, `Shutdown` (connect to `Event` for all) |
| `reetched.logging` | `info(msg)`, `warning(msg)`, `error(msg)` |
| `reetched.ui` | `add_action("Menu/Sub/Label", callback)` → handle, `remove_action(handle)`, `call_later(seconds, cb)`, `call_every(seconds, cb)`, `cancel(handle)` |

Channel ids: `basecolor, metallic, roughness, normal, height, emissive,
opacity, specularlevel, specularedgecolor, anisotropylevel, anisotropyangle,
transmissive, scattering, scatteringcolor, coatopacity, coatcolor,
coatroughness, sheenopacity, sheencolor, sheenroughness, ambientocclusion`.
Colors are linear RGB tuples. When the app refuses something (no project
open, unknown channel, deleted layer…) you get `reetched.ReEtchedError`.

Export config (Painter style):

```python
export.export_project_textures({
    "exportPath": "C:/out",
    "defaultExportPreset": "Unreal Engine",          # a name from list_presets()
    "exportList": [{"rootPath": "Body"}],             # omit for all texture sets
    "exportParameters": [{"parameters": {
        "fileFormat": "png", "bitDepth": "8",
        "paddingAlgorithm": "infinite", "dilationDistance": 16}}],
})
```

## Substance 3D Painter scripts

`import substance_painter` gives a compatibility layer so typical small
Painter scripts run unchanged. Covered:

- `project`: `is_open`, `file_path`, `name`, `needs_saving`, `open`, `save`, `save_as`, `create`, `close`, `is_busy`, `execute_when_not_busy`, `ProjectSaveMode`
- `textureset`: `all_texture_sets`, `get_active_stack`, `set_active_stack`, `TextureSet` (`name`, `get/set_resolution`, `all_stacks`, `get_stack`, `has_uv_tiles`, `all_uv_tiles`), `Stack` (`material`, `all_channels`, `has_channel`, `add_channel`, `remove_channel`, `from_name`), `ChannelType`, `ChannelFormat`, `Resolution`
- `layerstack`: `get_root_layer_nodes`, `get/set_selected_nodes`, `insert_fill/paint/group`, `delete_node`, `move_node`, `InsertPosition` (`from_textureset_stack`, `above_node`, `below_node`, `inside_node` with `NodeStack.Substack`), node `get/set_name`, `is/set_visible`, `get_type`, `uid`, `get/set_blending_mode`, `get/set_opacity`, `has_mask`, `add_mask`, `remove_mask`, `sub_layers`, `get_parent`, fill `active_channels`, `set_source`, `get_source`; `BlendingMode`, `NodeType`, `MaskBackground`, `ScopedModification`
- `resource`: `import_project_resource`, `Resource.identifier()`, `ResourceID`, `Usage`
- `export`: `export_project_textures` (→ `TextureExportResult` with `status`, `message`, `textures`), `list_resource_export_presets`, `export_mesh`, `ExportStatus`
- `event`: `DISPATCHER`, `ProjectOpened`, `ProjectCreated`, `ProjectEditionEntered`, `ProjectAboutToSave`, `ProjectSaved`, `ProjectAboutToClose`, `ExportTexturesAboutToStart`, `ExportTexturesEnded`, `LayerStacksModelDataChanged`, `BakingProcessEnded`
- `logging`: `info`, `warning`, `error`, `log`
- `ui`: `add_action(menu, action)` with a `(menu, "Label", callback)` form or any object with `text()`/`trigger()`, `delete_ui_element`, `ApplicationMenu`
- `colormanagement.Color`, `exception.ProjectError` (= `ReEtchedError`)
- `js.evaluate` raises `NotImplementedError`

Not available: Qt/PySide UI (docks, widgets, real `QAction`s), JavaScript,
custom export preset definitions inside the export config (use a named
preset), layered materials (one stack per texture set), effects and masks
beyond add/remove, `ScopedModification` undo grouping (each call is its own
undo step). `ProjectAboutToClose` fires after the close.
