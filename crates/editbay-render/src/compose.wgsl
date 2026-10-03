@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<OUTPUT_FORMAT, write>;

fn linear(value: vec3f) -> vec3f {
    return INPUT_LINEAR;
}

@compute @workgroup_size(16, 16)
fn compose(@builtin(global_invocation_id) position: vec3u) {
    let size = textureDimensions(source);
    if position.x >= size.x || position.y >= size.y { return; }
    let input = textureLoad(source, vec2i(position.xy), 0);
    let alpha = input.a * 0.8;
    let rgb = linear(input.rgb) * 1.5 * alpha + vec3f(0.02) * (1.0 - alpha);
    textureStore(destination, vec2i(position.xy), vec4f(rgb, 1.0));
}
