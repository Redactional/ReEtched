// lib-alpha.glsl: alpha test.

//: param auto channel_opacity
uniform SamplerSparse opacity_tex;
//: param custom { "default": 0.33, "label": "Alpha threshold", "min": 0.0, "max": 1.0, "group": "Common Parameters" }
uniform float alpha_threshold;
//: param custom { "default": false, "label": "Alpha dithering", "group": "Common Parameters" }
uniform bool alpha_dither;

// Call after all texture sampling: discarding breaks derivatives.
void alphaKill(float alpha) {
  float threshold = alpha_dither ? getBlueNoiseThresholdTemporal() : alpha_threshold;
  if (alpha < threshold) {
    discard;
  }
}

void alphaKill(SparseCoord coord) {
  alphaKill(getOpacity(opacity_tex, coord));
}
