#version 450

const vec2 positions[3] = vec2[](
    vec2(-0.7, 0.6),
    vec2(0.7, 0.6),
    vec2(0.0, -0.7)
);

void main() {
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
}
