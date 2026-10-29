# Fills, images and materials

Fill layers cover a whole texture set with a material — plain values, your own images or a procedural material — and masks decide where it shows. This page covers fill layers, image projections, and the built-in, procedural and smart materials in the **Assets** panel.

## Fill layers

Click **Add fill layer** at the top of the **Layers** panel to add one. A new fill layer is a plain grey material with **Base color**, **Metallic** and **Roughness** turned on.

Select the fill layer to edit it in the **Properties** panel. Its **MATERIAL** section has a button for each channel of the texture set. Turn a channel on to have the fill affect it; channels that are off leave the layers below unchanged.

Below the buttons, each channel that's on has its value:

- Color channels (such as **Base color** and **Emissive**) use a color picker.
- **Height** uses a slider from −1 to 1.
- **Anisotropy angle** uses a slider from 0° to 360°.
- **Normal** is flat unless you give it an image (a normal map).
- Other channels use a slider from 0 to 1.

To limit where the fill shows, add a mask. See [Layers and masks](layers-and-masks.md#masks).

## Using images in a fill

Any channel of a fill layer can use an image instead of a single value:

1. Select the fill layer and make sure the channel is turned on.
2. Click the image icon next to the channel (**Use an image…**).
3. Choose an image file. PNG, JPEG, TGA, TIFF, EXR, HDR and SVG files are supported.

The image's name replaces the value. Click **×** (**Remove image**) to go back to a plain value.

You can also double-click a texture in the **Textures** category of the **Assets** panel. It goes into the selected fill layer's channel that is currently chosen in the **Layers** panel's channel list, or into the selected mask. See [Asset library](asset-library.md).

You can also drag an image file from your computer onto the ReEtched window. It's added as a new fill layer with the image as its base color. (With the Projection tool active, the image becomes the projection image instead; see [Painting](painting.md#projection-3).)

### Color space

When a channel uses an image, a list next to it sets how the image's values are read:

| Option | Use it for |
|--------|------------|
| **Auto** | The default. Reads the image as sRGB in color channels (Base color, Emissive, …) and as raw values in data channels (Roughness, Metallic, Normal, Height, …). |
| **sRGB** | Color images, whatever channel they're in. |
| **Linear / raw** | Data images, or color images that are already linear. |

If you've loaded an OCIO color configuration, its color spaces are listed too, under **OCIO**. See [Viewport and color](viewport-and-color.md).

### Text

You can make an image from text for a fill channel or mask (**Use text…**), for the brush shape, or for the Projection tool (**Text…**). The **TEXT** window opens:

1. Type your text in the box. It can span several lines.
2. Choose a **Font** from the fonts installed on your computer.
3. Set **Style** (**B** for bold, **I** for italic), **Align** (**Left**, **Center** or **Right**) and **Line spacing**.
4. Click **Create**.

The text becomes white letters on a transparent background, and it stays sharp at any size. It's added to the project as an image named after the first line of the text. When you use text on a fill channel, that channel is turned on for you.

## Projections

The **PROJECTION** section of a fill layer (or of a mask that uses an image) controls how its images are wrapped around the model. Choose a mode in the **Projection** list:

| Mode | How images are placed |
|------|-----------------------|
| **UV projection** | Along the mesh's UVs. The default. |
| **Tri-planar projection** | Projected from three directions and blended, so there are no UV seams or stretching. |
| **Planar projection** | Projected straight through the model from one direction, like a slide projector. |
| **Spherical projection** | Wrapped around the model from a center point. |
| **Cylindrical projection** | Wrapped around the model like a label on a can. |
| **Warp projection** | Along the UVs, bent by a 4 × 4 grid that you drag into shape. |
| **UV set to UV set projection** | Along the mesh's second UV set. |

Settings for all modes:

| Setting | What it does |
|---------|--------------|
| **Scale** | How many times the image repeats, horizontally and vertically. |
| **Rotation** | Turns the image, from −180° to 180°. |
| **Offset** | Slides the image horizontally and vertically. |
| **Repeat** | When ticked, the image tiles. When off, it appears once and the fill covers nothing outside it — useful for decals. |

Extra settings for tri-planar, planar, spherical and cylindrical projection:

| Setting | What it does |
|---------|--------------|
| **Size** | The size, in world units, that one repeat of the image covers. |
| **Position** | Where the projector sits, in X, Y and Z. |
| **Rotation 3D** | How the projector is turned around X, Y and Z, in degrees. |
| **Symmetry** | **X**, **Y** and **Z** buttons mirror the projection across that axis. |
| **Front faces only** | Planar only. Skips surfaces that face away from the projector. |
| **Hardness** | Tri-planar only. How sharply the three directions blend into each other. |

### Warp projection

With **Warp projection**, an orange grid appears over the UV layout in the **2D view**. Drag its points to bend the image — for example to follow a curved UV shell. Click **Reset** next to **Warp grid** to flatten it again.

## Painting images onto the model

To place an image by hand — a logo, a label, a photo detail — use the **Projection** tool (**3**) on a paint layer instead of a fill. You load an image, position it over the 3D view and brush it onto the model. See [Painting](painting.md#projection-3).

## Built-in materials

The **Materials** category of the **Assets** panel holds simple ready-made materials: Aluminium, Brass, Ceramic, Chrome, Clay, Concrete, Copper, Gold, Iron, Leather, Plastic Red, Plastic White, Rubber, Silver, Skin and Wood. Each one is a fill layer with a base color, metallic and roughness value.

To use one, either:

- **Double-click** it. A fill layer is added above the selected layer of the selected texture set.
- **Drag** it onto the 3D view, the 2D view, a texture set in the **Texture set list**, or the layer stack. Dropping on a texture set adds it to that set; dropping on a layer adds it above that layer.

Type in the search box at the top of the **Assets** panel to filter the list by name.

## Procedural materials

Procedural materials are generated rather than painted: you change their settings and the textures update. Click **Procedural materials (.sbs)** in the **Assets** panel to see them.

ReEtched comes with these:

| Material | Settings |
|----------|----------|
| **Brushed Steel** | Color, Roughness, Brush scale |
| **Rough Plastic** | Color, Roughness, Bump |
| **Wood Planks** | Light wood, Dark wood, Planks, Grain rings, Roughness |
| **Woven Fabric** | Color, Threads, Roughness |
| **Concrete** | Color, Pits, Roughness |
| **Rust** | Scale, Roughness |
| **Leather** | Color, Grain scale, Roughness |
| **Bricks** | Brick, Mortar, Columns, Rows |

Double-click a procedural material, or drag it like a built-in material, to add it as a fill layer. The channels the material produces are turned on for you.

### Procedural material settings

Select the layer to see its **PROCEDURAL MATERIAL** section in the **Properties** panel:

| Setting | What it does |
|---------|--------------|
| **Resolution** | The size the material is generated at. **Texture set** matches the texture set; you can also pick 256 to 4096. |
| **Random seed** | Changes the random pattern. Click **Random** for a new one. |
| Material settings | The material's own sliders, colors, checkboxes and lists, grouped as in the original graph. |
| **Reset to defaults** | Puts every material setting back to its default. |
| **Detach** | Turns the layer into a plain fill layer. |

A spinner shows while the material is being generated. Any problems while generating appear as warnings in this section.

While a fill layer uses a procedural material, its channel values come from the material, so the per-channel values and image buttons are hidden. The **PROJECTION** settings still apply.

### Substance Designer graphs (.sbs)

You can import Substance Designer graphs (`.sbs` files):

1. Click **Procedural materials (.sbs)** in the **Assets** panel, then **Import Substance graph (.sbs)…**. You can also use **File > Import Substance graph (.sbs)…**.
2. Choose one or more `.sbs` files.

Every graph in the file that has outputs is added to the list. Use them like the built-in procedural materials. The graph's outputs are matched to channels by their usage or name (base color, metallic, roughness, normal, height, emissive, opacity, ambient occlusion and others). Graphs that read baked mesh maps (curvature, ambient occlusion, position, …) use the texture set's baked maps. See [Baking](baking.md).

Files that the `.sbs` uses from its own folder are stored in your project when you add the material, so the project keeps working without the original files.

Many graphs use nodes from Substance Designer's own library (paths starting with `sbs://`). To load those, Substance 3D Designer must be installed in its standard location. If it isn't, ReEtched lists what it couldn't load in the **Log**, and the material may come out incomplete.

Imported graphs stay in the list, and ReEtched imports them again the next time it starts (as long as the files are still where they were). Layers that already use one are saved with the project.

## Smart materials

A smart material is a saved layer or folder — usually several fill layers with masks — that you can drop onto any model. Its masks use generators such as **Edge wear** and **Cavity dirt**, so the wear and dirt adapt to each model's shape.

### Built-in smart materials

The **Smart materials** category of the **Assets** panel includes:

| Smart material | What it is |
|----------------|------------|
| **Painted Steel Worn** | Red paint over steel, worn away on the edges. |
| **Rusty Iron** | Iron with rust in occluded areas and noisy patches. |
| **Dusty Plastic** | Dark plastic with dust settling on top and in creases. |
| **Gold Grime** | Gold with grime in cavities and occluded areas. |

Double-click a smart material, or drag it onto the model, a texture set or the layer stack, to add it as a folder. Its generators need baked mesh maps; if the texture set has none, ReEtched reminds you to bake them. See [Baking](baking.md).

### Saving your own

1. Select the layer or folder you want to save.
2. Click **Save as smart material…** at the top of the **Layers** panel.
3. Name the file and save it as a `.rsm` file.

The file holds the layer or folder with its masks, effects, paint and the images it uses.

### Importing smart materials

1. Click **Smart materials** in the **Assets** panel.
2. Click **Import smart material…** and choose one or more `.rsm` files.

They're added to the **Smart materials** list, and ReEtched loads them again the next time it starts (as long as the files are still where they were). You can also put `.rsm` files in a library folder; they then appear under **All library files** in the **Assets** panel, and double-clicking one adds it to the stack. See [Asset library](asset-library.md).
