// Test stand-in for ReEtched's viewport host header (contract: HOST.md).
// Plausible bindings; the real header lives in the app.

layout(location = 0) in vec3 rx_in_position;
layout(location = 1) in vec3 rx_in_normal;
layout(location = 2) in vec2 rx_in_uv;
layout(location = 3) in vec4 rx_in_tangent;
layout(location = 4) in vec2 rx_in_uv2;

layout(location = 0) out vec4 rx_frag_color;

layout(set = 0, binding = 0, std140) uniform RxGlobals {
  mat4 view;
  mat4 view_proj;
  mat4 inv_view_proj;
  vec4 eye;          // xyz position, w: 1 = perspective
  vec4 camera_dir;   // xyz forward, w: fovy (radians)
  vec4 screen;       // width, height, 1/width, 1/height
  vec4 env;          // rotation (turns), exposure (linear), max lod, time (s)
  vec4 main_light;   // xyz direction toward the light, w intensity
  vec4 misc;         // x: 2D view flag, y: scene radius
  uvec4 present;     // channel presence bits: x ids 0-31, y ids 32-63
  vec4 sizes[3];     // texel size per bound channel texture (xy), see rx_channel_size
  vec4 sh[9];        // irradiance SH (rgb), already divided by pi and exposed
} rx_globals;
layout(set = 0, binding = 1) uniform texture2D rx_env_texture;
layout(set = 0, binding = 2) uniform sampler rx_env_sampler;

// Set 1: the texture set. The stub packs a few channels; ids it lacks read as absent.
layout(set = 1, binding = 0) uniform texture2D rx_tex_basecolor;   // rgb + coverage
layout(set = 1, binding = 1) uniform texture2D rx_tex_normal;      // tangent space 0..1
layout(set = 1, binding = 2) uniform texture2D rx_tex_orm;         // ao, roughness, metallic
layout(set = 1, binding = 3) uniform sampler rx_channel_sampler;

bool rx_channel_present(int id) {
  if (id < 0 || id > 63) return false;
  uint bits = id < 32 ? rx_globals.present.x : rx_globals.present.y;
  return (bits & (1u << uint(id & 31))) != 0u;
}

vec2 rx_channel_size(int id) {
  if (!rx_channel_present(id)) return vec2(0.0);
  if (id == 0) return rx_globals.sizes[0].xy;
  if (id == 3) return rx_globals.sizes[1].xy;
  return rx_globals.sizes[2].xy;
}

vec4 rx_channel(int id, vec2 uv, vec2 ddx_uv, vec2 ddy_uv) {
  if (!rx_channel_present(id)) return vec4(0.0);
  if (id == 0) return textureGrad(sampler2D(rx_tex_basecolor, rx_channel_sampler), uv, ddx_uv, ddy_uv);
  if (id == 3) return vec4(textureGrad(sampler2D(rx_tex_normal, rx_channel_sampler), uv, ddx_uv, ddy_uv).xyz, 1.0);
  vec4 orm = textureGrad(sampler2D(rx_tex_orm, rx_channel_sampler), uv, ddx_uv, ddy_uv);
  if (id == 20) return vec4(orm.r, 1.0, 0.0, 1.0);
  if (id == 2) return vec4(orm.g, 1.0, 0.0, 1.0);
  if (id == 1) return vec4(orm.b, 1.0, 0.0, 1.0);
  return vec4(0.0);
}

const float RX_PI = 3.14159265;

vec2 rx_equirect_uv(vec3 dir) {
  vec3 d = normalize(dir);
  float u = atan(d.z, d.x) / (2.0 * RX_PI) + 0.5 + rx_globals.env.x;
  float v = acos(clamp(d.y, -1.0, 1.0)) / RX_PI;
  return vec2(u, v);
}

vec3 rx_env_sample(vec3 dir, float lod) {
  return textureLod(sampler2D(rx_env_texture, rx_env_sampler), rx_equirect_uv(dir), lod).rgb * rx_globals.env.y;
}

vec3 rx_env_specular(vec3 dir, float roughness) {
  return rx_env_sample(dir, sqrt(clamp(roughness, 0.0, 1.0)) * rx_globals.env.z);
}

// Order-2 SH irradiance (Ramamoorthi & Hanrahan basis); coefficients are
// convolved and rotated on the CPU.
vec3 rx_env_irradiance(vec3 n) {
  vec3 r = rx_globals.sh[0].rgb
    + rx_globals.sh[1].rgb * n.y + rx_globals.sh[2].rgb * n.z + rx_globals.sh[3].rgb * n.x
    + rx_globals.sh[4].rgb * (n.x * n.y) + rx_globals.sh[5].rgb * (n.y * n.z)
    + rx_globals.sh[6].rgb * (3.0 * n.z * n.z - 1.0)
    + rx_globals.sh[7].rgb * (n.x * n.z) + rx_globals.sh[8].rgb * (n.x * n.x - n.y * n.y);
  return max(r, vec3(0.0));
}

float rx_env_max_lod() { return rx_globals.env.z; }
float rx_env_rotation() { return rx_globals.env.x; }
float rx_env_exposure() { return rx_globals.env.y; }
vec3 rx_eye_position() { return rx_globals.eye.xyz; }
vec3 rx_camera_direction() { return rx_globals.camera_dir.xyz; }
mat4 rx_view_matrix() { return rx_globals.view; }
mat4 rx_view_proj_matrix() { return rx_globals.view_proj; }
mat4 rx_inv_view_proj_matrix() { return rx_globals.inv_view_proj; }
bool rx_is_perspective() { return rx_globals.eye.w > 0.5; }
float rx_fovy() { return rx_globals.camera_dir.w; }
vec4 rx_screen_size() { return rx_globals.screen; }
bool rx_is_2d_view() { return rx_globals.misc.x > 0.5; }
vec4 rx_main_light() { return rx_globals.main_light; }
float rx_scene_radius() { return rx_globals.misc.y; }
float rx_time() { return rx_globals.env.w; }

void rx_finish(vec3 albedo, vec3 diffuse, vec3 specular, vec3 emissive, float alpha,
               vec4 sss_coefficients, vec3 sss_color) {
  vec3 color = emissive + albedo * diffuse + specular;
  rx_frag_color = vec4(color, clamp(alpha, 0.0, 1.0));
}
