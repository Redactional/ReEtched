// lib-utils.glsl: color space conversions and small helpers.

//: param auto conversion_linear_to_srgb
uniform bool convert_to_srgb_opt;

// Painter picks this per export target; in the viewport nothing is converted.
const int output_conversion_method = 0;

vec3 tonemapSCurve(vec3 value, float sigma, float n) {
  vec3 pv = pow(max(value, vec3(0.0)), vec3(n));
  return pv / (pv + pow(sigma, n));
}

float sRGB2linear(float x) {
  return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4);
}

vec3 sRGB2linear(vec3 rgb) {
  return vec3(sRGB2linear(rgb.r), sRGB2linear(rgb.g), sRGB2linear(rgb.b));
}

vec4 sRGB2linear(vec4 rgba) {
  return vec4(sRGB2linear(rgba.rgb), rgba.a);
}

float linear2sRGB(float x) {
  return x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

vec3 linear2sRGB(vec3 rgb) {
  return vec3(linear2sRGB(rgb.r), linear2sRGB(rgb.g), linear2sRGB(rgb.b));
}

vec4 linear2sRGB(vec4 rgba) {
  return vec4(linear2sRGB(rgba.rgb), rgba.a);
}

float linear2sRGBOpt(float x) {
  return convert_to_srgb_opt ? linear2sRGB(x) : x;
}

vec3 linear2sRGBOpt(vec3 rgb) {
  return convert_to_srgb_opt ? linear2sRGB(rgb) : rgb;
}

vec4 linear2sRGBOpt(vec4 rgba) {
  return convert_to_srgb_opt ? linear2sRGB(rgba) : rgba;
}

// 0: none, 1: linear to sRGB, 2: sRGB to linear.
float convertOutput(float x) {
  if (output_conversion_method == 1) return linear2sRGB(x);
  if (output_conversion_method == 2) return sRGB2linear(x);
  return x;
}

vec3 convertOutput(vec3 rgb) {
  if (output_conversion_method == 1) return linear2sRGB(rgb);
  if (output_conversion_method == 2) return sRGB2linear(rgb);
  return rgb;
}

vec4 convertOutput(vec4 rgba) {
  return vec4(convertOutput(rgba.rgb), rgba.a);
}

float getDitherThreshold(uvec2 coords) {
  return bayerMatrix8(coords);
}

vec4 RGB2Gray(vec4 rgba) {
  float g = dot(rgba.rgb, vec3(0.299, 0.587, 0.114));
  return vec4(g, g, g, rgba.a);
}

// Glossy metals reflect mostly from directions AO does not describe well, so
// lift their specular occlusion toward 1.
float specularOcclusionCorrection(float diffuseOcclusion, float metallic, float roughness) {
  float gloss = 1.0 - roughness;
  return mix(diffuseOcclusion, 1.0, metallic * gloss * gloss);
}
