# Keyboard and mouse

ReEtched comes with two keymaps: one that follows Substance 3D Painter and one that follows Blender. This page lists every default shortcut and mouse control for both, and shows how to change them.

## Choosing a keymap

1. Choose **File → Settings…**.
2. Under **KEYBOARD AND MOUSE**, pick a **Preset**:
   - **Substance Painter** (the default)
   - **Blender**

The change takes effect right away. The line under the preset reminds you how that preset navigates the 3D view.

> **On macOS,** press **Cmd** instead of **Ctrl** for the keyboard shortcuts and for Blender-style zooming. The painting controls further down use the **Control** key.

## Keyboard shortcuts

Most shortcuts are the same in both presets.

| Action | Substance Painter | Blender |
|---|---|---|
| New project | Ctrl+N | Ctrl+N |
| Open project | Ctrl+O | Ctrl+O |
| Save | Ctrl+S | Ctrl+S |
| Save as | Ctrl+Shift+S | Ctrl+Shift+S |
| Export textures | Ctrl+Shift+E | Ctrl+Shift+E |
| Undo | Ctrl+Z | Ctrl+Z |
| Redo | Ctrl+Y or Ctrl+Shift+Z | Ctrl+Shift+Z |
| Delete layer | Delete | X or Delete |
| Frame the model | F | Home or . (period) |
| Smaller brush | [ | [ |
| Bigger brush | ] | ] |
| Paint tool | 1 | 1 |
| Eraser | 2 | 2 |
| Projection tool | 3 | 3 |
| Polygon fill | 4 | 4 |
| Smudge | 5 | 5 |
| Clone | 6 | 6 |
| Path tool | 7 | 7 |

A few notes:

- Shortcuts need the exact modifier keys. **Ctrl+Shift+S** is **Save as**, not **Save**.
- **Frame the model** works on the view under the pointer: it frames the model in the 3D view, or fits the UV tile in the 2D view.
- **Delete layer** deletes the selected layer. If a mask is selected, it removes the mask instead.
- Undo, redo, delete, framing, brush size and tool keys don't fire while you're typing in a text field.

## Navigating the 3D view

| To | Substance Painter | Blender |
|---|---|---|
| Orbit | Alt + left drag | Middle drag |
| Pan | Middle drag (with or without Alt) | Shift + middle drag |
| Zoom | Alt + right drag, or mouse wheel | Ctrl + middle drag, or mouse wheel |
| Frame the model | F | Home or . (period) |

The **Fit view** button on the toolbar also frames the model.

## Navigating the 2D view

The 2D view uses the same controls in both presets.

| To | Do this |
|---|---|
| Pan | Middle drag, or Alt + left drag |
| Zoom | Mouse wheel, or Alt + right drag |
| Fit the UV tile | The **Frame the model** key (F, or Home in the Blender preset) |

## Mouse controls while painting

These work the same in both presets. While you hold **Alt**, clicks in the views don't paint.

| Tool | To | Do this |
|---|---|---|
| Paint, Eraser, Smudge, Clone, Projection | Paint a stroke | Left drag |
| Paint, Eraser, Smudge, Clone, Projection | Draw a straight line from the end of the last stroke | Shift + click |
| Clone | Set the clone source | Ctrl + click |
| Projection | Move the projected image (3D view) | Ctrl + drag |
| Polygon fill | Fill the selection under the pointer | Click |
| Polygon fill | Erase the selection under the pointer | Ctrl + click |
| Polygon fill | Fill every front-facing triangle in a rectangle (3D view) | Left drag |
| Path | Add a point on the model (3D view) | Click |
| Path | Remove the last point | Backspace |
| Path | Clear all points | Esc |
| Path | Paint along the path | Enter |

See [Painting](painting.md) for how each tool works.

## Changing shortcuts

You can change the shortcut for any action in the list above.

1. Choose **File → Settings…**.
2. Under **KEYBOARD AND MOUSE**, find the action. The button next to it shows its current shortcut.
3. Click the button. It changes to **press a key…**.
4. Press the new key, together with any of **Ctrl**, **Shift** and **Alt**. Press **Esc** to cancel instead.

The new shortcut replaces all the shortcuts that action had. It applies right away and is saved when you close Settings.

Some keys are listed by name in Settings: for example **[** shows as **OpenBracket**, **]** as **CloseBracket**, and **.** as **Period**.

Your changes are kept on top of the preset, so they stay when you switch between **Substance Painter** and **Blender**. To go back to the preset's own shortcuts, click **Reset changes** next to the preset. This button only appears once you've changed something.

Mouse navigation follows the preset and can't be rebound.
