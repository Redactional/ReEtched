// lib-bayer.glsl: ordered dithering.

// 8x8 Bayer matrix value built by interleaving the bits of (x ^ y, y), lowest
// coordinate bits becoming the highest result bits. Result is in (0, 1).
float bayerMatrix8(uvec2 coords) {
  uint x = coords.x ^ coords.y;
  uint y = coords.y;
  uint v = 0u;
  for (int b = 0; b < 3; b++) {
    v = (v << 2u) | (((x >> uint(b)) & 1u) << 1u) | ((y >> uint(b)) & 1u);
  }
  return (float(v) + 0.5) / 64.0;
}
