// lib-sampler.glsl: channel getters with Painter's defaults.
//
// Channel values are premultiplied by coverage: grayscale channels carry the
// value in R and coverage in G, color channels carry coverage in A. Wherever
// coverage is missing the getters fill in the documented default, so an
// absent channel (all zeros) yields exactly the default.

//: param auto ao_blending_mode
uniform int ao_blending_mode;
//: param auto texture_ao
uniform SamplerSparse base_ao_tex;
//: param auto channel_ao
uniform SamplerSparse ao_tex;
//: param custom { "default": 0.75, "label": "AO Intensity", "min": 0.0, "max": 1.0, "group": "Common Parameters" }
uniform float ao_intensity;
//: param auto shadow_mask_enable
uniform bool sm_enable;
//: param auto shadow_mask_opacity
uniform float sm_opacity;
//: param auto screen_size
uniform vec4 screen_size;

const vec3 DEFAULT_BASE_COLOR = vec3(0.5);
const float DEFAULT_ROUGHNESS = 0.3;
const float DEFAULT_METALLIC = 0.0;
const float DEFAULT_ANISOTROPY_LEVEL = 0.0;
const float DEFAULT_ANISOTROPY_ANGLE = 0.0;
const float DEFAULT_OPACITY = 1.0;
const float DEFAULT_AO = 1.0;
const float DEFAULT_SPECULAR_LEVEL = 0.5;
const float DEFAULT_HEIGHT = 0.0;
const float DEFAULT_DISPLACEMENT = 0.0;
const float DEFAULT_SCATTERING = 0.0;

float getGlossiness(vec4 sampledValue) {
  return sampledValue.r + (1.0 - DEFAULT_ROUGHNESS) * (1.0 - sampledValue.g);
}
float getGlossiness(SamplerSparse s, SparseCoord coord) {
  return getGlossiness(textureSparse(s, coord));
}

float getRoughness(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_ROUGHNESS * (1.0 - sampledValue.g);
}
float getRoughness(SamplerSparse s, SparseCoord coord) {
  return getRoughness(textureSparse(s, coord));
}

float getMetallic(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_METALLIC * (1.0 - sampledValue.g);
}
float getMetallic(SamplerSparse s, SparseCoord coord) {
  return getMetallic(textureSparse(s, coord));
}

float getAnisotropyLevel(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_ANISOTROPY_LEVEL * (1.0 - sampledValue.g);
}
float getAnisotropyLevel(SamplerSparse s, SparseCoord coord) {
  return getAnisotropyLevel(textureSparse(s, coord));
}

// The channel stores the angle as a fraction of a full turn.
float getAnisotropyAngle(vec4 sampledValue) {
  return M_2PI * (sampledValue.r + DEFAULT_ANISOTROPY_ANGLE * (1.0 - sampledValue.g));
}
// ponytail: plain filtered lookup; filtering across the 0/1 wrap smears the
// angle there (Painter unwraps it with a manual trilinear fetch).
float getAnisotropyAngle(SamplerSparse s, SparseCoord coord) {
  return getAnisotropyAngle(textureSparse(s, coord));
}

float getOpacity(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_OPACITY * (1.0 - sampledValue.g);
}
float getOpacity(SamplerSparse s, SparseCoord coord) {
  return getOpacity(textureSparse(s, coord));
}

float getHeight(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_HEIGHT * (1.0 - sampledValue.g);
}
float getHeight(SamplerSparse s, SparseCoord coord) {
  return getHeight(textureSparse(s, coord));
}

float getDisplacement(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_DISPLACEMENT * (1.0 - sampledValue.g);
}
float getDisplacement(SamplerSparse s, SparseCoord coord) {
  return getDisplacement(textureSparse(s, coord));
}

float getSpecularLevel(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_SPECULAR_LEVEL * (1.0 - sampledValue.g);
}
float getSpecularLevel(SamplerSparse s, SparseCoord coord) {
  return getSpecularLevel(textureSparse(s, coord));
}

float getScattering(vec4 sampledValue) {
  return sampledValue.r + DEFAULT_SCATTERING * (1.0 - sampledValue.g);
}
float getScattering(SamplerSparse s, SparseCoord coord) {
  return getScattering(textureSparse(s, coord));
}

// Baked AO (mesh map: R value, A validity) combined with the painted AO
// channel, then attenuated by the AO intensity slider.
float getAO(SparseCoord coord, bool is_premult) {
  vec4 base = textureSparse(base_ao_tex, coord);
  float ao = base.r + DEFAULT_AO * (1.0 - base.a);
  if (rx_is_set(ao_tex)) {
    vec4 v = textureSparse(ao_tex, coord);
    float channel_ao = is_premult ? v.r + DEFAULT_AO * (1.0 - v.g)
                                  : (v.g > 0.0 ? v.r / v.g : DEFAULT_AO);
    if (ao_blending_mode == BlendingMode_Replace) {
      ao = mix(ao, channel_ao, v.g);
    } else {
      ao *= channel_ao;
    }
  }
  return mix(1.0, ao, ao_intensity);
}

float getAO(SparseCoord coord) {
  return getAO(coord, true);
}

// No shadows in the ReEtched viewport.
float getShadowFactor() {
  return 1.0;
}

vec3 getBaseColor(vec4 sampledValue) {
  return sampledValue.rgb + DEFAULT_BASE_COLOR * (1.0 - sampledValue.a);
}
vec3 getBaseColor(SamplerSparse s, SparseCoord coord) {
  return getBaseColor(textureSparse(s, coord));
}

vec3 getDiffuse(vec4 sampledValue) {
  return getBaseColor(sampledValue);
}
vec3 getDiffuse(SamplerSparse s, SparseCoord coord) {
  return getDiffuse(textureSparse(s, coord));
}

vec3 getSpecularColor(vec4 sampledValue) {
  vec3 default_specular = mix(vec3(0.04), DEFAULT_BASE_COLOR, DEFAULT_METALLIC);
  return sampledValue.rgb + default_specular * (1.0 - sampledValue.a);
}
vec3 getSpecularColor(SamplerSparse s, SparseCoord coord) {
  return getSpecularColor(textureSparse(s, coord));
}

vec2 generateAnisotropicRoughness(float roughness, float anisoLevel) {
  return vec2(roughness, roughness / sqrt(max(1.0 - anisoLevel, 1e-8)));
}

vec3 generateDiffuseColor(vec3 baseColor, float metallic) {
  return baseColor * (1.0 - metallic);
}

vec3 generateSpecularColor(float specularLevel, vec3 baseColor, float metallic) {
  return mix(vec3(0.08 * specularLevel), baseColor, metallic);
}

vec3 generateSpecularColor(vec3 baseColor, float metallic) {
  return mix(vec3(0.04), baseColor, metallic);
}
