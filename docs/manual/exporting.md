# Exporting

When your textures are done, export them as image files for your game engine or renderer. The same window also exports the mesh together with its textures, and layered Photoshop files.

## Export textures

1. Choose **File → Export textures…** or press **Ctrl+Shift+E**.
2. Set the **Output directory**. Type a path or click **Browse…**. For a saved project, it starts as a `textures` folder next to the project file.
3. Pick an **Output template** (see [Built-in templates](#built-in-templates)).
4. Choose the **File type**, bit depth and **Padding**.
5. Under **TEXTURE SETS**, tick the texture sets to export. Each one shows its resolution.
6. Check the file names under **Files for the first texture set:**.
7. Click **Export**.

The log reports how many textures were written and where.

Textures are exported at each texture set's resolution, set in **Texture set settings**.

## Built-in templates

An output template decides which files are written, what they're called, and which channel goes into which color channel of each file (channel packing).

| Template | Files per texture set |
|---|---|
| **PBR Metallic Roughness** | Base color, metallic, roughness, normal (OpenGL), height, emissive and opacity, one file each. |
| **Unreal Engine (Packed)** | Base color, normal (DirectX), occlusion/roughness/metallic packed into R/G/B, emissive. |
| **Unity HDRP (Metallic Standard)** | Base map, mask map (R metallic, G occlusion, B black, A smoothness), normal. |
| **Unity URP (Metallic Standard)** | Base map, metallic/smoothness map (RGB metallic, A smoothness), normal. |
| **glTF PBR Metal Roughness** | Base color with opacity in alpha, occlusion/roughness/metallic packed into R/G/B, normal, emissive. |
| **Blender (Principled BSDF)** | Base color, metallic, roughness, normal (OpenGL), height. |
| **Godot (ORM)** | Albedo, occlusion/roughness/metallic packed into R/G/B, normal. |

A few notes:

- **Smoothness** is 1 minus roughness.
- **Occlusion** comes from the baked ambient occlusion map. If the texture set has none, it's white. See [Baking mesh maps](baking.md).
- Inside ReEtched, normal maps use the OpenGL convention. Templates that need DirectX normals convert them on export.

## File types and bit depth

| File type | Bit depths |
|---|---|
| **PNG** | 8 bits, 16 bits |
| **TGA** | 8 bits |
| **TIF** | 8 bits, 16 bits, 32 bits float |
| **EXR** | 32 bits float |

8- and 16-bit files store base color and emissive in sRGB. 32-bit float files keep them linear; the window reminds you of this when you pick a float format.

## Padding

Padding fills the empty space around UV islands, so edges don't show seams when the texture is filtered or mipmapped.

| Option | What it does |
|---|---|
| **No padding** | Leaves the space outside UV islands at the default value. |
| **Dilation infinite** | Extends the island edges until the whole texture is filled. This is the default. |
| **Dilation + transparent** | Extends the edges by the number of pixels you set (1–256, default 16). Beyond that the texture is transparent, if the file has an alpha channel. |
| **Dilation + default background** | Extends the edges by the number of pixels you set. Beyond that the texture keeps the default value. |

## File names

Templates build file names from tokens:

| Token | Replaced with |
|---|---|
| `$project` | The project's name. |
| `$mesh` | The mesh's file name, without the extension. |
| `$textureSet` | The texture set's name. |
| `$udim` | The UDIM tile number. |

For example, `$mesh_$textureSet_BaseColor` becomes `Robot_Body_BaseColor.png` for a texture set "Body" on a mesh "Robot". Characters that aren't allowed in file names are replaced with `_`.

### UDIM texture sets

A texture set that spans several UDIM tiles is exported as one file per tile. If the template's file names don't contain `$udim`, the tile number is added at the end, for example `Robot_Body_BaseColor.1001.png`.

## Painter export presets (.spexp)

You can use export presets from Substance 3D Painter, and save ReEtched's templates in the same format.

- **Import Painter preset (.spexp)…** reads a `.spexp` (or `.json`) file. Every preset in the file is added to the **Output template** menu, and the first one is selected. Anything the importer can't map is listed in the log.
- **Save preset as .spexp…** saves the currently selected template as a `.spexp` file.

The export window doesn't have a channel-packing editor. To make your own template, edit a `.spexp` file (in Substance 3D Painter or a text editor) and import it.

Presets can use Painter's channels (base color, metallic, roughness, height, emissive, opacity and the other channels ReEtched has), the converted maps **Normal_OpenGL**, **Normal_DirectX**, **Glossiness** and **Mixed_AO**, and the baked ambient occlusion map.

## Export the mesh with its textures

Under **MESH WITH TEXTURES** you can write the mesh together with textures it references, ready to open in another application.

1. Set the **Output directory** at the top of the window.
2. Pick a format: **USD (.usda)**, **USD binary (.usdc)**, **USDZ package** or **OBJ**.
3. Optional: set **Displacement**. Above 0, the vertices are moved along their normals by the height channel times this value (in world units). Leave it at 0 to export the mesh unchanged.
4. Click **Export mesh**.

The mesh file is named after the original mesh. Its textures are written next to it with the **PBR Metallic Roughness** template, using the **File type**, bit depth and **Padding** chosen above. A **USDZ package** also contains the textures, so you can share a single file.

All texture sets are included in the mesh export, whether or not they're ticked under **TEXTURE SETS**. For UDIM texture sets, the mesh references every tile through a `<UDIM>` file name.

## Export a layered PSD

Under **LAYERED PSD**, click **Export PSD** to write Photoshop files that keep your layer stack: layers, folders, masks, blend modes and opacities.

- One `.psd` file is written per texture set and channel, named `Mesh_TextureSet_Channel.psd`, for example `Robot_Body_BaseColor.psd`.
- Only channels that some layer actually uses are exported. Base color is always exported.
- Only the texture sets ticked under **TEXTURE SETS** are exported.
- For UDIM texture sets, the tile number is added before `.psd`.
- Files are 8 bits per channel. Base color and other color channels are sRGB.

If something in the stack can't be represented in Photoshop, the log says so.

## Scripted export

You can also export from Python with your own settings. See [Python scripting](scripting.md).
