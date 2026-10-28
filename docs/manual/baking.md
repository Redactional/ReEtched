# Baking mesh maps

Mesh maps are textures that describe the shape of your model: its fine surface detail, which areas are hidden in crevices, where the edges are, and so on. Generators and smart materials read them to place wear, dirt and other effects automatically, so bake them before you start texturing.

## Open the baker

You can open the **Bake mesh maps** window in two ways:

- **File → Bake mesh maps…**
- In the **Texture set settings** panel, click **Bake…** next to the **MESH MAPS** heading.

## Bake your maps

1. Optional: next to **High definition mesh**, click **Browse…** and pick a detailed version of your model. Leave the field empty to bake from the model you're painting.
2. Choose an **Output size**.
3. Adjust the other settings if needed (see below).
4. Under **BAKERS**, switch on the maps you want. All of them are on by default.
5. Under **TEXTURE SETS**, tick the texture sets to bake.
6. Click **Bake selected textures**.

A progress bar shows which texture set is baking. Click **Cancel** to stop. When the bake finishes, the log reports how many maps were baked and the maps appear in the **MESH MAPS** list in **Texture set settings**.

Baked maps are stored in your project as 16-bit PNG images. Their edges are extended past the UV islands automatically, so you won't see seams from texture filtering.

## High-poly and low-poly meshes

If you set a **High definition mesh**, ReEtched projects the detail of that mesh onto the UVs of the model you're painting (the low-poly mesh). The high-poly mesh doesn't need UVs. It can be in any mesh format ReEtched opens (FBX, OBJ, glTF/GLB, USD).

Without a high-poly mesh, the maps are baked from your model itself. That's enough for ambient occlusion, curvature, position, thickness and world space normals. The **Normal** map then only shows the model's own smooth shading, and **Height** stays flat.

Rays are cast from a "cage" around the low-poly surface. **Max frontal distance** and **Max rear distance** set how far outside and inside the low-poly surface the baker looks for the high-poly mesh. If parts of the high-poly detail are missing from the result, increase these. If detail from neighbouring parts leaks in, lower them.

## Settings

| Setting | What it does |
|---|---|
| **High definition mesh** | Detailed mesh to bake from. Empty: bake from the low-poly mesh. |
| **Output size** | Resolution of the baked maps, up to 4096 × 4096. Default 2048 × 2048. |
| **Antialiasing** | **None**, **Subsampling 2×2** or **Subsampling 3×3**. Takes several samples per pixel for smoother edges; slower. |
| **Max frontal distance** | How far outside the low-poly mesh to look for the high-poly mesh, as a percentage of the mesh size. Default 1%. |
| **Max rear distance** | How far inside the low-poly mesh to look, as a percentage of the mesh size. Default 1%. |
| **AO rays** | Rays per sample for ambient occlusion. More rays give less noise. Default 32. |
| **AO / thickness distance** | How far ambient occlusion and thickness rays travel, as a percentage of the mesh size. Default 25%. |
| **Thickness rays** | Rays per sample for thickness. Default 16. |

"Mesh size" means the diagonal of the low-poly mesh's bounding box, so the same settings work for small and large models.

## Map types

| Map | What it contains |
|---|---|
| **Normal** | Tangent-space normal map (OpenGL convention) carrying the high-poly detail. It's used as the starting normal of the layer stack, so the detail shows in the 3D view and in exported normal maps. |
| **World space normal** | The surface direction in world space. |
| **ID** | A flat color for each material of the mesh you bake from. Used by the **Color selection** mask effect. |
| **Ambient occlusion** | How hidden each point is from its surroundings. Dark in crevices, white in the open. |
| **Curvature** | Mid-grey on flat areas, brighter on convex edges, darker in concave creases. |
| **Position** | Each point's position inside the model's bounding box. |
| **Thickness** | How thick the model is behind each point. Dark in thin areas. |
| **Height** | Distance between the low-poly and high-poly surfaces. Mid-grey means no difference. |
| **Opacity** | White where the baker found the high-poly mesh, black where it didn't. Useful to check your cage distances. |

## UDIM meshes

If a texture set uses several UDIM tiles, every tile is baked. In **Texture set settings**, a tile selector appears above the mesh map list so you can see and manage the maps of each tile.

## Manage mesh maps

The **MESH MAPS** list in **Texture set settings** shows every map type for the selected texture set:

- A baked or imported map shows its file name and a **Clear** button.
- An empty slot shows **—** and a **Load…** button. Click it to use any image (PNG, TGA, TIFF, EXR or JPG) for that map.

## Import maps baked elsewhere

If you baked maps in another application, you can import them all at once by name:

1. Name each file `TextureSetName_suffix`, using the suffixes below. Upper and lower case don't matter.
2. In **Texture set settings**, click **Import Painter maps…**.
3. Select all the files.

Each file goes to the texture set and map its name matches. Files that don't match are listed in the log. The log also reports how many maps were imported.

| Map | File name suffix | Example |
|---|---|---|
| Normal | `normal_base` | `Body_normal_base.png` |
| World space normal | `world_space_normals` | `Body_world_space_normals.png` |
| ID | `id` | `Body_id.png` |
| Ambient occlusion | `ambient_occlusion` | `Body_ambient_occlusion.png` |
| Curvature | `curvature` | `Body_curvature.png` |
| Position | `position` | `Body_position.png` |
| Thickness | `thickness` | `Body_thickness.png` |
| Height | `height` | `Body_height.png` |
| Opacity | `opacity` | `Body_opacity.png` |

These are the names Substance 3D Painter uses when it exports baked maps, so maps exported from Painter import without renaming.

Maps imported this way go to the texture set's first UDIM tile. For other tiles, use **Load…** with the tile selected.

## View mesh maps

The 3D and 2D views each have a view mode menu in their top-left corner. Below the channel views, under **MESH MAPS**, pick any map type to display it on the model or in UV space. Pick **Material** to return to the normal view. See [Viewport, rendering and color](viewport-and-color.md).

## How mesh maps are used

- **Generators.** In a mask's **Add effect** menu, the generators read the baked maps:

  | Generator | Reads |
  |---|---|
  | **Edge wear** | Curvature |
  | **Cavity dirt** | Curvature |
  | **Ambient occlusion dirt** | Ambient occlusion |
  | **Position gradient** | Position |
  | **Thin areas** | Thickness |
  | **Noise**, **Light** | No mesh map needed |

- **Smart materials** use generators in their masks. If you add a smart material to a texture set without mesh maps, the log reminds you to bake them.
- **Color selection** masks pick areas by the colors of the **ID** map. Click the model to pick a color.
- **Normal** is the base normal of the layer stack.
- **Ambient occlusion** is used by export presets that include an occlusion map, such as the packed ORM textures. Without a baked AO map, those exports use white. See [Exporting](exporting.md).

More about masks and generators: [Layers and masks](layers-and-masks.md) and [Fills and materials](fills-and-materials.md).
