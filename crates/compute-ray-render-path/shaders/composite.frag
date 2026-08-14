#version 450

layout(set = 0, binding = 1) uniform sampler2D computed_image;
layout(location = 0) in vec2 texture_coordinate;
layout(location = 0) out vec4 output_color;

void main() {
    output_color = texture(computed_image, texture_coordinate);
}
