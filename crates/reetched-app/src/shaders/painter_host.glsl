// ReEtched's side of a Painter custom shader (the contract is
// crates/reetched-shader/HOST.md): varyings, bind groups 0-1 as the viewport
// lays them out (viewport.rs / viewport.wgsl), and the rx_* functions.

layout(location = 0) in vec3 rx_in_position;
layout(location = 1) in vec3 rx_in_normal;
layout(location = 2) in vec2 rx_in_uv;
layout(location = 3) in vec4 rx_in_tangent;
layout(location = 4) in vec2 rx_in_uv2;

layout(location = 0) out vec4 rx_out_color;
layout(location = 1) out vec4 rx_out_sss;

// Same layout as `Globals` in viewport.rs.
layout(set = 0, binding = 0, std140) uniform RxGlobals {
  mat4 view_proj;
  mat4 inv_view_proj;
  vec4 eye;
  vec4 params;    // exposure, env rotation (rad), env max lod, background lod
  vec4 view;
  vec4 uv_xform;
  vec4 sh[9];
  vec4 post;      // tone map, bloom, vignette, emissive intensity
  vec4 extra;     // alpha mode, cutoff, parallax, frame
  vec4 accum;
  vec4 sss;       // radius, px per unit, on
  vec4 lut;
  vec4 camera;    // forward xyz, vertical fov (rad)
  vec4 screen;    // width, height, 1/width, 1/height
  vec4 misc;      // time (s), scene radius, perspective
} rx_g;
layout(set = 0, binding = 1) uniform texture2D rx_env_tex;
layout(set = 0, binding = 2) uniform sampler rx_env_smp;

layout(set = 1, binding = 0) uniform texture2D rx_t0;
layout(set = 1, binding = 1) uniform texture2D rx_t1;
layout(set = 1, binding = 2) uniform texture2D rx_t2;
layout(set = 1, binding = 3) uniform sampler rx_set_smp;
layout(set = 1, binding = 4) uniform texture2D rx_ao;
layout(set = 1, binding = 6) uniform texture2D rx_t3;
layout(set = 1, binding = 7) uniform texture2D rx_b0;
layout(set = 1, binding = 8) uniform texture2D rx_b1;
layout(set = 1, binding = 9) uniform texture2D rx_b2;
layout(set = 1, binding = 10) uniform texture2D rx_c0;
layout(set = 1, binding = 11) uniform texture2D rx_c1;
layout(set = 1, binding = 12) uniform texture2D rx_c2;
// Same layout as `MaterialUniform` in viewport.rs.
layout(set = 1, binding = 13, std140) uniform RxMaterial {
  vec4 tile;      // xy: UV origin of the UDIM tile
  vec4 model;
  uvec4 present;  // channel bits: x ids 0-31, y ids 32-63
} rx_m;

const float RX_PI = 3.14159265;

bool rx_channel_present(int id) {
  if (id < 0 || id > 63) return false;
  uint bits = id < 32 ? rx_m.present.x : rx_m.present.y;
  return (bits & (1u << uint(id & 31))) != 0u;
}

vec2 rx_channel_size(int id) {
  if (!rx_channel_present(id)) return vec2(0.0);
  return vec2(textureSize(sampler2D(rx_t0, rx_set_smp), 0));
}

#define RX_S(tex) textureGrad(sampler2D(tex, rx_set_smp), t, dx, dy)

vec4 rx_channel(int id, vec2 uv, vec2 ddx_uv, vec2 ddy_uv) {
  if (!rx_channel_present(id)) return vec4(0.0);
  // Texture row 0 holds v = 1, within this UDIM tile.
  vec2 t = vec2(uv.x - rx_m.tile.x, 1.0 - (uv.y - rx_m.tile.y));
  vec2 dx = vec2(ddx_uv.x, -ddx_uv.y);
  vec2 dy = vec2(ddy_uv.x, -ddy_uv.y);
  if (id == 0) return vec4(RX_S(rx_t0).rgb, 1.0);
  if (id == 1) return vec4(RX_S(rx_t0).a, 1.0, 0.0, 1.0);
  if (id == 2) return vec4(RX_S(rx_t1).a, 1.0, 0.0, 1.0);
  if (id == 3) return vec4(normalize(RX_S(rx_t1).xyz) * 0.5 + 0.5, 1.0);
  if (id == 4) return vec4(RX_S(rx_t2).x, 1.0, 0.0, 1.0);
  if (id == 5) return vec4(RX_S(rx_t3).rgb, 1.0);
  if (id == 6) return vec4(RX_S(rx_t2).w, 1.0, 0.0, 1.0);
  if (id == 7) return vec4(RX_S(rx_b0).a, 1.0, 0.0, 1.0);
  if (id == 8) return vec4(RX_S(rx_b0).rgb, 1.0);
  if (id == 9) return vec4(RX_S(rx_b1).a, 1.0, 0.0, 1.0);
  if (id == 10) return vec4(RX_S(rx_b2).x, 1.0, 0.0, 1.0);
  if (id == 11) return vec4(RX_S(rx_b2).y, 1.0, 0.0, 1.0);
  if (id == 12) return vec4(RX_S(rx_b2).z, 1.0, 0.0, 1.0);
  if (id == 13) return vec4(RX_S(rx_b1).rgb, 1.0);
  if (id == 14) return vec4(RX_S(rx_c0).a, 1.0, 0.0, 1.0);
  if (id == 15) return vec4(RX_S(rx_c0).rgb, 1.0);
  if (id == 16) return vec4(RX_S(rx_c1).a, 1.0, 0.0, 1.0);
  if (id == 17) return vec4(RX_S(rx_c2).x, 1.0, 0.0, 1.0);
  if (id == 18) return vec4(RX_S(rx_c1).rgb, 1.0);
  if (id == 19) return vec4(RX_S(rx_c2).y, 1.0, 0.0, 1.0);
  if (id == 20) return vec4(RX_S(rx_c2).z, 1.0, 0.0, 1.0);
  if (id == 32) return vec4(vec3(RX_S(rx_ao).r), 1.0);
  return vec4(0.0);
}

// The environment's frame (matches env_frame in viewport.wgsl).
vec3 rx_env_frame(vec3 d) {
  float c = cos(rx_g.params.y);
  float s = sin(rx_g.params.y);
  return vec3(c * d.x - s * d.z, d.y, s * d.x + c * d.z);
}

// Exposure is applied after shading (by the tone mapping pass), so lookups stay unexposed.
vec3 rx_env_sample(vec3 dir, float lod) {
  vec3 r = rx_env_frame(normalize(dir));
  vec2 uv = vec2(atan(r.z, r.x) / (2.0 * RX_PI) + 0.5, acos(clamp(r.y, -1.0, 1.0)) / RX_PI);
  return textureLod(sampler2D(rx_env_tex, rx_env_smp), uv, lod).rgb;
}

// The pyramid is GGX-prefiltered with roughness linear in the mip level.
vec3 rx_env_specular(vec3 dir, float roughness) {
  return rx_env_sample(dir, clamp(roughness, 0.0, 1.0) * rx_g.params.z);
}

vec3 rx_env_irradiance(vec3 n_world) {
  vec3 n = rx_env_frame(normalize(n_world));
  vec3 e = rx_g.sh[0].rgb * 0.282095;
  e += rx_g.sh[1].rgb * 0.488603 * n.y;
  e += rx_g.sh[2].rgb * 0.488603 * n.z;
  e += rx_g.sh[3].rgb * 0.488603 * n.x;
  e += rx_g.sh[4].rgb * 1.092548 * n.x * n.y;
  e += rx_g.sh[5].rgb * 1.092548 * n.y * n.z;
  e += rx_g.sh[6].rgb * 0.315392 * (3.0 * n.z * n.z - 1.0);
  e += rx_g.sh[7].rgb * 1.092548 * n.x * n.z;
  e += rx_g.sh[8].rgb * 0.546274 * (n.x * n.x - n.y * n.y);
  return max(e, vec3(0.0));
}

float rx_env_max_lod() { return rx_g.params.z; }
float rx_env_rotation() { return fract(rx_g.params.y / (2.0 * RX_PI)); }
float rx_env_exposure() { return 1.0; }
vec3 rx_eye_position() { return rx_g.eye.xyz; }
vec3 rx_camera_direction() { return rx_g.camera.xyz; }

mat4 rx_view_matrix() {
  vec3 f = normalize(rx_g.camera.xyz);
  vec3 s = normalize(cross(f, vec3(0.0, 1.0, 0.0)));
  vec3 u = cross(s, f);
  vec3 e = rx_g.eye.xyz;
  return mat4(
    vec4(s.x, u.x, -f.x, 0.0),
    vec4(s.y, u.y, -f.y, 0.0),
    vec4(s.z, u.z, -f.z, 0.0),
    vec4(-dot(s, e), -dot(u, e), dot(f, e), 1.0));
}

mat4 rx_view_proj_matrix() { return rx_g.view_proj; }
mat4 rx_inv_view_proj_matrix() { return rx_g.inv_view_proj; }
bool rx_is_perspective() { return rx_g.misc.z > 0.5; }
float rx_fovy() { return rx_g.camera.w; }
vec4 rx_screen_size() { return rx_g.screen; }
bool rx_is_2d_view() { return false; }
vec4 rx_main_light() { return vec4(normalize(vec3(0.3, 1.0, 0.5)), 1.0); }
float rx_scene_radius() { return rx_g.misc.y; }
float rx_time() { return rx_g.misc.x; }

void rx_finish(vec3 albedo, vec3 diffuse, vec3 specular, vec3 emissive, float alpha,
               vec4 sss_coefficients, vec3 sss_color) {
  // Opacity follows the 3D view's mode (blended transparency draws these opaque).
  int mode = int(rx_g.extra.x);
  if (mode == 1 && alpha < rx_g.extra.y) discard;
  if (mode == 2 && alpha < 0.5) discard;
  vec3 lit = albedo * diffuse;
  // Scattering surfaces hand their diffuse light to the screen-space SSS pass.
  bool scatter = sss_coefficients.w > 0.0 && rx_g.sss.z > 0.5;
  vec3 sss = scatter ? lit * sss_color : vec3(0.0);
  vec3 color = emissive * rx_g.post.w + specular + (scatter ? vec3(0.0) : lit);
  rx_out_color = vec4(color, 1.0);
  rx_out_sss = vec4(sss, 1.0);
}
