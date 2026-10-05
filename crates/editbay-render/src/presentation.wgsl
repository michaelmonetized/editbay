@group(0) @binding(0) var picture: texture_2d<f32>;

struct Vertex { @builtin(position) position: vec4f, @location(0) uv: vec2f };
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let positions = array<vec2f, 3>(vec2f(-1, -1), vec2f(-1, 3), vec2f(3, -1));
    let uv = array<vec2f, 3>(vec2f(0, 1), vec2f(0, -1), vec2f(2, 1));
    return Vertex(vec4f(positions[index], 0, 1), uv[index]);
}
fn load_at(position: vec2i, size: vec2i) -> vec4f {
    return textureLoad(picture, clamp(position, vec2i(0), size - vec2i(1)), 0);
}
@fragment fn fragment(input: Vertex) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(picture));
    let position = input.uv * vec2f(size) - vec2f(0.5);
    let low = vec2i(floor(position));
    let fraction = fract(position);
    let upper = mix(load_at(low, size), load_at(low + vec2i(1, 0), size), fraction.x);
    let lower = mix(load_at(low + vec2i(0, 1), size), load_at(low + vec2i(1, 1), size), fraction.x);
    let pixel = mix(upper, lower, fraction.y);
    let checker = (u32(input.position.x / 16) + u32(input.position.y / 16)) % 2;
    let background = select(0.16, 0.22, checker == 1);
    let encoded = clamp(pixel.rgb + vec3f(background) * (1 - clamp(pixel.a, 0, 1)), vec3f(0), vec3f(1));
    if SURFACE_SRGB {
        let linear = select(pow((encoded + 0.055) / 1.055, vec3f(2.4)), encoded / 12.92, encoded <= vec3f(0.04045));
        return vec4f(linear, 1);
    }
    return vec4f(encoded, 1);
}
