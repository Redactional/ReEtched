# Viewport, rendering and color

This page covers how the 3D view looks: the camera, lighting environment, tone mapping and post effects, the shader each texture set uses, color management with OCIO, and the path tracer.

Most of these options are in the **Display settings** panel. Open it from the icon strip at the side of the window, or from the **Window** menu.

## View modes

The 3D view and the 2D view each have a small toolbar in their top-left corner. The view mode menu there decides what the view shows:

| Mode | Shows |
|---|---|
| **Material** | The fully shaded result. |
| **Base color**, **Roughness**, **Metallic**, **Emissive**, **Opacity** | One channel on its own, unlit. |
| **Normal (mesh)** | The mesh's normals. |
| **UV coordinates** | The UV layout as colors. |
| Other channels | Any extra channels the texture set has, such as **Specular level** or **Coat**. |
| **MESH MAPS** | Any baked mesh map, such as **Ambient occlusion** or **Curvature**. See [Baking mesh maps](baking.md). |

The 3D and 2D views have separate view modes, so you can, for example, see the material in 3D and the roughness in 2D.

## Camera

The 3D view's toolbar also has a **Perspective** / **Orthographic** menu. The same choice is under **CAMERA → Projection** in **Display settings**, together with **Field of view** (10° to 90°) for the perspective camera.

Navigation shortcuts depend on your keymap. See [Shortcuts](shortcuts.md).

## Environment

The environment is the HDR image that lights the model and appears behind it. Until you load one, ReEtched uses a soft built-in sky.

To load an environment, do one of these:

- In **Display settings**, under **ENVIRONMENT**, click **Load environment map…**.
- Choose **File → Load environment…**.
- Drag an `.hdr` or `.exr` file onto the ReEtched window.

Environment settings:

| Setting | What it does |
|---|---|
| **Opacity** | How visible the environment image is behind the model. |
| **Exposure (EV)** | Brightness in stops, from −6 to +6. |
| **Rotation** | Turns the environment around the model, from −180° to 180°. |
| **Blur** | Blurs the background image. |

## Rendering settings

Under **RENDERING** in **Display settings**:

| Setting | What it does |
|---|---|
| **Tone mapping** | How bright HDR values are fitted to your screen: **ACES (filmic)**, **AgX**, **Khronos PBR Neutral** or **Linear (clamped)**. When an OCIO config is active, **OCIO display / view** is also available (see below). |
| **Bloom** | Glow around very bright areas. **Bloom threshold** appears when bloom is above 0 and sets how bright an area must be to glow. |
| **Vignette** | Darkens the corners of the view. |
| **Emissive intensity** | Multiplies the brightness of the emissive channel. |
| **Opacity** | How the opacity channel shows: **Opaque**, **Cutout (alpha test)**, **Dithered** or **Blend (order independent)**. **Cutout** adds a **Cutoff** slider. |
| **Displacement** | Parallax occlusion from the height channel, which makes height detail look raised. 0 turns it off. |
| **Subsurface scattering** | Tick to let light spread under the surface. **radius** sets how far it travels, in scene units. |
| **Renderer** | **Raster** (the default) or **Path tracer**. See below. |
| **Anti-aliasing** | **Progressive** refines the image over a few frames while nothing moves. |
| **Depth of field** | Needs progressive anti-aliasing. **Aperture** sets the lens size (bigger blurs more). **Focus distance** sets where the image is sharp; 0 focuses on the point the camera orbits around. |

Under **VIEWER**:

- **Wireframe (3D view)** draws the mesh's edges over the model.
- **UV wireframe (2D view)** draws the UV edges in the 2D view.

If your graphics card can't draw lines, both are greyed out.

## Path tracer

The path tracer renders physically based light transport: reflections, shadows and light bouncing between surfaces. It's slower than the regular view, so use it to check your work rather than while painting.

1. In **Display settings**, set **Renderer** to **Path tracer**.
2. Set **Bounces**, the number of times light can bounce (1 to 16, default 6).
3. Set **Samples**, the number of samples per pixel after which the image stops refining (default 512). The number next to it shows how many samples are done.
4. Stop moving the camera. The image gets cleaner with every frame.

The path tracer works in the **Material** view mode only. Moving the camera starts the refinement over. It shades every texture set with the metal/roughness model, even if the texture set uses another shader.

## Shaders

Each texture set has its own shader. Set it in the **Texture set settings** panel, in the **Shader** menu:

| Shader | Use it for |
|---|---|
| **PBR Metal/Rough** | The standard metal/roughness workflow used by most game engines. |
| **Adobe Standard Material** | Adds specular level and edge color, anisotropy, transmission, scattering, coat and sheen. |
| **OpenPBR** | The OpenPBR Surface model, with the same extra channels. |

When a shader reads channels the texture set doesn't have yet, a button **Add the N channels this shader reads** appears below the shader menu. Click it to add them. You can also add and remove channels yourself with **Add channel** under **CHANNELS**.

**Normal map format** always shows **OpenGL**: normal maps inside ReEtched use the OpenGL convention. You can still export DirectX normal maps; see [Exporting](exporting.md).

### Painter GLSL shaders

ReEtched can load custom viewport shaders written for Substance 3D Painter (`.glsl` files).

1. Select the texture set in the **Texture set list**.
2. In **Texture set settings**, open the **Shader** menu and click **Painter GLSL shader…**, then pick the file. You can also drag a `.glsl` file onto the window.
3. The shader's settings appear under **SHADER PARAMETERS**. Image parameters have a **Use an image…** button.

While the shader compiles, the panel shows **Compiling…**. If it can't be compiled, the panel shows the error and the 3D view keeps using the PBR shader. After editing the file, click the **Compile again** button next to **SHADER PARAMETERS**.

Custom shaders only change how the 3D view looks. Exported textures are the same whatever shader you use.

## Color management (OCIO)

By default, ReEtched uses its built-in tone mapping. If your pipeline uses OpenColorIO, you can use an OCIO config for the 3D view instead.

Under **COLOR MANAGEMENT** in **Display settings**, open the **OCIO config** menu and choose one of:

- **None (built-in tone mapping)**: OCIO off.
- One of the built-in ACES configs:
  - **cg-config-v2.2.0_aces-v1.3_ocio-v2.4**
  - **cg-config-v4.0.0_aces-v2.0_ocio-v2.5**
  - **cg-config-v5.0.0_aces-v2.1_ocio-v2.6**
- **Load .ocio file…** to use your own config. You can also drag an `.ocio` file onto the window.

When a config is active, **Tone mapping** switches to **OCIO display / view** and these settings appear:

| Setting | What it does |
|---|---|
| **Rendering space** | The config's name for linear Rec.709 / sRGB, which is the space ReEtched renders in. ReEtched picks it automatically; change it only if your config names it differently. |
| **Display** | Your monitor type, as named by the config. |
| **View** | The view transform for that display. |
| **Look** | An optional look from the config. |

With a config active, fill layers can also read bitmaps in any of the config's color spaces. You'll find that option in the fill layer's properties; see [Fills and materials](fills-and-materials.md).

Choose **None (built-in tone mapping)** to turn OCIO off again.

## UDIM tiles

For a texture set that spans several UDIM tiles, the 2D view outlines each tile and labels it with its number (or the name you gave it). **Texture set settings** has a **UDIM TILES** section where you can name each tile and give it its own resolution, or leave it at **Same as the set**. See [Projects and meshes](projects-and-meshes.md).
