// Banded toon shading with an outline, a paper texture and an unlit mode.
import lib-sampler.glsl
import lib-vectors.glsl

//: param auto channel_basecolor
uniform SamplerSparse basecolor_tex;
//: param auto texture_curvature
uniform SamplerSparse curvature_tex;
//: param auto main_light
uniform vec4 light_main;

//: param custom { "default": 4, "label": "Bands", "min": 1, "max": 8, "step": 1 }
uniform int bands;
//: param custom { "default": [0.1, 0.05, 0.2], "label": "Outline color", "widget": "color" }
uniform vec3 outline_color;
//: param custom { "default": 0.25, "label": "Outline width", "min": 0.0, "max": 1.0 }
uniform float outline_width;
//: param custom { "default": false, "label": "Unlit", "description": "Skip lighting entirely" }
uniform bool unlit;
//: param custom {
//:   "default": 0,
//:   "label": "Paper mode",
//:   "widget": "combobox",
//:   "values": { "Off": 0, "Multiply": 1, "Overlay": 2 },
//:   "group": "Paper"
//: }
uniform int paper_mode;
//: param custom { "default": [4.0, 4.0], "label": "Paper tiling", "group": "Paper" }
uniform vec2 paper_tiling;
//: param custom { "default": "paper_grain", "label": "Paper", "usage": "texture", "default_color": [1.0, 1.0, 1.0, 1.0], "group": "Paper" }
uniform sampler2D paper_tex;

const float LIGHT_WRAP = 0.2;

void shade(V2F inputs)
{
  vec3 baseColor = getBaseColor(basecolor_tex, inputs.sparse_coord);
  vec4 paper = texture(paper_tex, inputs.tex_coord * paper_tiling);
  if (paper_mode == 1) {
    baseColor *= paper.rgb;
  } else if (paper_mode == 2) {
    baseColor = mix(2.0 * baseColor * paper.rgb, vec3(1.0) - 2.0 * (vec3(1.0) - baseColor) * (vec3(1.0) - paper.rgb), step(0.5, baseColor));
  }

  if (unlit) {
    diffuseShadingOutput(baseColor);
    return;
  }

  vec3 N = normalize(inputs.normal);
  vec3 V = getEyeVec(inputs.position);
  vec3 L = length(light_main.xyz) > 0.0 ? normalize(light_main.xyz) : normalize(vec3(1, 1, 1));

  float curvature = textureSparse(curvature_tex, inputs.sparse_coord).r;
  float rim = dot(N, V);
  if (rim < outline_width * (curvature_tex.is_set ? 1.5 - curvature : 1.0)) {
    diffuseShadingOutput(outline_color);
    return;
  }

  float lambert = clamp((dot(N, L) + LIGHT_WRAP) / (1.0 + LIGHT_WRAP), 0.0, 1.0);
  float banded = floor(lambert * float(bands) + 0.5) / float(bands);
  diffuseShadingOutput(baseColor * max(banded, 0.1));
}
