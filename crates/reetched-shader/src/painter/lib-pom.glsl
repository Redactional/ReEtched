// lib-pom.glsl: parallax occlusion mapping on the displacement channel.
// Convention: displacement 1.0 is the top, -1.0 the bottom; inward only.

//: param auto is_2d_view
uniform bool isTextureView;
//: param auto channel_displacement
uniform SamplerSparse displacement_tex;
//: param custom { "default": false, "label": "Enable", "group": "Parallax Occlusion Mapping" }
uniform bool usePOM;
//: param custom { "default": 1.0, "label": "Strength", "min": 0.01, "max": 10.0, "group": "Parallax Occlusion Mapping" }
uniform float pomStrength;
//: param custom { "default": 4, "label": "Minimum samples", "min": 1, "max": 64, "group": "Parallax Occlusion Mapping" }
uniform int minPOMSamples;
//: param custom { "default": 16, "label": "Maximum samples", "min": 1, "max": 64, "group": "Parallax Occlusion Mapping" }
uniform int maxPOMSamples;

vec2 getParallaxOffset(SparseCoord coord, vec3 viewTS) {
  if (!usePOM || isTextureView || !rx_is_set(displacement_tex)) {
    return vec2(0.0);
  }
  vec3 view = normalize(viewTS);
  float vz = max(abs(view.z), 1e-3);
  // More steps at grazing angles.
  int steps = int(mix(float(maxPOMSamples), float(minPOMSamples), vz));
  steps = max(steps, 1);
  float depthStep = 2.0 / float(steps);
  vec2 uvStep = -view.xy * (4.0 * pomStrength / (HEIGHT_FACTOR * vz * float(steps)));

  // March down from the top until the ray goes below the surface.
  SparseCoord c = coord;
  float rayHeight = 1.0;
  float prevRay = rayHeight;
  float surface = getDisplacement(textureSparse(displacement_tex, c));
  float prevSurface = surface;
  vec2 offset = vec2(0.0);
  for (int i = 0; i < steps && surface < rayHeight; ++i) {
    prevRay = rayHeight;
    prevSurface = surface;
    rayHeight -= depthStep;
    offset += uvStep;
    c.tex_coord = coord.tex_coord + offset;
    surface = getDisplacement(textureSparse(displacement_tex, c));
  }

  // Refine between the last two samples by intersecting the linear segments.
  vec2 offsetAbove = offset - uvStep;
  vec2 offsetBelow = offset;
  float rayAbove = prevRay;
  float rayBelow = rayHeight;
  float errAbove = prevSurface - prevRay;
  float errBelow = surface - rayHeight;
  for (int i = 0; i < 3; ++i) {
    float denom = errBelow - errAbove;
    float t = abs(denom) > 1e-6 ? -errAbove / denom : 0.5;
    t = clamp(t, 0.0, 1.0);
    vec2 o = mix(offsetAbove, offsetBelow, t);
    float r = mix(rayAbove, rayBelow, t);
    c.tex_coord = coord.tex_coord + o;
    float e = getDisplacement(textureSparse(displacement_tex, c)) - r;
    offset = o;
    if (abs(e) < 1e-3) {
      break;
    }
    if (e < 0.0) {
      offsetAbove = o; rayAbove = r; errAbove = e;
    } else {
      offsetBelow = o; rayBelow = r; errBelow = e;
    }
  }
  return offset;
}

void applyParallaxOffset(inout V2F inputs, vec3 viewTS) {
  vec2 offset = getParallaxOffset(inputs.sparse_coord, viewTS);
  if (offset != vec2(0.0)) {
    inputs.tex_coord += offset;
    inputs.sparse_coord = getSparseCoord(inputs.tex_coord);
  }
}
