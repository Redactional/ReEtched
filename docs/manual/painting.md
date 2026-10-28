# Painting

ReEtched lets you paint straight onto the model in the 3D view, or flat onto the UV layout in the 2D view. This page covers what you paint on, the brush settings and each painting tool.

## What you paint on

Strokes always go into the layer or mask that is selected in the **Layers** panel:

- **A paint layer** — click **Add paint layer** (the brush icon) at the top of the layer stack, then paint. See [Layers and masks](layers-and-masks.md).
- **A mask** — click a layer's mask thumbnail to select the mask, then paint on it to hide or show parts of that layer.

The top of the brush settings tells you where the next stroke will land, for example *Painting on “Layer 2”* or *Painting on the mask of “Rust”*. If a fill layer or folder is selected, you'll see a reminder that it isn't a paint layer; paint strokes won't go anywhere until you select a paint layer or a mask.

## Painting in the 3D and 2D views

Press the left mouse button and drag over the model in the **3D view** to paint. The brush is projected from your view onto the surface, and a circle shows the brush size under the cursor.

You can also paint in the **2D view**, which shows the texture set's UV layout. Here the brush paints directly in texture space, which is handy for seams, tiling details or touching up a specific UV island.

Holding **Alt** stops the brush from painting so you can move the camera. See [Viewport and color](viewport-and-color.md) for navigation.

Painting only affects the texture set you have selected in the **Texture set list**.

## Choosing a tool

The tools are in the strip at the edge of the window, and each has a number key:

| Key | Tool | What it does |
|-----|------|--------------|
| **1** | Paint | Paints the brush's material. |
| **2** | Eraser | Removes paint. |
| **3** | Projection | Projects an image onto the model through the brush. |
| **4** | Polygon fill | Fills whole triangles, faces, loops, UV islands or mesh pieces. |
| **5** | Smudge | Drags existing paint along with the brush. |
| **6** | Clone | Copies paint from one place to another. |
| **7** | Path | Paints along a smooth curve through points you place. |

Choosing a tool shows its settings in the **Properties** panel. With a paint layer selected, you can also click **Brush settings** in that layer's properties to get back to them.

You can change these keys in **File > Settings…** (see [Shortcuts](shortcuts.md)).

## Brush settings

| Setting | What it does |
|---------|--------------|
| **Size** | The brush radius on screen, from 1 to 400 pt. Press **[** and **]** to make the brush smaller or bigger. |
| **Flow** | How much paint each dab lays down. Lower flow builds up gradually as you paint over the same spot. |
| **Stroke opacity** | The most a single stroke can cover. Paint never goes past this value within one stroke, however many times you go over the same area. |
| **Spacing** | Distance between dabs, as a fraction of the brush's diameter. Small values give smooth strokes; large values give a dotted line. |
| **Hardness** | How sharp the brush edge is. 0 is very soft, 1 is a hard edge. |
| **Angle** | Rotates the brush shape, from −180° to 180°. Most noticeable with an image or text brush shape. |
| **Lazy mouse** | Makes the brush trail behind the cursor by this distance (0 to 200 pt) for smoother lines. 0 turns it off. |
| **Pen pressure** | With a pressure-sensitive pen, **Size** and **Flow** can follow pen pressure. Size is on by default. |
| **Backface culling** | When ticked, the brush skips surfaces that face away from you by more than the angle you set (1° to 180°, 85° by default). This stops paint leaking onto the back of thin parts. |
| **Alpha** | The brush shape. See [Brush shapes](#brush-shapes). |
| **Symmetry** | Mirrors or repeats every dab around the model. See [Symmetry](#symmetry). |
| **Dynamic stroke** | Turns each dab into several varied dabs. See [Dynamic strokes](#dynamic-strokes). |

### Brush shapes

By default the brush is **Round**. Next to **Alpha** you can:

- Click the image icon (**Use a greyscale image as the brush shape…**) and choose an image file. White areas of the image paint; black areas don't.
- Click the text icon (**Use text as the brush shape…**) to type some text and use it as the brush. The **Text** window is described in [Fills, images and materials](fills-and-materials.md#text).
- Click the **×** next to the image name (**Use a round brush**) to go back to the round brush.

You can also double-click an alpha in the **Alphas** category of the **Assets** panel to use it as the brush shape. See [Asset library](asset-library.md).

### Symmetry

Pick a mode in the **Symmetry** list:

- **Off**
- **Mirror X**, **Mirror Y**, **Mirror Z** — every dab is also painted mirrored across the middle of the model, on the chosen axis.
- **Radial X**, **Radial Y**, **Radial Z** — every dab is repeated around the chosen axis through the middle of the model. Set the number of copies (2 to 64, 6 by default) in the box next to the list.

Symmetry applies when you paint in the 3D view.

### Dynamic strokes

A dynamic stroke places several dabs for each step of the brush, each with its own offset, size, angle and flow. Choose a preset in the **Dynamic stroke** list:

| Preset | Effect |
|--------|--------|
| **Scatter** | A few dabs scattered around the stroke, with random sizes and rotations. |
| **Spray** | Many small dabs sprayed around the stroke. |
| **Wave** | One dab that weaves from side to side and changes size. |
| **Rope** | Two dabs twisting around each other. |
| **Grass** | Several small dabs spread along the stroke with random tilts. |
| **Pressure jitter** | Pen pressure drives size, flow and a wobble across the stroke. |

Once a preset is chosen, its formulas appear below it and you can edit them. Changing one renames the preset to **Custom**.

| Field | Meaning |
|-------|---------|
| **Dabs** | Dabs per step. |
| **Along** | Offset along the stroke, in brush radii. |
| **Across** | Offset across the stroke, in brush radii. |
| **Size** | Size multiplier. |
| **Angle** | Rotation added, in degrees. |
| **Flow** | Flow multiplier. |

Each field is a small formula. You can use these values:

- `i` — this dab's number (starting at 0), `n` — the number of dabs
- `t` — distance along the stroke, in brush diameters, `s` — the step number
- `p` — pen pressure
- `r1`, `r2`, `r3`, `r4` — random numbers between 0 and 1, different for each dab

And these functions: `sin`, `cos`, `tan`, `abs`, `sqrt`, `exp`, `floor`, `fract`, `sign`, `noise`, `rand`, `pow`, `min`, `max`, `step`, `mix`, `clamp`, `smoothstep`, plus `pi`. Use `+ - * / % ^` and brackets as usual. If a formula has a mistake, the error is shown in red under the fields.

Set **Dynamic stroke** back to **Off** to paint normal strokes.

## Choosing the channels you paint

For the **Paint**, **Polygon fill** and **Path** tools, the **MATERIAL** section of the brush settings lists the texture set's channels (Base color, Metallic, Roughness, Normal, Height, Emissive, Opacity and any extra channels you've added). Turn a channel's button on to paint it, then set its value below:

- Color channels (such as **Base color** and **Emissive**) use a color picker.
- **Height** uses a slider from −1 to 1.
- **Normal** paints flat normals.
- Other channels use a slider from 0 to 1.

By default the brush paints Base color, Metallic and Roughness. If every channel is off, ReEtched tells you the brush has no channels enabled.

When a mask is selected, the **MATERIAL** section shows a single **Mask value** slider instead: 1 paints white (shows the layer), 0 paints black (hides it).

The **Eraser**, **Smudge** and **Clone** tools work on every channel the layer already has paint in, whatever the brush channels are set to.

## Straight lines

**Shift+click** to draw a straight line from where your last stroke ended to the point you click. The line must be in the same view (3D or 2D) as the last stroke.

## The tools

### Paint (1)

Paints the channels turned on in the **MATERIAL** section, or the mask value when a mask is selected.

### Eraser (2)

Removes paint from the selected paint layer or mask. It uses the same brush settings (size, flow, hardness and so on) as the Paint tool.

### Projection (3)

The Projection tool places an image over the 3D view and paints it onto the model wherever you brush. Use it to put decals, labels or photo detail exactly where you want them.

1. Select a paint layer (or a mask).
2. Press **3** or choose the Projection tool.
3. Click **Load image…** and choose an image, or click **Text…** to make an image from text.
4. Adjust the image's placement:
   - **Size** — how large the image appears in the view.
   - **Rotation** — turns the image, from −180° to 180°.
   - **Overlay** — how visible the image preview is over the model.
   - **Ctrl+drag** in the 3D view to move the image.
   - **Center** puts the image back in the middle of the view and resets its rotation.
5. Paint over the model. Wherever the brush goes, the image is painted onto the surface.

The image is painted into **Base color**. Any other channels you've turned on in the Paint tool's **MATERIAL** section are painted with their values at the same time. You can also project onto a selected mask.

The Projection tool only works in the 3D view. Because the image stays fixed to the screen, you can rotate the camera between strokes to project the same image from another angle.

### Polygon fill (4)

Polygon fill paints whole pieces of the mesh at once, with clean edges along the geometry.

Choose what a click selects with the **Select** buttons:

| Mode | What gets filled |
|------|------------------|
| **Triangle** | The single triangle you click. |
| **Polygon** | The clicked triangle and its flat neighbours — a quad or flat n-gon. |
| **Loop** | A ring of quads running along the quad edge nearest your click. |
| **UV island** | Every triangle in the same UV island. |
| **Mesh** | Every triangle connected to the one you click. |

- **Click** to fill the selection with the brush's material (or mask value).
- **Ctrl+click** to erase it instead.
- In the 3D view, **drag a rectangle** to fill every triangle facing you whose center is inside the rectangle. Hold **Ctrl** while releasing to erase instead.

Polygon fill works in both views; the rectangle only works in the 3D view. You can only fill the texture set that is selected.

### Smudge (5)

Smudge drags existing paint along with the brush, like a finger through wet paint. **Stroke opacity** sets how strong the smudge is.

### Clone (6)

Clone copies paint from one place to another.

1. **Ctrl+click** where you want to copy from. In the 2D view, an orange circle marks the source.
2. Paint somewhere else. Your first stroke sets the distance between the source and the brush; later strokes keep the same distance.
3. To start over, **Ctrl+click** a new source, or click **Forget source**.

If you start painting before choosing a source, ReEtched reminds you to Ctrl+click first.

### Path (7)

The Path tool paints along a smooth curve through points you place on the model.

1. Choose what the path does:
   - **Paint** — paints along the curve with the current brush.
   - **Erase** — erases along the curve.
   - **Smudge** — smudges along the curve.
   - **Ribbon** — draws the brush shape as one continuous band that follows the curve. Useful with an image brush shape for stitches, tape or trims.
   - **Filled** — closes the path and fills every face facing you inside it.
2. Click points on the model in the 3D view. The properties show how many points you've placed.
3. Press **Enter** or click **Paint path** to apply it.

While placing points, **Backspace** removes the last point and **Esc** clears them all. You need at least two points, all on the selected texture set. The path is painted as seen from the current view, so keep all its points on screen. Symmetry is not applied to ribbons.

## Saving brush presets

You can save the current brush — every setting on this page, including its image or text shape — as a brush preset:

1. In the **Assets** panel, click **Brushes**.
2. Click **Save brush…**.
3. Name the file and save it. It is saved as a `.rbrush` file, in the brushes folder of **My library** unless you choose another place.

To use a saved brush, double-click it in the **Brushes** category. See [Asset library](asset-library.md).
