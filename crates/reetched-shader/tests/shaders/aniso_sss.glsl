// Brushed-metal anisotropy plus subsurface scattering for skin-like layers.
import lib-sampler.glsl
import lib-pbr.glsl
import lib-pbr-aniso.glsl
import lib-sss.glsl

//: param auto channel_basecolor
uniform SamplerSparse basecolor_tex;
//: param auto channel_roughness
uniform SamplerSparse roughness_tex;
//: param auto channel_metallic
uniform SamplerSparse metallic_tex;
//: param auto channel_anisotropylevel
uniform SamplerSparse anisotropylevel_tex;
//: param auto channel_anisotropyangle
uniform SamplerSparse anisotropyangle_tex;
//: param auto channel_scatteringcolor or channel_basecolor
uniform SamplerSparse scattering_color_tex;

void shade(V2F inputs)
{
  vec3 baseColor = getBaseColor(basecolor_tex, inputs.sparse_coord);
  float roughness = getRoughness(roughness_tex, inputs.sparse_coord);
  float metallic = getMetallic(metallic_tex, inputs.sparse_coord);
  float anisoLevel = getAnisotropyLevel(anisotropylevel_tex, inputs.sparse_coord);
  float anisoAngle = getAnisotropyAngle(anisotropyangle_tex, inputs.sparse_coord);

  vec3 normal = computeWSNormal(inputs.sparse_coord, inputs.tangent, inputs.bitangent, inputs.normal);
  LocalVectors vectors = computeLocalFrame(inputs, normal, anisoAngle);

  vec3 specColor = generateSpecularColor(baseColor, metallic);
  vec2 roughnessAniso = generateAnisotropicRoughness(roughness, anisoLevel);
  float occlusion = getAO(inputs.sparse_coord);

  albedoOutput(generateDiffuseColor(baseColor, metallic));
  diffuseShadingOutput(occlusion * envIrradiance(vectors.normal));
  specularShadingOutput(occlusion * pbrComputeSpecularAnisotropic(vectors, specColor, roughnessAniso));
  sssCoefficientsOutput(getSSSCoefficients(inputs.sparse_coord));
  sssColorOutput(getBaseColor(scattering_color_tex, inputs.sparse_coord));
}
