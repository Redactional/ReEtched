# Asset library

The asset library gathers textures, brush shapes, environments, brushes, smart materials, smart masks and more from folders on your computer, and shows them in the **Assets** panel. You can search them, tag them, mark favorites and use them with a double-click.

## Library folders

ReEtched always has one library of its own, **My library**. It's where saved brushes and smart masks go by default, and where **Add files to My library…** copies files.

You can add any other folders as libraries — for example a shared studio folder or a texture collection:

1. Open **File > Settings…**.
2. Under **ASSET LIBRARIES**, click **Add library folder…**.
3. Choose the folder.

The folder is added with its own name as the library name. You can change the name in the box next to it. To remove a library, click **×** (**Remove this library (its files stay)**); the files on disk are not touched.

You can also reach these settings with the **Library folders…** button (the gear icon) in the **Assets** panel.

Subfolders are included. Changes are picked up as soon as you make them.

## Asset kinds

ReEtched recognises files by their extension, and for some images by the name of the folder they're in:

| Kind | Files |
|------|-------|
| **Texture** | PNG, JPEG, TGA, TIFF, EXR, BMP, WebP and SVG images. |
| **Alpha** | The same image types, inside a folder named `alpha`, `alphas` or `brushes alphas`. |
| **Environment** | `.hdr` files, and `.exr` files inside a folder named `environment`, `environments`, `hdri`, `hdris` or `env`. |
| **Smart material** | `.rsm` files. |
| **Smart mask** | `.rsmk` files. |
| **Brush** | `.rbrush` brush presets. |
| **Procedural** | Substance Designer graphs (`.sbs`). |
| **Shader** | `.glsl` shaders. |
| **OCIO config** | `.ocio` color configurations. |
| **Project template** | `.rtpl` project templates. |
| **Mesh** | FBX, OBJ, glTF/GLB and USD (`.usd`, `.usda`, `.usdc`, `.usdz`) files. |

Other files are ignored. Asset names are made from the file name, so `rusty_metal-01.png` shows as “Rusty Metal 01”.

## Browsing the library

The row of icons at the top of the **Assets** panel picks what's shown. These ones show library assets:

- **Smart masks**
- **Brushes**
- **Alphas**
- **Textures**
- **Environments**
- **Shaders**
- **All library files** — every asset of every kind, including smart materials, meshes, project templates and OCIO configs.

(The first three icons — **Materials**, **Smart materials** and **Procedural materials (.sbs)** — show ReEtched's built-in materials. See [Fills, images and materials](fills-and-materials.md).)

Textures, alphas and environments show a picture of the file. Other kinds show an icon. Hover over an asset to see its full name, kind, tags and where the file is.

If ReEtched can't watch a library folder for changes, it checks it every few seconds instead and says so above the assets.

## Searching

Type in the **Search** box at the top of the **Assets** panel. Every word must match the asset's name or one of its tags; the best matches come first.

You can narrow the search further:

| Type | To find |
|------|---------|
| `tag:metal` | Only assets with a tag containing “metal”. |
| `kind:alpha` | Only assets of one kind, such as `texture`, `alpha`, `environment` (or `hdri`), `smartmaterial`, `smartmask`, `brush`, `procedural` (or `sbs`), `shader`, `ocio`, `template` or `mesh`. |
| `"rusty metal"` | The words as one phrase. |

For example, `scratch kind:alpha tag:edge` finds alphas named like “scratch” that have an “edge” tag.

## Favorites and tags

Right-click an asset for these options:

| Option | What it does |
|--------|--------------|
| **Add to favorites** / **Remove from favorites** | Marks the asset with a star. |
| **Edit tags…** | Opens a box where you type tags separated by commas. Click **Save** or press **Enter**. |
| **Show in folder** | Opens the folder that holds the file. |
| **Delete file…** | Deletes the file from disk after you confirm. This can't be undone. |

Click the star button (**Favorites only**) at the top of the panel to show only your favorites.

For library folders you can write to, tags and favorites are saved in a file inside the library folder, so they go with the folder if you share or move it.

## Adding files

Click **+** (**Add files to My library…**) and choose files. They're copied into **My library**, sorted into a subfolder for their kind.

Click the folder icon (**Open My library's folder**) to open **My library** on disk. Anything you copy there yourself appears in the panel too.

## Using assets

Double-click an asset to use it:

| Kind | What double-clicking does |
|------|---------------------------|
| **Texture** | Puts the image in the selected fill layer, in the channel chosen in the **Layers** panel's channel list. If a mask is selected, it becomes the mask's bitmap. Select a fill layer or a mask first. |
| **Alpha** | Makes the image the brush shape. See [Painting](painting.md#brush-shapes). |
| **Environment** | Loads it as the lighting environment. See [Viewport and color](viewport-and-color.md). |
| **Smart material** | Adds it to the layer stack. |
| **Smart mask** | Puts it on the selected layer, replacing any mask it had. |
| **Brush** | Loads the brush preset, including its brush shape. |
| **Procedural** | Imports the graph into **Procedural materials (.sbs)**. |
| **Shader** | Loads the shader. |
| **OCIO config** | Loads the color configuration. |
| **Project template** | Opens the new project window with that template chosen. |
| **Mesh** | Opens the new project window with that mesh filled in. See [Projects and meshes](projects-and-meshes.md). |

Library assets are used by double-clicking. The built-in materials, procedural materials and smart materials can also be dragged onto the model, a texture set or the layer stack; see [Fills, images and materials](fills-and-materials.md).

## Saving into the library

### Brushes

1. Set up the brush the way you want it. See [Painting](painting.md).
2. In the **Assets** panel, click **Brushes**.
3. Click **Save brush…**, name the file and save it.

The brush is saved as a `.rbrush` file in **My library**, with its image or text shape inside the file.

### Smart masks

Select a mask, click **Save as smart mask…** in its properties and save. The `.rsmk` file goes to **My library** unless you choose another place. See [Layers and masks](layers-and-masks.md#smart-masks).

### Smart materials

Select a layer or folder, click **Save as smart material…** at the top of the **Layers** panel and save the `.rsm` file into one of your library folders, for example **My library** (use **Open My library's folder** to find it). It then shows under **All library files**. See [Fills, images and materials](fills-and-materials.md#smart-materials).

## Live updates

ReEtched watches your library folders. When you add, change, rename or delete files in them — in ReEtched, in your file manager or in another program — the **Assets** panel updates on its own, and thumbnails are redrawn for changed images.
