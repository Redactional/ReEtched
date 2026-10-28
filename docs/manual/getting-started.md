# Getting started

This page takes you from installing ReEtched to exporting your first set of textures. It covers the basics in order: install, create a project, look around, paint, save and export.

## Install ReEtched

You can install ReEtched in two ways:

- **With ReLauncher.** Get ReLauncher from [redactional.github.io/ReLauncher](https://redactional.github.io/ReLauncher/) and install ReEtched from it.
- **With an installer.** Download the installer for your system from the [latest release](https://github.com/Redactional/ReEtched/releases/latest) and run it.

### System requirements

- A graphics card that supports **Vulkan**, **Metal** or **DirectX 12**. Very old or very basic GPUs may not be able to run ReEtched. See [Troubleshooting](troubleshooting.md) if it won't start.
- **Python 3.8 or newer** (optional). You only need it for scripting and plugins. Everything else works without it. See [Python scripting](scripting.md).

## The welcome screen

When ReEtched starts, it shows the welcome screen. From here you can:

- Click **New project…** to start a new project from a mesh.
- Click **Open project…** to open a saved `.retx` project.
- Click **Import Painter project…** to convert a Substance 3D Painter `.spp` file. See [Coming from Substance Painter](painter-projects.md).
- Click **Documentation** to open the project's documentation in your browser.
- Click a project under **RECENT PROJECTS** to open it again.

You can also drop a mesh, a project or an `.spp` file onto the window.

To close the welcome screen, click the **✕**, press **Esc**, or click outside it. To stop it from appearing at startup, clear **Show this screen at startup**. You can open it again any time from **Help → Welcome screen**.

## Create your first project

1. Click **New project…** on the welcome screen, or choose **File → New project…** (**Ctrl+N**).
2. In the **NEW PROJECT** window, pick a **Template**. If you're not sure, keep **PBR Metal/Rough**. Templates are explained in [Projects and meshes](projects-and-meshes.md#project-templates).
3. Next to **Mesh**, click **Browse…** and choose your model. ReEtched reads FBX, OBJ, glTF/GLB and USD files.
4. Choose a **Document resolution**, for example 2048 × 2048. You can change it later.
5. Click **OK**.

ReEtched loads the mesh and creates one texture set for each material in it. Each texture set starts with one empty paint layer, **Layer 1**.

If your mesh has no UVs, ReEtched unwraps it for you. UVs outside the 0–1 square become UDIM tiles automatically.

## Look around the 3D view

The **3D VIEW** shows your model. With the default keymap:

| To | Do this |
|---|---|
| Orbit | Hold **Alt** and drag with the left mouse button |
| Pan | Drag with the middle mouse button |
| Zoom | Hold **Alt** and drag with the right mouse button, or turn the mouse wheel |
| Frame the model | Press **F** with the pointer over the view |

If you're used to Blender, switch to the Blender keymap in **File → Settings…**. See [Keyboard and mouse](shortcuts.md) for both keymaps.

The **2D VIEW** next to it shows the model's UV layout. You can paint in either view.

## Paint your first stroke

1. Press **1** to pick the **Paint** tool, or click the brush icon at the top of the tool strip on the left.
2. Look at the **PROPERTIES** panel. Under **PAINT** it should say *Painting on “Layer 1”*. If it asks you to select a paint layer, click **Layer 1** in the **LAYERS** panel, then press **1** again.
3. Under **MATERIAL**, click the color swatch next to **Base color** and pick a color.
4. Drag with the left mouse button over the model in the 3D view.

A few things to try next:

- Press **[** and **]** to make the brush smaller or bigger, or use the **Size** slider.
- Hold **Shift** and click to draw a straight line from where your last stroke ended.
- Press **Ctrl+Z** to undo and **Ctrl+Y** to redo.

There's much more to painting. See [Painting](painting.md) and [Layers and masks](layers-and-masks.md).

## Save your project

1. Choose **File → Save** (**Ctrl+S**).
2. Pick a folder and a name. ReEtched projects use the `.retx` extension.

After the first save, **Ctrl+S** saves to the same file. Use **File → Save as…** (**Ctrl+Shift+S**) to save a copy under a new name.

ReEtched also autosaves your work every few minutes so it can recover it after a crash. Autosave doesn't replace saving. If you quit, start a new project or open another one with unsaved changes, ReEtched asks whether to save them first. See [Projects and meshes](projects-and-meshes.md#autosave-and-crash-recovery).

## Export your textures

1. Choose **File → Export textures…** (**Ctrl+Shift+E**).
2. Next to **Output directory**, click **Browse…** and choose a folder. If you've saved the project, ReEtched suggests a `textures` folder next to it.
3. Pick an **Output template** for the engine or renderer you use, and a **File type**.
4. Under **TEXTURE SETS**, check the texture sets you want to export.
5. Click **Export**.

The **LOG** panel and the status bar tell you how many textures were written and where. For all the export options, see [Exporting](exporting.md).

## Next steps

- [The interface](interface.md): what each panel does.
- [Painting](painting.md): brushes and painting tools.
- [Layers and masks](layers-and-masks.md): building up materials in layers.
- [Fills, images and materials](fills-and-materials.md): dropping in ready-made materials.
- [Baking mesh maps](baking.md): what smart materials and generators need to work.
