// lib-pbr.glsl: GGX microfacet BRDF helpers and image-based lighting.

//: param auto environment_max_lod
uniform float maxLod;

//: param custom {
//:   "default": 16,
//:   "label": "Quality",
//:   "widget": "combobox",
//:   "values": {
//:     "Very low (4 spp)": 4,
//:     "Low (16 spp)": 16,
//:     "Medium (32 spp)": 32,
//:     "High (64 spp)": 64,
//:     "Very high (128 spp)": 128,
//:     "Ultra (256 spp)": 256
//:   },
//:   "group": "Common Parameters"
//: }
uniform int nbSamples;

//: param custom { "default": 1.3, "label": "Horizon Fading", "min": 0.0, "max": 2.0, "group": "Common Parameters" }
uniform float horizonFade;

const float EPSILON_COEF = 1e-4;

// GGX / Trowbridge-Reitz distribution, alpha = roughness^2.
float normal_distrib(float ndh, float Roughness) {
  float alpha = Roughness * Roughness;
  float a2 = alpha * alpha;
  float d = ndh * ndh * (a2 - 1.0) + 1.0;
  return a2 / max(M_PI * d * d, 1e-12);
}

// Schlick Fresnel, spherical-Gaussian approximation of the power of 5.
vec3 fresnel(float vdh, vec3 F0) {
  float sphg = exp2((-5.55473 * vdh - 6.98316) * vdh);
  return F0 + (vec3(1.0) - F0) * sphg;
}

// Schlick-Smith masking for one direction, divided by its cosine.
float G1(float ndw, float k) {
  return 1.0 / (ndw * (1.0 - k) + k);
}

// Masking-shadowing divided by (n.l)(n.v).
float visibility(float ndl, float ndv, float Roughness) {
  float k = max(Roughness * Roughness * 0.5, 1e-5);
  return G1(ndl, k) * G1(ndv, k);
}

// Specular weight of one GGX importance sample (the pdf is already divided out).
vec3 cook_torrance_contrib(float vdh, float ndh, float ndl, float ndv, vec3 Ks, float Roughness) {
  return fresnel(vdh, Ks) * (visibility(ndl, ndv, Roughness) * vdh * ndl / max(ndh, EPSILON_COEF));
}

vec3 importanceSampleGGX(vec2 Xi, vec3 T, vec3 B, vec3 N, float roughness) {
  float a = roughness * roughness;
  float cosT = sqrt((1.0 - Xi.y) / (1.0 + (a * a - 1.0) * Xi.y));
  float sinT = sqrt(max(1.0 - cosT * cosT, 0.0));
  float phi = M_2PI * Xi.x;
  return T * (sinT * cos(phi)) + B * (sinT * sin(phi)) + N * cosT;
}

float probabilityGGX(float ndh, float vdh, float Roughness) {
  return normal_distrib(ndh, Roughness) * ndh / (4.0 * max(vdh, EPSILON_COEF));
}

// Solid-angle stretch of equirectangular texels: sin of the polar angle.
float distortion(vec3 Wn) {
  return sqrt(max(1.0 - Wn.y * Wn.y, 1e-6));
}

// Filtered importance sampling: pick the mip whose texel solid angle matches
// the sample's solid angle 1/(N*pdf). Base level of an equirect map whose
// width is 2^maxLod has texels of 4*pi^2*sin(theta)/width^2 steradians.
float computeLOD(vec3 Ln, float p) {
  float samples = float(nbSamples);
  return max(maxLod - 0.5 * log2(max(samples * p * 4.0 * M_PI * M_PI * distortion(Ln), 1e-12)), 0.0);
}

// Fades reflections pointing below the geometric surface.
float horizonFading(float ndl, float horizonFade) {
  float h = clamp(1.0 + horizonFade * ndl, 0.0, 1.0);
  return h * h;
}

vec3 pbrComputeDiffuse(vec3 normal, vec3 diffColor) {
  return envIrradiance(normal) * diffColor;
}

// Split-sum image-based specular: prefiltered radiance along the reflection
// times an analytic fit of the pre-integrated GGX BRDF (Karis 2014, mobile
// approximation). One environment fetch instead of nbSamples.
vec3 rx_env_brdf(vec3 specColor, float roughness, float ndv) {
  vec4 r = roughness * vec4(-1.0, -0.0275, -0.572, 0.022) + vec4(1.0, 0.0425, 1.04, -0.04);
  float a004 = min(r.x * r.x, exp2(-9.28 * ndv)) * r.x + r.y;
  vec2 ab = vec2(-1.04, 1.04) * a004 + r.zw;
  return specColor * ab.x + ab.y;
}

vec3 pbrComputeSpecular(LocalVectors vectors, vec3 specColor, float roughness) {
  float ndv = max(dot(vectors.normal, vectors.eye), 1e-4);
  vec3 dir = reflect(-vectors.eye, vectors.normal);
  float fade = horizonFading(dot(vectors.vertexNormal, dir), horizonFade);
  return fade * rx_env_specular(dir, roughness) * rx_env_brdf(specColor, roughness, ndv);
}
