# Python scripting

You can automate ReEtched with Python: run snippets in a console, run script files, and install plugins that add their own menu entries. This page shows how to set it up and gives an overview of the API.

## Requirements

Scripting uses your own Python installation: CPython 3.8 or newer, from python.org, the Microsoft Store, a Linux distribution package, or a virtual environment. ReEtched works normally without Python; only the **Python** menu features are unavailable.

Packages you install into that Python with `pip` can be imported from your scripts.

### Which Python is used

By default, ReEtched finds Python on its own. It checks, in this order:

1. The `REETCHED_PYTHON` environment variable, if set (a Python executable or an install folder).
2. `py -3`, `python3` or `python` on your PATH.
3. On Windows, the registered and standard install locations.

To pick a specific Python instead:

1. Choose **File → Settings…**.
2. Next to **Python**, click **Choose…** and select the Python executable (for example `python.exe`).
3. Restart ReEtched.

Click **Automatic** to go back to finding Python on its own.

If no suitable Python is found, the **Python** menu shows "Python 3.8+ not found" with a hint on how to fix it.

## The Python menu

| Item | What it does |
|---|---|
| **Console** | Opens the Python console. |
| **Plugins…** | Opens the plugin list, where you enable, disable and reload plugins. |
| **Run script…** | Runs a `.py` file and opens the console to show its output. |
| **Open the plugins folder** | Opens the folder ReEtched loads plugins from. |

Menu entries added by plugins and scripts appear below these.

## The console

**Python → Console** opens an interactive console.

- Type code in the box at the bottom and press **Ctrl+Enter** or click **Run**.
- Variables you define stay available for later runs.
- If the last line is an expression, its value is printed, like in the regular Python prompt.
- Errors show the full traceback with line numbers.
- **Clear** empties the output.

The line at the top of the console shows which Python version is running.

## Running script files

**Python → Run script…** runs a `.py` file as `__main__` in a fresh namespace. Its output appears in the console.

Output from plugins, timers and event callbacks (`print()` and errors) goes to the **Log** panel instead.

## Plugins

A plugin is a `.py` file, or a package folder with an `__init__.py`, placed in the plugins folder. Use **Python → Open the plugins folder** to find it. Files and folders whose names start with `_` or `.` are ignored.

### Install and enable a plugin

1. Copy the plugin into the plugins folder.
2. Choose **Python → Plugins…**.
3. Tick the checkbox next to the plugin's name.

ReEtched remembers which plugins are enabled and starts them the next time it opens.

In the plugin list:

- **Reload** loads the plugin again from disk, including its own modules. Use it after editing the plugin.
- **not running** means the plugin is enabled but didn't start. The error is shown below it.

### Writing a plugin

A plugin can define two functions:

```python
def start_plugin():   # called when the plugin is enabled
    ...

def close_plugin():   # called when it's disabled, reloaded, or ReEtched closes
    ...
```

Menu entries, event connections and timers that the plugin registers are removed automatically when it closes. `close_plugin` only needs to undo anything else you set up yourself.

The plugins folder is on Python's import path, so a package plugin can import its own modules normally.

## API overview

Everything is in the `reetched` package:

| Module | What it does |
|---|---|
| `reetched.project` | Open, create, save and close projects; undo and redo. |
| `reetched.textureset` | List texture sets, get or set the active one; change resolution, channels and shader. |
| `reetched.layerstack` | Read the layer stack; add, delete and move fill, paint and folder layers; set blend modes, opacity, masks and fill values. |
| `reetched.resource` | Import images into the project. |
| `reetched.export` | List export templates, export textures and the mesh. |
| `reetched.baking` | Bake mesh maps. |
| `reetched.display` | Exposure, environment and tone mapping of the 3D view. |
| `reetched.event` | Run your code when something happens, such as a project being opened or saved, or an export finishing. |
| `reetched.logging` | Write messages to the Log panel. |
| `reetched.ui` | Add entries to the **Python** menu; run code later or at regular intervals. |

Channels are named by id: `basecolor`, `metallic`, `roughness`, `normal`, `height`, `emissive`, `opacity`, and so on. Colors are linear RGB tuples. When ReEtched refuses a request, for example because no project is open or a channel doesn't exist, you get a `reetched.ReEtchedError`.

All `reetched` calls must run on ReEtched's own thread: in the console, a plugin's start or close function, a menu action, an event callback or a timer. If you do work in a background thread, hand the result back with `reetched.ui.call_later(0, callback)`.

The full list of functions and classes is in the [API reference](https://github.com/Redactional/ReEtched/blob/main/crates/reetched-script/README.md).

## Examples

### Add a layer from a menu entry

```python
# rust_layer.py, placed in the plugins folder
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

After enabling the plugin, **Python → My Tools → Add Rust Layer** adds the layer to the selected texture set. A `/` in the label creates a submenu.

### Bake and export

```python
from reetched import baking, export

baking.bake(resolution=2048, maps=["ambient_occlusion", "curvature", "normal"])

result = export.export_project_textures({
    "exportPath": "C:/out",
    "defaultExportPreset": "Unreal Engine (Packed)",
    "exportParameters": [{"parameters": {"fileFormat": "png", "bitDepth": "8"}}],
})
print(result.all_files())
```

`baking.bake` waits until the bake is done. The preset name must match a template from `export.list_presets()` exactly. Leave out `"exportList"` to export every texture set, or list them with `[{"rootPath": "Body"}]`.

### React to events

```python
from reetched import event, logging

def saved(e):
    logging.info("Project saved")

event.DISPATCHER.connect(event.ProjectSaved, saved)
```

### Change the 3D view

```python
from reetched import display

display.set_exposure(1.0)          # in stops
display.set_tone_mapping("agx")    # "aces", "agx", "neutral", "linear" or "ocio"
```

## Running Substance 3D Painter scripts

ReEtched includes a `substance_painter` compatibility module, so many small scripts written for Substance 3D Painter run unchanged. It covers the common parts of the `project`, `textureset`, `layerstack`, `resource`, `export`, `event`, `logging` and `ui` modules.

Not available:

- Qt/PySide user interfaces (docks, widgets, real `QAction`s).
- JavaScript (`js.evaluate`).
- Export preset definitions inside the export config. Use the name of an existing template.
- Layered materials (ReEtched has one layer stack per texture set).
- Effects, and masks beyond adding and removing them.
- Grouping several changes into one undo step. Each call is its own undo step.

See the [API reference](https://github.com/Redactional/ReEtched/blob/main/crates/reetched-script/README.md) for exactly what's covered.
