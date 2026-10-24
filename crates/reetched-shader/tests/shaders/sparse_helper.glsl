// User helpers taking SamplerSparse, Painter's `sampler` parameter name,
// `.is_set` / `.size`, raw textureSparse reads and a mix of engine params.
import lib-sampler.glsl
import lib-normal.glsl
import lib-env.glsl
import lib-defines.glsl
import lib-random.glsl

//: param auto channel_basecolor
uniform SamplerSparse basecolor_tex;
//: param auto channel_height
uniform SamplerSparse height_tex;
//: param auto channel_user0
uniform SamplerSparse mask_tex;
//: param auto world_eye_position
uniform vec3 camera_pos;
//: param auto camera_view_matrix
uniform mat4 view_mat;
//: param auto environment_exposure
uniform float exposure;
//: param auto time
uniform float seconds;
//: param auto channel_basecolor_size
uniform vec4 basecolor_size;
//: param auto texture_blue_noise_size
uniform vec4 noise_size;

//: param custom { "default": 1.0, "label": "Edge boost", "min": 0.0, "max": 4.0 }
uniform float edge_boost;

#define DISABLE_FRAMEBUFFER_SRGB_CONVERSION

vec3 sampleTinted(SamplerSparse sampler, SparseCoord coord, vec3 tint)
{
  if (!sampler.is_set) {
    return tint;
  }
  vec4 raw = textureSparse(sampler, coord);
  return raw.rgb * tint + vec3(0.5) * (1.0 - raw.a);
}

float texelEdge(SamplerSparse s, SparseCoord coord)
{
  vec2 texel = s.size.zw;
  vec2 offsets[2] = vec2[2](vec2(texel.x, 0.0), vec2(0.0, texel.y));
  vec4 results[2];
  textureSparseOffsets(s, coord, offsets, results);
  vec4 center = textureSparse(s, coord);
  return length(results[0].r - center.r) + length(results[1].r - center.r);
}

void shade(V2F inputs)
{
  vec3 base = sampleTinted(basecolor_tex, inputs.sparse_coord, vec3(1.0, 0.9, 0.8));
  float edge = texelEdge(height_tex, inputs.sparse_coord) * edge_boost;
  float mask = mask_tex.is_set ? textureSparse(mask_tex, inputs.sparse_coord).r : 1.0;
  vec3 n = getTSNormal(inputs.sparse_coord);
  vec3 viewN = mat3(view_mat) * n;
  float pulse = 0.5 + 0.5 * sin(seconds * M_2PI);
  float noise = getBlueNoiseThreshold();
  vec3 lit = base * envIrradiance(inputs.normal) * exposure;
  lit += vec3(edge + 0.01 * noise * pulse + 0.0 * viewN.z + 0.0 * basecolor_size.x + 0.0 * noise_size.x);
  diffuseShadingOutput(lit * mask);
  emissiveColorOutput(vec3(0.0));
}
