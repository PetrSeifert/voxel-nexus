#version 450

layout(location = 0) in vec3 fragment_normal;
layout(location = 1) in vec4 fragment_linear_base_color;
layout(location = 2) in vec3 fragment_eye_offset;
layout(location = 3) flat in float fragment_far_clip;
layout(location = 0) out vec4 output_color;

void main() {
    if (fragment_far_clip > 0.0 && length(fragment_eye_offset) > fragment_far_clip) {
        discard;
    }
    vec3 light_direction = normalize(vec3(0.4, 0.8, 0.6));
    float lighting = 0.35 + 0.65 * max(dot(normalize(fragment_normal), light_direction), 0.0);
    output_color = vec4(fragment_linear_base_color.rgb * lighting, fragment_linear_base_color.a);
}
