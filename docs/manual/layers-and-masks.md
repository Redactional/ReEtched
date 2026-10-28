# Layers and masks

Every texture set has its own stack of layers. Layers are combined from the bottom up, channel by channel, and masks control where each layer shows.

## The layer stack

The **Layers** panel shows the stack for the texture set selected in the **Texture set list**. The top layer covers the ones below it.

There are three kinds of layer:

| Kind | What it holds |
|------|---------------|
| **Paint layer** | Brush strokes. Paint on it with the painting tools. See [Painting](painting.md). |
| **Fill layer** | A material that covers the whole texture set: fixed values, images or a procedural material for each channel. See [Fills, images and materials](fills-and-materials.md). |
| **Folder** | A group of layers. A folder has its own blend modes, opacity and mask, which apply to everything inside it. |

Each row shows, from left to right: an arrow to open or close the folder (folders only), the eye button, the layer's thumbnail, its mask thumbnail (if it has a mask), its name, its blend mode and its opacity.

## Adding and removing layers

The buttons at the top of the **Layers** panel (hover over one to see its name) are:

| Button | What it does |
|--------|--------------|
| **Add paint layer** (brush icon) | Adds an empty paint layer. |
| **Add mask** | Opens a menu with **Add white mask** and **Add black mask** for the selected layer. |
| **Add fill layer** | Adds a fill layer with a plain grey material. |
| **Add folder** | Adds an empty folder. |
| **Save as smart material…** | Saves the selected layer or folder as a smart material. See [Smart materials](fills-and-materials.md#smart-materials). |
| **Duplicate** | Adds a copy of the selected layer, named “… copy”. |
| **Delete (Del)** | Deletes the selected layer, or the selected mask. |

New layers are added just above the selected layer, or at the top of the stack if nothing is selected, and are selected straight away.

You can also add a fill layer by dragging a material from the **Assets** panel onto the stack. Dropping it on a layer adds it above that layer.

Press **Delete** to delete the selected layer. If a mask is selected, only the mask is deleted. **Ctrl+Z** undoes.

## Selecting and reordering

- Click a layer's name to select it and show its settings in the **Properties** panel.
- Click a mask thumbnail to select the mask.
- Click an empty part of the panel to clear the selection.
- Drag a layer by its name onto another layer to move it above that layer. Drag it onto a folder to put it inside the folder.
- Click the arrow next to a folder to open or close it.

To rename a layer, select it and edit **Name** in the **Properties** panel.

## Visibility

Click the eye button on a row to hide or show that layer. Hidden layers keep their contents and their names are shown greyed out.

## Blend modes and opacity per channel

Each layer blends separately in every channel. The list at the top left of the **Layers** panel picks the channel the stack is showing — **Base color** by default. The blend mode and opacity on each row are for that channel only.

For example, to make a scratch layer change only the roughness:

1. Choose **Roughness** in the channel list.
2. Leave the layer's roughness blend mode at **Normal** and opacity at 100.
3. Choose **Base color** in the channel list and set the layer's base color opacity to 0, or its blend mode to **Disable**.

The opacity box goes from 0 to 100; drag it or click it to type a value. The blend mode list offers:

Normal, Disable, Replace, Multiply, Divide, Inverse Divide, Darken (Min), Lighten (Max), Linear Dodge (Add), Subtract, Inverse Subtract, Difference, Exclusion, Signed Addition (AddSub), Overlay, Screen, Linear Burn, Color Burn, Color Dodge, Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Tint, Saturation, Color, Value, Normal Map Combine, Normal Map Detail, Normal Map Inverse Detail.

Folders also offer **Passthrough**.

New layers start with **Normal** in most channels, **Normal Map Detail** in the Normal channel and **Linear Dodge (Add)** in the Height channel.

## Channels

Every texture set has seven core channels: **Base color**, **Metallic**, **Roughness**, **Normal**, **Height**, **Emissive** and **Opacity**.

You can add more channels in the **Texture set settings** panel, under **CHANNELS**:

1. Click **Add channel**.
2. Choose a channel.

The extra channels are: Specular level, Specular color, Anisotropy, Anisotropy angle, Transmission, Scattering, Scattering color, Coat, Coat color, Coat roughness, Sheen, Sheen color, Sheen roughness and Ambient occlusion. Painted ambient occlusion is multiplied with the baked one.

Remove an extra channel with the **×** next to it (**Remove this channel**). The core channels can't be removed.

If the shader you've picked for the texture set reads channels the set doesn't have yet, a button such as **Add the 3 channels this shader reads** appears; click it to add them all.

Once added, a channel appears in the stack's channel list, in the brush's **MATERIAL** section and in each fill layer's **MATERIAL** section.

## Layer thumbnails

A layer's thumbnail shows its content in the channel chosen at the top of the stack. Paint layers with paint, and fill layers that use an image, show a small picture of it. Plain fill layers show a color swatch, and empty paint layers and folders show an icon.

## Masks

A mask controls where a layer (or folder) shows: white shows the layer, black hides it, and grey shows it partly.

### Adding a mask

1. Select a layer.
2. Click **Add mask** at the top of the **Layers** panel.
3. Choose **Add white mask** (the layer shows everywhere until you hide parts) or **Add black mask** (the layer is hidden until you reveal parts).

The new mask is selected, ready to paint or edit.

### Mask settings

Select a mask by clicking its thumbnail. The **Properties** panel shows:

| Setting | What it does |
|---------|--------------|
| **Enabled** | Turns the mask on or off. You can also **Shift+click** the mask thumbnail. A turned-off mask is drawn dimmed. |
| **Background** | **White** or **Black** — the mask's starting value before paint, images or effects. |
| **Invert** | Swaps white and black. |
| **Bitmap** | Fills the mask with a greyscale image (**Use an image…**) or with text (**Use text…**). Click **×** (**Remove image**) to take it out. When a bitmap is set, a **PROJECTION** section lets you place it; see [Projections](fills-and-materials.md#projections). |

### Painting a mask

1. Click the mask thumbnail to select the mask.
2. Choose the Paint tool (**1**) or Polygon fill (**4**).
3. Set **Mask value** in the brush's **MATERIAL** section: 1 paints white, 0 paints black.
4. Paint on the model. Use the Eraser (**2**) to remove mask paint.

All painting tools work on masks. See [Painting](painting.md).

### Mask effects

Effects build or adjust a mask from the model's shape, so wear and dirt land in believable places. A mask can have up to four effects.

1. Select the mask.
2. In **EFFECTS**, click **Add effect** and choose one.

Every effect has:

- A checkbox to turn it on or off.
- **Move up** / **Move down** and **Remove** buttons.
- **Blend** — how it combines with the mask so far: **Normal**, **Multiply**, **Add**, **Subtract**, **Min** or **Max**.
- **Opacity** — how strongly it applies.

#### Generators

Generators make a mask from the texture set's baked mesh maps. If the map a generator needs hasn't been baked, it shows a red **Needs a baked … map** note. See [Baking](baking.md).

| Generator | What it picks out | Needs |
|-----------|-------------------|-------|
| **Edge wear** | Sharp, outward edges. | Curvature map |
| **Cavity dirt** | Creases and inward corners. | Curvature map |
| **Ambient occlusion dirt** | Areas hidden from light. | Ambient occlusion map |
| **Position gradient** | A ramp along the X, Y or Z axis. Choose the axis with **Axis**. | Position map |
| **Thin areas** | Thin parts of the model. | Thickness map |
| **Noise** | Procedural noise only. | Nothing |
| **Light** | Surfaces facing a light. Set its direction with **Light direction** (around and height). | Nothing |

Generators share these settings:

| Setting | What it does |
|---------|--------------|
| **Type** | Switches to a different generator. |
| **Level** | Where the cut-off falls. Move it to grow or shrink the masked area. |
| **Contrast** | How sharp or soft the transition is. |
| **Noise** | How much noise breaks up the edge. |
| **Noise scale** | How fine the noise is. |
| **Invert** | Swaps the result. |

#### Levels

**Levels** remaps the mask's values with **Input** (low and high), **Gamma** and **Output** (low and high). Use it to boost contrast or soften the mask.

#### Color selection

**Color selection** masks the parts of the model that have particular colors in the baked ID map. It needs a baked ID map.

1. Add a **Color selection** effect.
2. Click **Pick from model**, then click the model in the 3D view. The color under the cursor is added to **Colors**.
3. Repeat to add up to four colors. Click **✕** next to a color to remove it, or click the swatch to edit it.
4. Adjust **Tolerance** to include more or fewer similar colors.

## Smart masks

A smart mask is a saved mask — its settings, bitmap, paint and effects — that you can put on any layer.

To save one:

1. Select a mask.
2. Click **Save as smart mask…** at the bottom of its properties.
3. Name the file and save it. It's saved as a `.rsmk` file, in **My library** unless you choose another place.

To use one:

1. Select the layer you want to mask.
2. In the **Assets** panel, click **Smart masks**.
3. Double-click the smart mask. It replaces the layer's current mask, if it has one.

See [Asset library](asset-library.md) for more on library folders.

## UDIM tiles

If the texture set uses several UDIM tiles, a layer's properties show a **UDIM tiles** row with a button for each tile. Turn a tile off to keep the layer from affecting it.
