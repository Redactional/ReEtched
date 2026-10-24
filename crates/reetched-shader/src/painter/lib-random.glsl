// lib-random.glsl: blue noise thresholds and Fibonacci sample sequences.

//: param auto random_seed
uniform int alg_random_seed;

const ivec2 texture_blue_noise_size = ivec2(256, 256);

// ponytail: no blue-noise texture is bound; interleaved gradient noise on the
// pixel position stands in for it (similar spectrum, no texture fetch).
float getBlueNoiseThreshold() {
  vec2 p = floor(gl_FragCoord.xy);
  return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715))));
}

float getBlueNoiseThresholdTemporal() {
  return fract(getBlueNoiseThreshold() + M_GOLDEN_RATIO * float(alg_random_seed));
}

float fibonacci1D(int i) {
  return fract((float(i) + 1.0) * M_GOLDEN_RATIO);
}

vec2 fibonacci2D(int i, int nbSamples) {
  return vec2((float(i) + 0.5) / float(nbSamples), fibonacci1D(i));
}

vec2 fibonacci2DDitheredTemporal(int i, int nbSamples) {
  vec2 s = fibonacci2D(i, nbSamples);
  s.x = fract(s.x + getBlueNoiseThresholdTemporal());
  return s;
}
