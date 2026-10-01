#version 450

layout(location = 0) in vec3 fragment_normal;
layout(location = 1) in vec4 fragment_linear_base_color;
layout(location = 2) in vec3 fragment_eye_offset;
layout(location = 3) flat in float fragment_far_clip;
layout(location = 4) in vec3 fragment_voxel_coordinate;
layout(location = 5) flat in vec3 fragment_fog_range_and_edge_shading;
layout(location = 0) out vec4 output_color;

// Matches PresentationStyle::DISTANCE_FOG_LINEAR_COLOR and the compute path's copy.
const vec3 DISTANCE_FOG_COLOR = vec3(0.42, 0.56, 0.74);

// Matches the compute path's copy so switching paths does not change a voxel's shade.
float voxel_edge_shade(vec3 voxel_coordinate, vec3 normal) {
    vec3 within = fract(voxel_coordinate);
    vec3 edge_distance = min(within, 1.0 - within);
    // The face lies on an integer plane along its normal, so only the two tangent axes count.
    edge_distance = mix(edge_distance, vec3(1.0), abs(normal));
    float nearest = min(edge_distance.x, min(edge_distance.y, edge_distance.z));
    return mix(0.7, 1.0, smoothstep(0.0, 0.1, nearest));
}

void main() {
    float distance = length(fragment_eye_offset);
    if (fragment_far_clip > 0.0 && distance > fragment_far_clip) {
        discard;
    }
    vec3 normal = normalize(fragment_normal);
    vec3 light_direction = normalize(vec3(0.4, 0.8, 0.6));
    float lighting = 0.35 + 0.65 * max(dot(normal, light_direction), 0.0);
    vec3 color = fragment_linear_base_color.rgb * lighting;
    if (fragment_fog_range_and_edge_shading.z > 0.0) {
        color *= voxel_edge_shade(fragment_voxel_coordinate, normal);
    }
    float fog_end = fragment_fog_range_and_edge_shading.y;
    if (fog_end > 0.0) {
        float fog = smoothstep(fragment_fog_range_and_edge_shading.x, fog_end, distance);
        color = mix(color, DISTANCE_FOG_COLOR, fog);
    }
    output_color = vec4(color, fragment_linear_base_color.a);
}
