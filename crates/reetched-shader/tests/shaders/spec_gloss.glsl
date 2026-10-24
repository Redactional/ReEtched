// Specular/glossiness workflow.
import lib-sampler.glsl
import lib-pbr.glsl

//: param auto channel_diffuse
uniform SamplerSparse diffuse_tex;
//: param auto channel_specular
uniform SamplerSparse specular_tex;
//: param auto channel_glossiness
uniform SamplerSparse glossiness_tex;

//: state cull_face off

void shade(V2F inputs)
{
  vec3 diffColor = getDiffuse(diffuse_tex, inputs.sparse_coord);
  vec3 specColor = getSpecularColor(specular_tex, inputs.sparse_coord);
  float glossiness = getGlossiness(glossiness_tex, inputs.sparse_coord);
  float roughness = 1.0 - glossiness;

  LocalVectors vectors = computeLocalFrame(inputs);
  float occlusion = getAO(inputs.sparse_coord);

  albedoOutput(diffColor);
  diffuseShadingOutput(occlusion * pbrComputeDiffuse(vectors.normal, vec3(1.0)));
  specularShadingOutput(occlusion * pbrComputeSpecular(vectors, specColor, roughness));
}
