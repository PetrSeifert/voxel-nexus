#version 450

layout(push_constant) uniform CameraConstants {
    mat4 view_projection;
    vec4 volume_origin_and_voxel_size;
    ivec4 core_origin;
    vec4 eye_and_far_clip;
    vec4 fog_range_and_edge_shading;
} camera;

layout(set = 0, binding = 0, std430) readonly buffer MaterialTable {
    vec4 colors[];
} materials;

layout(location = 0) in uvec4 packed_vertex;
layout(location = 0) out vec3 fragment_normal;
layout(location = 1) out vec4 fragment_linear_base_color;
layout(location = 2) out vec3 fragment_eye_offset;
layout(location = 3) flat out float fragment_far_clip;
layout(location = 4) out vec3 fragment_voxel_coordinate;
layout(location = 5) flat out vec3 fragment_fog_range_and_edge_shading;

void main() {
    const vec3 normals[6] = vec3[6](
        vec3(-1, 0, 0), vec3(1, 0, 0),
        vec3(0, -1, 0), vec3(0, 1, 0),
        vec3(0, 0, -1), vec3(0, 0, 1));
    vec3 coordinate = vec3(camera.core_origin.xyz + ivec3(packed_vertex.xyz));
    vec3 position = camera.volume_origin_and_voxel_size.xyz
        + coordinate * camera.volume_origin_and_voxel_size.w;
    gl_Position = camera.view_projection * vec4(position, 1.0);
    fragment_eye_offset = position - camera.eye_and_far_clip.xyz;
    fragment_far_clip = camera.eye_and_far_clip.w;
    fragment_voxel_coordinate = coordinate;
    fragment_fog_range_and_edge_shading = camera.fog_range_and_edge_shading.xyz;
    fragment_normal = normals[packed_vertex.w & 7u];
    fragment_linear_base_color = materials.colors[packed_vertex.w >> 3u];
}
