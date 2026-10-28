# The interface

This page walks through ReEtched's main window: its panels, menus, toolbar and status bar, and the Settings window. The layout follows Substance 3D Painter's, so if you know Painter you'll find most things where you expect them.

## Layout at a glance

From left to right, the main window has:

- the **tool strip**, a column of tool icons on the far left
- the **ASSETS** panel
- the **3D VIEW** and **2D VIEW**, side by side in the middle
- the **TEXTURE SET LIST**, **LAYERS** and **PROPERTIES** panels stacked on the right
- a narrow strip on the far right with buttons for **Display settings** and **Log**

The menu bar and toolbar run along the top. The status bar runs along the bottom.

## Panels

### Tool strip

The icons on the far left pick the active tool. Hover over an icon to see its name and shortcut.

| Tool | Key |
|---|---|
| Paint | 1 |
| Eraser | 2 |
| Projection | 3 |
| Polygon fill | 4 |
| Smudge | 5 |
| Clone | 6 |
| Path | 7 |

Picking a tool shows its settings in **PROPERTIES**. See [Painting](painting.md) for what each tool does.

### Assets

**ASSETS** holds the materials and files you can use in your project. The row of icons at the top picks a category:

- **Materials**
- **Smart materials**
- **Procedural materials (.sbs)**
- **Smart masks**
- **Brushes**
- **Alphas**
- **Textures**
- **Environments**
- **Shaders**
- **All library files**

Type in the **Search** box to filter the list by name. To use a material, double-click it, or drag it onto the model, a texture set or the layer stack. See [Fills, images and materials](fills-and-materials.md) and [Asset library](asset-library.md).

### 3D view

The **3D VIEW** shows your model with its current textures. You paint on it directly. Two menus float at its top-left corner:

- **Perspective** / **Orthographic** switches the camera projection.
- The view mode menu shows the full **Material**, a single channel such as **Base color** or **Roughness**, or one of the baked **MESH MAPS**.

See [Keyboard and mouse](shortcuts.md) for how to move around, and [Viewport, rendering and color](viewport-and-color.md) for how the view is drawn.

### 2D view

The **2D VIEW** shows the selected texture set flattened out along its UVs. You can paint here too. Each UDIM tile is outlined and labeled with its number (and its name, if you gave it one). The view mode menu at the top-left works like the one in the 3D view.

### Texture Set List

**TEXTURE SET LIST** shows one row per texture set (one per material in your mesh). Click a name to select that texture set; the **LAYERS** panel and the 2D view follow your selection. Click the eye icon to show or hide that texture set in the 3D view. You can drop a material from **ASSETS** onto a row to add it to that texture set.

### Layers

**LAYERS** shows the layer stack of the selected texture set. From here you add paint layers, fill layers, folders and masks, and change blend modes and opacity. Click a layer to select it and show its settings in **PROPERTIES**. See [Layers and masks](layers-and-masks.md).

### Texture Set Settings

**TEXTURE SET SETTINGS** shares a tab group with **LAYERS**. It shows the selected texture set's **Size** (resolution), **Shader**, its **CHANNELS** (with **Add channel**), its **UDIM TILES** if it has any, and its baked **MESH MAPS**. See [Projects and meshes](projects-and-meshes.md) and [Baking mesh maps](baking.md).

### Properties

**PROPERTIES** shows the settings of whatever you're working with:

- After you pick a tool, it shows that tool's settings: brush size, flow, hardness and so on, plus the material it paints.
- After you click a layer, it shows that layer's settings.

If nothing is selected, it says **Select a layer to edit its properties**.

### Display Settings

**DISPLAY SETTINGS** controls how the views look, not your textures: the **ENVIRONMENT** lighting, **RENDERING** options, **COLOR MANAGEMENT**, the **CAMERA**, and the wireframe overlays under **VIEWER**. See [Viewport, rendering and color](viewport-and-color.md).

### Log

**LOG** lists messages with the time since ReEtched started: files opened and saved, exports, warnings and errors. Errors are shown in a different color. The newest message also appears in the status bar.

## Arranging panels

Every panel sits in a tab. You can rearrange them to suit you:

- **Move a panel:** drag its tab to another place in the window. Drop it next to another panel to split the space, or onto another tab bar to group them.
- **Resize panels:** drag the gap between two panels.
- **Close a panel:** click the close button on its tab.
- **Bring a panel back:** open the **Window** menu and check the panel's name. The buttons on the right-hand strip open **Display settings** and **Log** directly.
- **Start over:** choose **Window → Reset layout** to return every panel to where it started.

ReEtched remembers your layout and restores it the next time you open it.

## Toolbar

The toolbar under the menu bar has:

| Button | What it does |
|---|---|
| **3D / 2D view** | Shows both views side by side |
| **3D view only** | Hides the 2D view |
| **2D view only** | Hides the 3D view |
| **Fit view** | Frames the model in the 3D view and fits the 2D view. **F** does the same for the view under the pointer |

## Status bar

The left side of the status bar shows the latest message from the **LOG**. The right side shows how much GPU memory ReEtched's caches use, the name of your graphics card, and the ReEtched version.

## Menus

### File

| Item | What it does |
|---|---|
| **New project…** | Starts a project from a mesh. See [Projects and meshes](projects-and-meshes.md) |
| **Open project…** | Opens a `.retx` project |
| **Open recent** | Lists the projects you opened or saved lately |
| **Save** / **Save as…** | Saves the project |
| **Save and compact** | Drops images and files no layer uses any more, then saves |
| **Save texture set as template…** | Saves the selected texture set's setup as a project template |
| **Reimport mesh…** | Swaps in a changed version of your mesh and keeps the paint |
| **Export textures…** | Opens the export window. See [Exporting](exporting.md) |
| **Bake mesh maps…** | Opens the baking window. See [Baking mesh maps](baking.md) |
| **Load environment…** | Loads an HDR or EXR image to light the views |
| **Import Substance graph (.sbs)…** | Adds a procedural material. See [Fills, images and materials](fills-and-materials.md) |
| **Import Painter project (.spp)…** | Converts a Substance 3D Painter project. See [Coming from Substance Painter](painter-projects.md) |
| **Import Painter bridge…** | Imports a folder exported from Painter. See [Coming from Substance Painter](painter-projects.md) |
| **Settings…** | Opens the Settings window |
| **Quit** | Closes ReEtched |

### Edit

**Undo** and **Redo**, with their current shortcuts.

### Python

| Item | What it does |
|---|---|
| **Console** | Opens the Python console |
| **Plugins…** | Turns plugins on and off |
| **Run script…** | Runs a `.py` file |
| **Open the plugins folder** | Shows the folder ReEtched loads plugins from |

Plugins can add their own items to this menu. If Python isn't available, **Run script…** is grayed out and the menu tells you why. See [Python scripting](scripting.md) and [Troubleshooting](troubleshooting.md#python-and-scripting).

### Window

A checkbox for each panel, to show or hide it, and **Reset layout**.

### Help

| Item | What it does |
|---|---|
| **Documentation** | Opens the documentation in your browser |
| **Report a problem** | Opens the issue tracker in your browser |
| **Check for updates** | Checks whether a newer version is out |
| **Welcome screen** | Shows the welcome screen again |
| **About ReEtched** | Shows the version and license |

## Opening files by dropping them

You can drop files onto the ReEtched window instead of using the menus. ReEtched decides what to do from the file type:

- a mesh (FBX, OBJ, glTF, GLB, USD) opens as a new project
- a `.retx` file opens as a project
- an `.spp` file is converted from Substance 3D Painter
- an `.hdr` or `.exr` image becomes the environment
- an `.sbs` file is added as a procedural material
- an image (PNG, JPEG, TGA, TIFF, SVG) is added as a fill layer with the image as its base color, or, with the Projection tool active, becomes the projection image

## The welcome screen

The welcome screen appears when ReEtched starts. It has buttons for **New project…**, **Open project…**, **Import Painter project…** and **Documentation** under **START**, and your **RECENT PROJECTS** on the right. Each recent project shows when it was last changed and which folder it's in. Projects that can't be found any more are dimmed; click the **✕** next to one to remove it from the list.

Clear **Show this screen at startup** if you don't want it. **Help → Welcome screen** brings it back.

## Settings

Open the Settings window with **File → Settings…**. Your changes are saved when you close the window.

### General

| Setting | What it does |
|---|---|
| **Autosave every** | How often ReEtched autosaves, in minutes. 0 turns autosave off. Autosaves are offered back after a crash |
| **GPU cache budget** | How much video memory, in MB, ReEtched keeps paint and image textures in before it drops some and reloads them when needed. 0 means automatic |
| **Interface size** | Makes the whole interface bigger or smaller, from 75 % to 200 % |
| **Startup** | **Show the welcome screen** |
| **Updates** | **Check for a new version at startup**. ReEtched only tells you about a new version; it doesn't download or install anything |
| **Python** | Which Python install scripting uses. **Choose…** picks one; **Automatic** goes back to finding one for you |

### Asset libraries

Lists the folders that show up in the **ASSETS** panel. Click **Add library folder…** to add one. You can rename a library by editing its name. Click the **✕** next to a library to remove it from the list; the files in it stay where they are. See [Asset library](asset-library.md).

The **Open the settings folder** button shows the folder where ReEtched keeps your settings, autosaves and templates.

### Keyboard and mouse

Pick a **Preset**, **Substance Painter** or **Blender**, and change individual shortcuts. See [Keyboard and mouse](shortcuts.md#changing-shortcuts).

### Developer

**Show the console window (debug log)** opens a separate window with a detailed log. On macOS and Linux this setting is called **Debug log (in the terminal ReEtched was started from)**. See [Troubleshooting](troubleshooting.md#the-debug-log).

## Updates

If **Check for a new version at startup** is on and a newer version is available, ReEtched shows a small **Updates** window. Click **Open the download page** to get it. To check by hand, choose **Help → Check for updates**.
