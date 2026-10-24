// lib-pbr-aniso.glsl: anisotropic GGX, evaluated in the local tangent frame
// (x = tangent, y = bitangent, z = normal). alpha = per-axis roughness^2.

float normal_distrib(vec3 localH, vec2 alpha) {
  vec3 h = localH / vec3(alpha, 1.0);
  float d = dot(h, h);
  return 1.0 / max(M_PI * alpha.x * alpha.y * d * d, 1e-12);
}

// Smith masking for one direction, divided by its cosine.
float G1(vec3 localW, vec2 alpha) {
  float z2 = max(localW.z * localW.z, 1e-8);
  vec2 wa = localW.xy * alpha;
  float lambda = 0.5 * (-1.0 + sqrt(1.0 + dot(wa, wa) / z2));
  return 1.0 / ((1.0 + lambda) * max(localW.z, 1e-4));
}

float visibility(vec3 localL, vec3 localV, vec2 alpha) {
  return G1(localL, alpha) * G1(localV, alpha);
}

vec3 cook_torrance_contrib(float vdh, float ndh, vec3 localL, vec3 localE, vec3 Ks, vec2 alpha) {
  return fresnel(vdh, Ks) * (visibility(localL, localE, alpha) * vdh * localL.z / max(ndh, EPSILON_COEF));
}

// Samples half vectors proportionally to D(h) * (n.h).
vec3 importanceSampleGGX(vec2 Xi, vec2 alpha) {
  float phi = M_2PI * Xi.x;
  float u = min(Xi.y, 0.9999);
  vec2 slope = sqrt(u / (1.0 - u)) * vec2(alpha.x * cos(phi), alpha.y * sin(phi));
  return normalize(vec3(slope, 1.0));
}

float probabilityGGX(vec3 localH, float vdh, vec2 alpha) {
  return normal_distrib(localH, alpha) * localH.z / (4.0 * max(vdh, EPSILON_COEF));
}

// Monte Carlo over the environment mip pyramid with pdf-driven mip selection.
vec3 pbrComputeSpecularAnisotropic(LocalVectors vectors, vec3 specColor, vec2 roughness) {
  mat3 tangentToWorld = mat3(vectors.tangent, vectors.bitangent, vectors.normal);
  vec3 localE = vec3(dot(vectors.eye, vectors.tangent), dot(vectors.eye, vectors.bitangent), dot(vectors.eye, vectors.normal));
  vec2 alpha = max(roughness * roughness, vec2(1e-4));
  vec3 radiance = vec3(0.0);
  int count = max(nbSamples, 1);
  for (int i = 0; i < count; ++i) {
    vec2 Xi = fibonacci2DDitheredTemporal(i, count);
    vec3 localH = importanceSampleGGX(Xi, alpha);
    float vdh = dot(localE, localH);
    vec3 localL = 2.0 * vdh * localH - localE;
    if (localL.z > 0.0) {
      vec3 Ln = tangentToWorld * localL;
      float fade = horizonFading(dot(vectors.vertexNormal, Ln), horizonFade);
      float pdf = probabilityGGX(localH, vdh, alpha);
      float lod = computeLOD(Ln, pdf);
      radiance += fade * envSampleLOD(Ln, lod) *
                  cook_torrance_contrib(vdh, localH.z, localL, localE, specColor, alpha);
    }
  }
  return radiance / float(count);
}
