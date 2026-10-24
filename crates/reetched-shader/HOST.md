# reetched-shader host interface

`reetched_shader::translate(source, host_header)` compiles a Substance 3D Painter
custom shader into one GLSL 450 fragment shader and hands it to naga's GLSL front
end. The **host header** is ReEtched's part of that file: it declares the
varyings, the fragment output, bind groups 0 and 1, and the `rx_*` functions
listed here. The Painter library in this crate is written only against these
functions. A test stub lives in `tests/host_stub.glsl`.

The final source is laid out like this:

```
#version 450                       (added by the crate; the header must not have one)
<host header>
lib-sparse (types, outputs, sampling; always included)
generated prelude (set 2: parameter block, user textures, globals)
the Painter libraries the shader imports, plus their dependencies
the user's shader (annotations removed, line count unchanged)
generated void main()
```

Every identifier starting with `rx_` is reserved for the host and the crate.

## Varyings (the host header declares these, with exactly these names)

```glsl
layout(location = 0) in vec3 rx_in_position;  // world-space position
layout(location = 1) in vec3 rx_in_normal;    // world-space normal (need not be normalized)
layout(location = 2) in vec2 rx_in_uv;        // UV set 0, as stored in the mesh
layout(location = 3) in vec4 rx_in_tangent;   // world tangent xyz, w = bitangent sign (MikkTSpace)
layout(location = 4) in vec2 rx_in_uv2;       // UV set 1
```

The header also declares the fragment `out` variable(s) and writes them in
`rx_finish`. Meshes are drawn untransformed: model space = world space, so
Painter's `mvp_matrix` is `rx_view_proj_matrix()`.

## Bind groups

| Set | Owner | Contents |
|---|---|---|
| 0 | host | globals, environment (layout is the host's business) |
| 1 | host | texture set channels (layout is the host's business) |
| 2 | shader | `binding 0`: std140 parameter block `RxParams` (only if `Shader::uniform_size > 0`), `binding 1..=8`: one `texture2D` per `Shader::textures[k]` (`binding` field), `binding 9`: the shared `sampler` for them (only if there are textures) |

Fill set 2 binding 0 with `Shader::default_uniform_data()` and patch values at
`Param::offset`. Floats are `f32`, ints `i32`, **bools are `i32` 0/1**, vec3 is 12
bytes at a 16-byte aligned offset (std140).

## Functions the host header must define

All are called from fragment code, also inside non-uniform control flow (naga
accepted all of the library and test shaders with `ValidationFlags::all()`).

### Channels and mesh maps

```glsl
vec4 rx_channel(int id, vec2 uv, vec2 ddx_uv, vec2 ddy_uv);
bool rx_channel_present(int id);
vec2 rx_channel_size(int id);   // texture size in texels, (0, 0) when absent
```

`rx_channel` samples channel `id` at `uv` with explicit gradients (use
`textureGrad`); it must return **`vec4(0.0)` for an id the current texture set
does not have**. It is never called with a negative id.

Ids (also `reetched_shader::channel::*`):

| id | ReEtched channel | Painter tag(s) mapped to it |
|---|---|---|
| 0 | BaseColor | `channel_basecolor` |
| 1 | Metallic | `channel_metallic` |
| 2 | Roughness | `channel_roughness` |
| 3 | Normal | `channel_normal` |
| 4 | Height | `channel_height` |
| 5 | Emissive | `channel_emissive` |
| 6 | Opacity | `channel_opacity` |
| 7 | SpecularLevel | `channel_specularlevel` |
| 8 | SpecularColor (edge color) | `channel_specularedgecolor` (*) |
| 9 | Anisotropy level | `channel_anisotropylevel` |
| 10 | AnisotropyAngle | `channel_anisotropyangle` |
| 11 | Transmission | `channel_transmissive` |
| 12 | Scattering | `channel_scattering` |
| 13 | ScatteringColor | `channel_scatteringcolor` (*) |
| 14 | Coat | `channel_coatopacity`, `channel_coat` (*) |
| 15 | CoatColor | `channel_coatcolor` (*) |
| 16 | CoatRoughness | `channel_coatroughness` (*) |
| 17 | Sheen | `channel_sheenopacity`, `channel_sheen` (*) |
| 18 | SheenColor | `channel_sheencolor` (*) |
| 19 | SheenRoughness | `channel_sheenroughness` (*) |
| 20 | AmbientOcclusion | `channel_ambientocclusion`, `channel_ao` |
| 32 | baked mesh AO | `texture_ambientocclusion`, `texture_ao` |
| 33 | baked curvature | `texture_curvature` |
| 34 | baked thickness | `texture_thickness` |
| 35 | baked tangent-space normal | `texture_normal` |
| 36 | baked world-space normal | `texture_normal_ws` |
| 37 | baked position | `texture_position` |
| 38 | baked ID | `texture_id` |

(*) Painter's shader docs only list the older channel tags; these names follow
Painter's channel naming but are unverified. Every other `channel_*` /
`texture_*` tag (`user0..7`, `diffuse`, `specular`, `glossiness`, `displacement`,
`ior`, `reflection`, `blendingmask`, ...) is "absent": the SamplerSparse is `-1`,
reads `vec4(0)`, `is_set` is false, and the shader gets a warning. A mesh map
the host doesn't have is simply reported absent by `rx_channel_present`.

**Value encoding** (what `rx_channel` returns when the channel exists). Painter
stores channels premultiplied by coverage, and its getters fill in a default
where coverage is missing. ReEtched has full coverage everywhere, so:

| Kind | Ids | Return |
|---|---|---|
| color | 0, 5, 8, 13, 15, 18 | `vec4(linear_rgb, 1.0)` (sRGB-stored channels decoded to linear) |
| grayscale | 1, 2, 6, 7, 9, 11, 12, 14, 16, 17, 19, 20 | `vec4(value, 1.0, 0.0, 1.0)`: value in R, coverage in G |
| anisotropy angle | 10 | `vec4(turns, 1.0, 0.0, 1.0)`: angle as a fraction of a full turn in [0, 1) (`getAnisotropyAngle` multiplies by 2π) |
| height | 4 | `vec4(h, 1.0, 0.0, 1.0)`: signed, **0 = neutral**, about [-1, 1] like Painter's height range (convert if ReEtched stores 0.5-centred) |
| normal | 3 | `vec4(n * 0.5 + 0.5, 1.0)`: the painted tangent-space normal **detail only** (not including the baked mesh normal), OpenGL convention (+Y up) |
| mesh AO / curvature / thickness | 32, 33, 34 | `vec4(v, v, v, 1.0)` (A = validity; curvature 0.5 = flat) |
| mesh normals / position / id | 35, 36, 37, 38 | `vec4(rgb_0_to_1, 1.0)` as baked (tangent normal OpenGL convention) |

A **missing** channel returns `vec4(0)`; Painter's getters then return its
documented defaults: base color 0.5 grey, roughness 0.3, metallic 0, specular
level 0.5, opacity 1, AO 1, height 0, anisotropy 0, scattering 0, specular color
0.04, normal flat.

### Environment (image-based lighting only)

```glsl
vec3  rx_env_sample(vec3 dir, float lod);        // mip `lod` of the env pyramid along world dir
vec3  rx_env_specular(vec3 dir, float roughness); // GGX-prefiltered radiance for perceptual roughness
vec3  rx_env_irradiance(vec3 normal);            // diffuse irradiance / pi (Lambert-ready), from SH
float rx_env_max_lod();                          // highest mip index of the env pyramid
float rx_env_rotation();                         // Painter convention: [0, 1) turns around +Y
float rx_env_exposure();                         // linear multiplier
```

All three lookups take **world-space** directions and must already apply the
environment rotation and exposure. `rx_env_irradiance(n) * albedo` is the full
diffuse term (Painter: `envIrradiance`). `rx_env_specular` is the split-sum
prefiltered term: `lib-pbr`'s `pbrComputeSpecular` multiplies it by an analytic
pre-integrated BRDF. `rx_env_sample` backs Painter's `envSampleLOD`, used by
Monte-Carlo code (`pbrComputeSpecularAnisotropic` and user shaders) whose mip
choice assumes a plain box-filtered pyramid whose base width is
`2^rx_env_max_lod()`; with a roughness-prefiltered pyramid such code just looks
somewhat blurrier.

### Camera, screen, misc

```glsl
vec3  rx_eye_position();          // world
vec3  rx_camera_direction();      // world, the direction the camera looks
mat4  rx_view_matrix();           // world -> camera, rigid
mat4  rx_view_proj_matrix();      // projection * view
mat4  rx_inv_view_proj_matrix();
bool  rx_is_perspective();
float rx_fovy();                  // vertical field of view, radians
vec4  rx_screen_size();           // (width, height, 1/width, 1/height) in pixels
bool  rx_is_2d_view();            // rendering the 2D UV view
vec4  rx_main_light();            // xyz: world direction toward the dominant light, w: intensity
float rx_scene_radius();          // bounding-sphere radius of the mesh, scene units (SSS scale)
float rx_time();                  // seconds since start
```

### Output

```glsl
void rx_finish(vec3 albedo, vec3 diffuse, vec3 specular, vec3 emissive, float alpha,
               vec4 sss_coefficients, vec3 sss_color);
```

Called once at the end of `main()` unless the shader discarded. Defaults when the
shader didn't call the matching output: albedo 1, diffuse 0, specular 0,
emissive 0, alpha 1, sss coefficients 0, sss color 1. Painter's composition is
`emissive + albedo * diffuse + specular` (in linear space). `sss_coefficients`
is `lib-sss`'s packing: rgb = per-channel scattering radius in scene units, w = 1
when SSS applies; there is no SSS pass in ReEtched, so the host may ignore it
or fake it. `alpha` matters when `Shader::states` contains `blend` (`over`,
`over_premult`, `add`, `multiply`); `cull_face off` asks for two-sided drawing.
If `Shader::disable_srgb_conversion` is set, the shader already outputs sRGB.

No shadows: `getShadowFactor()` is always 1.

## What the crate does with Painter's syntax

- `import lib-*.glsl` lines are removed; the named library and its dependencies
  are included. Unknown libraries are an error. `lib-sparse` (SamplerSparse,
  SparseCoord, V2F, outputs, `textureSparse`) is always present, as in Painter.
- `//: param auto <id>` on a `SamplerSparse` -> a private `int` global holding the
  channel id (`id1 or id2` picks the first present at runtime). On other types ->
  a private global assigned from the host functions in `main()` (table in
  `engine_value` in `src/lib.rs`). Unsupported ids (`environment_irrad_mat_*`,
  `uvtile_*`, layering ids, ...) are warned about and read as zero.
  `<tag>_size` gives `(w, h, 1/w, 1/h)` of that channel.
- `//: param custom {json}` on bool/int/ivecN/float/vecN -> a member of the
  `RxParams` block plus a private global copied in `main()` (bools via `!= 0`).
  A multi-line JSON (continued `//:` lines) is fine. Unannotated plain uniforms
  become custom params defaulting to 0 (warning).
- `//: param custom` on `uniform sampler2D x` -> `texture2D rx_tex_x` at set 2
  binding 1+k and `#define x sampler2D(rx_tex_x, rx_user_sampler)`.
  `//: param auto` sampler2D engine textures (`texture_environment`,
  `texture_blue_noise`, `shadow_mask`) are an error: use lib-env / lib-random.
- `//: state` lines are collected into `Shader::states`; `//: metadata` is
  ignored with a warning.
- The user re-declaring a library uniform (e.g. `ao_intensity`) with the same
  type replaces the library's.
- In user code: `x.is_set` -> `rx_is_set(x)`, `x.size` -> `rx_sparse_size(x)`
  (for names known to be SamplerSparse), the identifier `sampler` ->
  `rx_sampler`, and `texture2D(` -> `texture(`.
- Errors come back as `Diagnostic`s with `Location::User { line }` (line in the
  user's file), `Library { module, line }`, `HostHeader { line }` or `Generated`.

## Painter libraries

Implemented from the public Shader API documentation (names, signatures,
documented behaviour), with our own code:

- **lib-sparse**: `SamplerSparse`, `SparseCoord`, `getSparseCoord`,
  `getSparseCoordLod0`, `textureSparse`, `textureSparseQueryLod`,
  `textureSparseQueryGrad`, `textureSparseOffsets` (1..4 offsets).
- **lib-defines**: `M_PI`, `M_2PI`, `M_INV_PI`, `M_INV_LOG2`, `M_GOLDEN_RATIO`, all `BlendingMode_*`.
- **lib-sampler**: every `get*` getter in both forms (vec4 / sampler+coord),
  `getAO`, `getShadowFactor` (= 1), `generateAnisotropicRoughness`,
  `generateDiffuseColor`, `generateSpecularColor` (both), `DEFAULT_*`, `ao_intensity`.
- **lib-normal**: `normalBlend` (whiteout), `normalBlendOriented` (RNM),
  `normalFade`, `normalUnpack` (both), `normalFromHeight`, `getTSNormal` (both),
  `computeWSBaseNormal`, `computeWSNormal`, `HEIGHT_FACTOR`.
- **lib-vectors**: `LocalVectors`, `isBackFace`, `getEyeVec`,
  `tangentSpaceToWorldSpace`, `worldSpaceToTangentSpace`, `computeLocalFrame` (both).
- **lib-env**: `envSampleLOD`, `envIrradiance`.
- **lib-pbr**: `normal_distrib`, `fresnel`, `G1`, `visibility`,
  `cook_torrance_contrib`, `importanceSampleGGX`, `probabilityGGX`,
  `distortion`, `computeLOD`, `horizonFading`, `pbrComputeDiffuse`,
  `pbrComputeSpecular` (split-sum, see above), `nbSamples`, `horizonFade`.
- **lib-pbr-aniso**: anisotropic `normal_distrib`, `G1`, `visibility`,
  `cook_torrance_contrib`, `importanceSampleGGX`, `probabilityGGX`,
  `pbrComputeSpecularAnisotropic` (Monte Carlo, `nbSamples` samples).
- **lib-emissive**: `pbrComputeEmissive`, `emissive_tex`, `emissive_intensity`.
- **lib-alpha**: `alphaKill(float)`, `alphaKill(SparseCoord)`, `alpha_threshold`, `alpha_dither`.
- **lib-pom**: `getParallaxOffset`, `applyParallaxOffset` (displacement channel,
  so inactive until ReEtched has one, as in Painter without it).
- **lib-sss**: `getSSSCoefficients` (both).
- **lib-random**: `getBlueNoiseThreshold` (interleaved gradient noise instead of
  the blue-noise texture), `getBlueNoiseThresholdTemporal`, `fibonacci1D`,
  `fibonacci2D`, `fibonacci2DDitheredTemporal`.
- **lib-bayer**: `bayerMatrix8`.
- **lib-utils**: `tonemapSCurve`, `sRGB2linear`, `linear2sRGB`,
  `linear2sRGBOpt`, `convertOutput` (all overloads), `getDitherThreshold`,
  `RGB2Gray`, `specularOcclusionCorrection`.
- Surface shader outputs: `albedoOutput`, `diffuseShadingOutput`,
  `specularShadingOutput`, `emissiveColorOutput`, `alphaOutput`,
  `sssCoefficientsOutput`, `sssColorOutput`.

Not implemented: `lib-bent-normal` (not in Painter's public docs),
`normalFromBaseNormal` / `normalFromNormal` (documented without signatures),
the deprecated vec2/sampler2D-based getters, layering
(`layering_declare_stacks` / `layering_bind_materials`), vertex colors (color0 is
white), UV sets beyond the second (uv2..uv7 repeat uv0), the `.tex` member of
SamplerSparse.

## naga GLSL front-end limits found (naga 30.0.1)

Works: anonymous and named uniform blocks, function overloading (also on array
size), `inout`/`out` struct and array parameters, global `const` arrays,
int -> float conversions in calls and initialisers, function-like macros,
`discard` inside helper functions, `textureGrad`/`texture` on
`sampler2D(texture2D, sampler)`, sampling and `dFdx` in non-uniform control flow
(passes the validator with all flags).

Doesn't work, worked around:

- Combined image samplers (`uniform sampler2D x;`) -> split texture + sampler plus a `#define`.
- **`sampler2D` as a function parameter type** -> not supported; user functions
  taking a texture param fail to compile (not worked around).
- `sampler` is a type keyword -> renamed in user code.
- Assigning a swizzle of a matrix column (`m[0].w = x`) -> build matrices from columns.
- Struct-typed uniforms (SamplerSparse) -> lowered to `int` ids.
- `bool` is not host-shareable in a uniform block -> stored as `int`.
- `#version` must be first -> added by the crate; the user's is dropped.
