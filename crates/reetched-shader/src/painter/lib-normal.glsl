// lib-normal.glsl: tangent-space normal helpers.

//: param auto channel_height
uniform SamplerSparse height_texture;
//: param auto channel_normal
uniform SamplerSparse normal_texture;
//: param auto texture_normal
uniform SamplerSparse base_normal_texture;
//: param auto normal_blending_mode
uniform int normal_blending_mode;
//: param auto normal_y_coeff
uniform float base_normal_y_coeff;

// Scale between height-channel slopes and normal tilt.
const float HEIGHT_FACTOR = 400.0;

// Whiteout blend: add the slopes, multiply the up components.
vec3 normalBlend(vec3 baseNormal, vec3 overNormal) {
  return normalize(vec3(baseNormal.xy + overNormal.xy, baseNormal.z * overNormal.z));
}

// Reoriented normal mapping: rotates the detail normal onto the base normal.
vec3 normalBlendOriented(vec3 baseNormal, vec3 overNormal) {
  vec3 t = baseNormal + vec3(0.0, 0.0, 1.0);
  vec3 u = overNormal * vec3(-1.0, -1.0, 1.0);
  return normalize(t * dot(t, u) / t.z - u);
}

vec3 normalFade(vec3 normal, float attenuation) {
  if (attenuation < 1.0 && normal.z < 1.0) {
    return normalize(mix(vec3(0.0, 0.0, 1.0), normal, attenuation));
  }
  return normal;
}

// Channel values are premultiplied by coverage (alpha); coverage also fades
// the normal back to flat.
vec3 normalUnpack(vec4 normal_alpha, float y_coeff) {
  if (normal_alpha.a <= 0.0 || normal_alpha.xyz == vec3(0.0)) {
    return vec3(0.0, 0.0, 1.0);
  }
  vec3 n = normal_alpha.xyz / normal_alpha.a * 2.0 - vec3(1.0);
  n.y *= y_coeff;
  return normalFade(normalize(n), normal_alpha.a);
}

vec3 normalUnpack(vec4 normal_alpha) {
  return normalUnpack(normal_alpha, 1.0);
}

// Central differences of the height channel one texel apart.
// ponytail: slope scale tuned by eye, not matched to Painter's exact strength.
vec3 normalFromHeight(SparseCoord coord, float height_force) {
  if (!rx_is_set(height_texture)) {
    return vec3(0.0, 0.0, 1.0);
  }
  vec4 size = rx_sparse_size(height_texture);
  vec2 texel = size.x > 0.0 ? size.zw : vec2(1.0 / 2048.0);
  vec2 offsets[4] = vec2[4](vec2(-texel.x, 0.0), vec2(texel.x, 0.0), vec2(0.0, -texel.y), vec2(0.0, texel.y));
  vec4 h[4];
  textureSparseOffsets(height_texture, coord, offsets, h);
  // Slopes in height units per texel; HEIGHT_FACTOR / 64 gives Painter-like bump strength.
  float dhdu = 0.5 * (h[1].r - h[0].r);
  float dhdv = 0.5 * (h[3].r - h[2].r);
  float k = height_force * HEIGHT_FACTOR / 64.0;
  return normalize(vec3(-dhdu * k, -dhdv * k, 1.0));
}

vec3 getTSNormal(SparseCoord coord, vec3 normalFromHeight) {
  vec3 base = normalUnpack(textureSparse(base_normal_texture, coord), base_normal_y_coeff);
  vec3 n = normalBlend(base, normalFromHeight);
  if (rx_is_set(normal_texture)) {
    vec3 channel = normalUnpack(textureSparse(normal_texture, coord));
    if (normal_blending_mode == BlendingMode_Replace) {
      n = normalBlend(channel, normalFromHeight);
    } else {
      n = normalBlendOriented(n, channel);
    }
  }
  return n;
}

vec3 getTSNormal(SparseCoord coord) {
  return getTSNormal(coord, normalFromHeight(coord, 1.0));
}

vec3 computeWSBaseNormal(SparseCoord coord, vec3 tangent, vec3 bitangent, vec3 normal) {
  vec3 n = normalUnpack(textureSparse(base_normal_texture, coord), base_normal_y_coeff);
  return normalize(n.x * tangent + n.y * bitangent + n.z * normal);
}

vec3 computeWSNormal(SparseCoord coord, vec3 tangent, vec3 bitangent, vec3 normal) {
  vec3 n = getTSNormal(coord);
  return normalize(n.x * tangent + n.y * bitangent + n.z * normal);
}
