// Deliberately broken: line 12 adds a vec3 to a vec2.
import lib-sampler.glsl

//: param auto channel_basecolor
uniform SamplerSparse basecolor_tex;

void shade(V2F inputs)
{
  vec3 baseColor = getBaseColor(basecolor_tex, inputs.sparse_coord);
  float k = 2.0;
  k *= 0.5;
  vec2 bad = baseColor + inputs.tex_coord;
  diffuseShadingOutput(baseColor * k);
}
