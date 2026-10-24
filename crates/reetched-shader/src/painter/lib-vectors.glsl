// lib-vectors.glsl: view vector and local shading frame.

//: param auto is_2d_view
uniform bool is2DView;
//: param auto is_perspective_projection
uniform bool is_perspective;
//: param auto world_eye_position
uniform vec3 camera_pos;
//: param auto world_camera_direction
uniform vec3 camera_dir;
//: param auto facing
uniform int facing;

struct LocalVectors {
  vec3 vertexNormal;
  vec3 tangent;
  vec3 bitangent;
  vec3 normal;
  vec3 eye;
};

bool isBackFace() {
  return facing == -1 || (facing == 0 && !gl_FrontFacing);
}

vec3 getEyeVec(vec3 position) {
  return is_perspective ? normalize(camera_pos - position) : -camera_dir;
}

vec3 tangentSpaceToWorldSpace(vec3 vecTS, V2F inputs) {
  return normalize(vecTS.x * inputs.tangent + vecTS.y * inputs.bitangent + vecTS.z * inputs.normal);
}

vec3 worldSpaceToTangentSpace(vec3 vecWS, V2F inputs) {
  return vec3(dot(vecWS, inputs.tangent), dot(vecWS, inputs.bitangent), dot(vecWS, inputs.normal));
}

LocalVectors computeLocalFrame(V2F inputs, vec3 normal, float anisoAngle) {
  LocalVectors vectors;
  vectors.vertexNormal = inputs.normal;
  vectors.normal = normal;
  if (isBackFace()) {
    vectors.vertexNormal = -vectors.vertexNormal;
    vectors.normal = -vectors.normal;
  }
  vectors.eye = is2DView ? vectors.normal : getEyeVec(inputs.position);

  // Tangent frame around the shading normal, rotated by the anisotropy angle.
  vec3 t = inputs.tangent - vectors.normal * dot(vectors.normal, inputs.tangent);
  if (dot(t, t) < 1e-12) {
    t = inputs.tangent;
  }
  t = normalize(t);
  vec3 b = cross(vectors.normal, t) * (dot(cross(inputs.normal, inputs.tangent), inputs.bitangent) < 0.0 ? -1.0 : 1.0);
  float c = cos(anisoAngle);
  float s = sin(anisoAngle);
  vectors.tangent = c * t + s * b;
  vectors.bitangent = c * b - s * t;
  return vectors;
}

LocalVectors computeLocalFrame(V2F inputs) {
  vec3 normal = computeWSNormal(inputs.sparse_coord, inputs.tangent, inputs.bitangent, inputs.normal);
  return computeLocalFrame(inputs, normal, 0.0);
}
