// lib-emissive.glsl

//: param auto channel_emissive
uniform SamplerSparse emissive_tex;

//: param custom { "default": 1.0, "label": "Emissive Intensity", "min": 0.0, "max": 100.0, "group": "Common Parameters" }
uniform float emissive_intensity;

vec3 pbrComputeEmissive(SamplerSparse emissive, SparseCoord coord) {
  return emissive_intensity * textureSparse(emissive, coord).rgb;
}
