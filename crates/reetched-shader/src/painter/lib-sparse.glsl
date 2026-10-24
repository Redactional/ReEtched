// lib-sparse.glsl: always included. Holds the types Painter gives every shader
// (SamplerSparse, SparseCoord, V2F), the shade() output functions and the
// sparse-texture sampling API. Painter's sparse virtual textures are not a
// thing here: every channel is fully resident, so the "climb the mip pyramid"
// logic reduces to plain gradient sampling.

// A SamplerSparse is just a channel id (see HOST.md); negative = absent.
#define SamplerSparse int

struct SparseCoord {
  vec2 tex_coord;
  vec2 dfdx;
  vec2 dfdy;
  float lod;
  uint material_lod_mask;
};

struct V2F {
  vec3 normal;
  vec3 tangent;
  vec3 bitangent;
  vec3 position;
  vec4 color[1];
  vec2 tex_coord;
  SparseCoord sparse_coord;
  vec2 multi_tex_coord[8];
};

bool rx_is_set(int s) {
  return s >= 0 && rx_channel_present(s);
}

// Painter's SamplerSparse.size: (width, height, 1/width, 1/height).
vec4 rx_sparse_size(int s) {
  vec2 size = s >= 0 ? rx_channel_size(s) : vec2(0.0);
  return vec4(size, size.x > 0.0 ? 1.0 / size.x : 0.0, size.y > 0.0 ? 1.0 / size.y : 0.0);
}

vec4 rx_sample(int s, vec2 uv, vec2 dx, vec2 dy) {
  if (s < 0) {
    return vec4(0.0);
  }
  return rx_channel(s, uv, dx, dy);
}

// Painter's camera_view_matrix_it. View matrices are rigid (rotation +
// translation), so the inverse-transpose keeps the rotation and moves the
// rotated-back translation into the bottom row.
mat4 rx_view_matrix_it() {
  mat4 v = rx_view_matrix();
  mat3 r = mat3(v);
  vec3 t = -(transpose(r) * v[3].xyz);
  return mat4(vec4(r[0], t.x), vec4(r[1], t.y), vec4(r[2], t.z), vec4(0.0, 0.0, 0.0, 1.0));
}

SparseCoord getSparseCoord(vec2 tex_coord) {
  SparseCoord coord;
  coord.tex_coord = tex_coord;
  coord.dfdx = dFdx(tex_coord);
  coord.dfdy = dFdy(tex_coord);
  // ponytail: lod is not tracked (always 0); textureSparseQueryLod computes it on demand.
  coord.lod = 0.0;
  coord.material_lod_mask = 0u;
  return coord;
}

SparseCoord getSparseCoordLod0(vec2 tex_coord) {
  SparseCoord coord;
  coord.tex_coord = tex_coord;
  coord.dfdx = vec2(0.0);
  coord.dfdy = vec2(0.0);
  coord.lod = 0.0;
  coord.material_lod_mask = 0u;
  return coord;
}

// Standard mip selection from the UV footprint in texels.
float textureSparseQueryLod(SamplerSparse s, SparseCoord coord) {
  vec2 size = rx_sparse_size(s).xy;
  vec2 dx = coord.dfdx * size;
  vec2 dy = coord.dfdy * size;
  float rho = max(dot(dx, dx), dot(dy, dy));
  return max(0.5 * log2(max(rho, 1e-20)), 0.0);
}

void textureSparseQueryGrad(out vec2 dfdx, out vec2 dfdy, SamplerSparse s, SparseCoord coord) {
  dfdx = coord.dfdx;
  dfdy = coord.dfdy;
}

vec4 textureSparse(SamplerSparse s, SparseCoord coord) {
  return rx_sample(s, coord.tex_coord, coord.dfdx, coord.dfdy);
}

void textureSparseOffsets(SamplerSparse s, SparseCoord coord, vec2 offsets[1], out vec4 results[1]) {
  results[0] = rx_sample(s, coord.tex_coord + offsets[0], coord.dfdx, coord.dfdy);
}

void textureSparseOffsets(SamplerSparse s, SparseCoord coord, vec2 offsets[2], out vec4 results[2]) {
  results[0] = rx_sample(s, coord.tex_coord + offsets[0], coord.dfdx, coord.dfdy);
  results[1] = rx_sample(s, coord.tex_coord + offsets[1], coord.dfdx, coord.dfdy);
}

void textureSparseOffsets(SamplerSparse s, SparseCoord coord, vec2 offsets[3], out vec4 results[3]) {
  results[0] = rx_sample(s, coord.tex_coord + offsets[0], coord.dfdx, coord.dfdy);
  results[1] = rx_sample(s, coord.tex_coord + offsets[1], coord.dfdx, coord.dfdy);
  results[2] = rx_sample(s, coord.tex_coord + offsets[2], coord.dfdx, coord.dfdy);
}

void textureSparseOffsets(SamplerSparse s, SparseCoord coord, vec2 offsets[4], out vec4 results[4]) {
  results[0] = rx_sample(s, coord.tex_coord + offsets[0], coord.dfdx, coord.dfdy);
  results[1] = rx_sample(s, coord.tex_coord + offsets[1], coord.dfdx, coord.dfdy);
  results[2] = rx_sample(s, coord.tex_coord + offsets[2], coord.dfdx, coord.dfdy);
  results[3] = rx_sample(s, coord.tex_coord + offsets[3], coord.dfdx, coord.dfdy);
}

// shade() outputs, with Painter's documented defaults (set again by main()).
vec3 rx_shade_albedo;
vec3 rx_shade_diffuse;
vec3 rx_shade_specular;
vec3 rx_shade_emissive;
float rx_shade_alpha;
vec4 rx_shade_sss_coefficients;
vec3 rx_shade_sss_color;

void albedoOutput(vec3 c) { rx_shade_albedo = c; }
void diffuseShadingOutput(vec3 c) { rx_shade_diffuse = c; }
void specularShadingOutput(vec3 c) { rx_shade_specular = c; }
void emissiveColorOutput(vec3 c) { rx_shade_emissive = c; }
void alphaOutput(float a) { rx_shade_alpha = a; }
void sssCoefficientsOutput(vec4 c) { rx_shade_sss_coefficients = c; }
void sssColorOutput(vec3 c) { rx_shade_sss_color = c; }

V2F rx_build_inputs() {
  V2F inputs;
  vec3 n = normalize(rx_in_normal);
  // Re-orthogonalize the interpolated tangent; fall back to any perpendicular
  // axis when the mesh has no usable tangents (no UVs).
  vec3 t = rx_in_tangent.xyz - n * dot(n, rx_in_tangent.xyz);
  if (dot(t, t) < 1e-12) {
    t = cross(abs(n.y) < 0.99 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0), n);
  }
  t = normalize(t);
  float handedness = rx_in_tangent.w < 0.0 ? -1.0 : 1.0;
  inputs.normal = n;
  inputs.tangent = t;
  inputs.bitangent = handedness * cross(n, t);
  inputs.position = rx_in_position;
  // ponytail: no vertex colors in ReEtched meshes, color0 is white.
  inputs.color[0] = vec4(1.0);
  inputs.tex_coord = rx_in_uv;
  inputs.sparse_coord = getSparseCoord(rx_in_uv);
  // ponytail: only two UV sets exist; uv2..uv7 repeat uv0.
  inputs.multi_tex_coord[0] = rx_in_uv;
  inputs.multi_tex_coord[1] = rx_in_uv2;
  inputs.multi_tex_coord[2] = rx_in_uv;
  inputs.multi_tex_coord[3] = rx_in_uv;
  inputs.multi_tex_coord[4] = rx_in_uv;
  inputs.multi_tex_coord[5] = rx_in_uv;
  inputs.multi_tex_coord[6] = rx_in_uv;
  inputs.multi_tex_coord[7] = rx_in_uv;
  return inputs;
}

void rx_reset_outputs() {
  rx_shade_albedo = vec3(1.0);
  rx_shade_diffuse = vec3(0.0);
  rx_shade_specular = vec3(0.0);
  rx_shade_emissive = vec3(0.0);
  rx_shade_alpha = 1.0;
  rx_shade_sss_coefficients = vec4(0.0);
  rx_shade_sss_color = vec3(1.0);
}

void rx_emit_outputs() {
  rx_finish(rx_shade_albedo, rx_shade_diffuse, rx_shade_specular, rx_shade_emissive,
            rx_shade_alpha, rx_shade_sss_coefficients, rx_shade_sss_color);
}
