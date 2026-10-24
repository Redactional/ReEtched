// lib-env.glsl: environment lighting. The host applies rotation and exposure.

//: param auto environment_rotation
uniform float environment_rotation;
//: param auto environment_exposure
uniform float environment_exposure;

vec3 envSampleLOD(vec3 dir, float lod) {
  return rx_env_sample(dir, lod);
}

vec3 envIrradiance(vec3 dir) {
  return rx_env_irradiance(dir);
}
