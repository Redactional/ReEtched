// lib-sss.glsl: subsurface scattering coefficients for sssCoefficientsOutput.

//: param auto channel_scattering
uniform SamplerSparse sss_tex;
//: param auto scene_original_radius
uniform float sssSceneScale;
//: param custom { "default": true, "label": "Enable", "group": "Subsurface Scattering Parameters", "description": "Needs a Scattering channel" }
uniform bool sssEnabled;
//: param custom { "default": 1, "label": "Scattering Type", "widget": "combobox", "values": { "Translucent": 0, "Skin": 1 }, "group": "Subsurface Scattering Parameters" }
uniform int sssType;
//: param custom { "default": 0.5, "label": "Scale", "min": 0.01, "max": 1.0, "group": "Subsurface Scattering Parameters" }
uniform float sssScale;
//: param custom { "default": [0.701, 0.301, 0.305], "label": "Color", "widget": "color", "group": "Subsurface Scattering Parameters" }
uniform vec3 sssColor;

// rgb: per-channel scattering radius in scene units, w: 1 when any is non-zero.
vec4 getSSSCoefficients(float scattering) {
  if (!sssEnabled) {
    return vec4(0.0);
  }
  vec3 coeffs = scattering * sssColor * (sssScale / max(sssSceneScale, 1e-6));
  return vec4(coeffs, coeffs != vec3(0.0) ? 1.0 : 0.0);
}

vec4 getSSSCoefficients(SparseCoord coord) {
  return getSSSCoefficients(getScattering(sss_tex, coord));
}
