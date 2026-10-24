// Cutout foliage: parallax on the displacement channel, alpha test, blending.
import lib-pbr.glsl
import lib-sampler.glsl
import lib-alpha.glsl
import lib-pom.glsl

//: state blend over
//: state cull_face off

//: param auto channel_basecolor
uniform SamplerSparse basecolor_tex;
//: param auto channel_roughness
uniform SamplerSparse roughness_tex;
//: param auto channel_opacity
uniform SamplerSparse opacity_channel;

//: param custom { "default": true, "label": "Cutout" }
uniform bool cutout;

void shade(V2F inputs)
{
  vec3 viewTS = worldSpaceToTangentSpace(getEyeVec(inputs.position), inputs);
  applyParallaxOffset(inputs, viewTS);

  vec3 baseColor = getBaseColor(basecolor_tex, inputs.sparse_coord);
  float roughness = getRoughness(roughness_tex, inputs.sparse_coord);
  float opacity = getOpacity(opacity_channel, inputs.sparse_coord);
  LocalVectors vectors = computeLocalFrame(inputs);

  // Sample everything before discarding.
  if (cutout) {
    alphaKill(inputs.sparse_coord);
  }

  albedoOutput(baseColor);
  diffuseShadingOutput(envIrradiance(vectors.normal));
  specularShadingOutput(pbrComputeSpecular(vectors, vec3(0.04), roughness));
  alphaOutput(opacity);
}
