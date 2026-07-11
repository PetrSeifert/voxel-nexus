#version 450

layout(push_constant) uniform CameraConstants {
    mat4 view_projection;
} camera;

layout(location = 0) in vec3 position;
layout(location = 1) in vec3 normal;
layout(location = 2) in vec4 linear_base_color;

layout(location = 0) out vec3 fragment_normal;
layout(location = 1) out vec4 fragment_linear_base_color;

void main() {
    gl_Position = camera.view_projection * vec4(position, 1.0);
    fragment_normal = normal;
    fragment_linear_base_color = linear_base_color;
}
