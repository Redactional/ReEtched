# Projects and meshes

A ReEtched project is your mesh plus everything you've painted on it. This page covers which meshes you can bring in, how they're split into texture sets, project templates and resolution, and how saving, autosave and crash recovery work.

## Supported mesh formats

| Format | File extensions |
|---|---|
| FBX | `.fbx` |
| OBJ | `.obj` |
| glTF | `.gltf`, `.glb` |
| USD | `.usd`, `.usda`, `.usdc`, `.usdz` |

To start a project from a mesh, use **File → New project…** (see below), or drop the mesh file onto the ReEtched window.

## Texture sets

ReEtched creates one **texture set** for each material in your mesh, named after the material. Each texture set has its own layer stack, resolution, channels and exported textures, the same as in Substance 3D Painter.

The **TEXTURE SET LIST** panel shows all of them. Click one to work on it. Plan your materials in your 3D app with this in mind: if you want two parts of the model to get separate texture files, give them separate materials before you export the mesh.

## Meshes without UVs

Painting needs UVs. If the mesh you open has none, ReEtched unwraps it automatically, and the **LOG** says *had no UVs; generated them*. Automatic UVs work for painting, but if you'll take the textures to another app, unwrap the mesh yourself first so the textures line up with the mesh there.

## UDIM tiles

If a texture set's UVs go beyond the 0–1 square, ReEtched treats it as a UDIM texture set. Each tile the UVs touch becomes a tile, numbered from 1001 in the usual UDIM way. There's nothing to turn on.

UDIM texture sets get a **UDIM TILES** section in **TEXTURE SET SETTINGS**. For each tile you can:

- type a **name**, which shows next to the tile number in the 2D view
- pick a resolution for that tile, or keep **Same as the set**

## Creating a project

1. Choose **File → New project…** (**Ctrl+N**), or click **New project…** on the welcome screen.
2. Pick a **Template**.
3. Next to **Mesh**, click **Browse…** and choose your mesh, or type its path.
4. Pick a **Document resolution**.
5. Click **OK**.

Every texture set in the new project starts with one empty paint layer called **Layer 1**.

## Project templates

A template sets up every texture set of a new project: its resolution, its shader and channels, and which export preset the export window starts with.

### Built-in templates

| Template | Shader | Export preset | Resolution |
|---|---|---|---|
| **PBR Metal/Rough** | PBR Metal/Rough | PBR Metallic Roughness | 2048 |
| **Adobe Standard Material** | Adobe Standard Material | PBR Metallic Roughness | 2048 |
| **OpenPBR** | OpenPBR | PBR Metallic Roughness | 2048 |
| **Unreal Engine** | PBR Metal/Rough | Unreal Engine (Packed) | 2048 |
| **Unity HDRP** | PBR Metal/Rough | Unity HDRP (Metallic Standard) | 2048 |
| **Mobile (1K)** | PBR Metal/Rough | glTF PBR Metal Roughness | 1024 |

The **Adobe Standard Material** and **OpenPBR** templates also add the extra channels those shaders read.

### Saving your own template

Once a texture set is set up the way you like (resolution, shader, extra channels, and an export preset picked in the export window):

1. Select that texture set in the **TEXTURE SET LIST**.
2. Choose **File → Save texture set as template…**.
3. Keep the folder the save dialog opens in, give the template a name, and save.

Templates saved in that folder show up in the **Template** list of the New project window, after the built-in ones. Templates are `.rtpl` files.

If the texture set uses a custom shader, the template saves **PBR Metal/Rough** instead.

You can also keep templates in an asset library folder. Opening one from the **ASSETS** panel starts a new project with that template selected. See [Asset library](asset-library.md).

## Resolution

Resolution is set per texture set. The choices are 256, 512, 1024, 2048, 4096 and 8192 pixels square.

To change it after you've created the project, select the texture set and change **Size** in **TEXTURE SET SETTINGS**. High resolutions use a lot of video memory: an 8192 × 8192 texture set uses about 1.5 GB.

## Opening and saving projects

ReEtched projects are `.retx` files. A project file holds the mesh, the layers and every image the layers use, so you can move or share a single file.

| To | Do this |
|---|---|
| Open a project | **File → Open project…** (**Ctrl+O**), or drop the `.retx` file onto the window |
| Open a recent project | **File → Open recent**, or click it on the welcome screen |
| Save | **File → Save** (**Ctrl+S**). The first time, ReEtched asks where |
| Save a copy under a new name | **File → Save as…** (**Ctrl+Shift+S**) |

**Open recent** keeps your last 10 projects.

If the project has unsaved changes when you quit, start a new project or open another one, ReEtched asks **Save changes to *name*?**. Click **Save** to save first, **Don't save** to throw the changes away, or **Cancel** to go back to your project.

### Save and compact

When you delete layers or replace images, the images they used stay in the project file. Over time this makes the file bigger. **File → Save and compact** drops images and files no layer uses any more, then saves. The **LOG** says how many it dropped.

## Reimport mesh

If you change your mesh in your 3D app after you've started painting, you can bring the new version in without losing your work.

1. Export the changed mesh from your 3D app.
2. In ReEtched, choose **File → Reimport mesh…**.
3. Choose the new mesh file.

ReEtched matches texture sets by material name and moves your paint onto the new UVs. Then:

- A material that's new in the mesh gets a new texture set with an empty **Layer 1**.
- A texture set whose material is gone from the mesh is dropped, with its layers. The **LOG** warns you about it.
- Baked mesh maps are cleared, because they no longer match the mesh. Bake them again. See [Baking mesh maps](baking.md).

Paint is copied from the nearest point on the old surface, so it works best when the mesh's shape changes only a little.

## Autosave and crash recovery

ReEtched autosaves your work in the background every 5 minutes, and only when something has changed. To change how often, or to turn autosave off, use **Autosave every** in **File → Settings…** (0 turns it off).

Autosaves are kept apart from your project file. They never overwrite your `.retx`. When you quit ReEtched normally, the autosave is deleted.

If ReEtched closes unexpectedly, the next time you start it you'll see **RECOVER WORK**, saying which project the autosave belongs to and how old it is:

- Click **Recover** to open the autosave. If the project had been saved before, **Save** writes back to that same file.
- Click **Discard** to throw the autosave away.

Either way, the autosave is deleted afterwards. If you recover, save your project straight away.
