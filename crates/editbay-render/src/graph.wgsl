struct Parameters {
    mode: vec4f,
    x: vec4f,
    y: vec4f,
    solid: vec4f,
    red: vec4f,
    green: vec4f,
    blue: vec4f,
}
@group(0) @binding(0) var first: texture_2d<f32>;
@group(0) @binding(1) var second: texture_2d<f32>;
@group(0) @binding(2) var destination: texture_storage_2d<OUTPUT_FORMAT, write>;
@group(0) @binding(3) var<uniform> p: Parameters;
@group(0) @binding(4) var mask: texture_2d<f32>;
@group(0) @binding(5) var<storage, read> vertices: array<vec2f>;

fn coverage(at: vec2f) -> f32 {
    let count = u32(p.solid.z);
    var inside = false;
    var distance_squared = 1e30;
    for (var i = 0u; i < count; i += 1u) {
        let a = vertices[i];
        let b = vertices[(i + 1u) % count];
        let edge = b - a;
        let length_squared = dot(edge, edge);
        var projection = 0.0;
        if length_squared > 0.0 { projection = clamp(dot(at - a, edge) / length_squared, 0.0, 1.0); }
        let delta = at - (a + projection * edge);
        distance_squared = min(distance_squared, dot(delta, delta));
        if (a.y > at.y) != (b.y > at.y) {
            let crossing = a.x + (at.y - a.y) * edge.x / edge.y;
            if at.x < crossing { inside = !inside; }
        }
    }
    var alpha = 0.0;
    if count > 0u {
        let distance = sqrt(distance_squared) * select(-1.0, 1.0, inside);
        alpha = smoothstep(-p.solid.x * 0.5, p.solid.x * 0.5, distance);
    }
    return select(alpha, 1.0 - alpha, p.solid.y == 1.0);
}

fn gamut(v: vec3f) -> vec3f {
    return vec3f(dot(p.red.xyz, v), dot(p.green.xyz, v), dot(p.blue.xyz, v));
}
fn decode(v: vec3f) -> vec3f {
    if p.mode.y == 1.0 {
        return select(pow(max((v + 0.055) / 1.055, vec3f(0.0)), vec3f(2.4)), v / 12.92, v <= vec3f(0.04045));
    }
    if p.mode.y == 2.0 {
        return select(pow(max((v + 0.099) / 1.099, vec3f(0.0)), vec3f(1.0 / 0.45)), v / 4.5, v < vec3f(0.081));
    }
    return v;
}
fn encode(v: vec3f) -> vec3f {
    if p.mode.y == 1.0 {
        return select(1.055 * pow(max(v, vec3f(0.0)), vec3f(1.0 / 2.4)) - 0.055, v * 12.92, v <= vec3f(0.0031308));
    }
    if p.mode.y == 2.0 {
        return select(1.099 * pow(max(v, vec3f(0.0)), vec3f(0.45)) - 0.099, v * 4.5, v < vec3f(0.018));
    }
    return v;
}
fn fetch(q: vec2i, source: bool) -> vec4f {
    let size = vec2i(textureDimensions(first));
    if any(q < vec2i(0)) || any(q >= size) { return vec4f(0.0); }
    let value = textureLoad(first, q, 0);
    if !source { return value; }
    var alpha = value.a;
    if p.mode.z == 0.0 { alpha = 1.0; }
    if alpha <= 0.0 { return vec4f(0.0); }
    var straight = value.rgb;
    if p.mode.z == 2.0 { straight /= alpha; }
    return vec4f(gamut(decode(straight)) * alpha, alpha);
}
fn bilinear(q: vec2f, source: bool) -> vec4f {
    let at = q - 0.5;
    let origin = vec2i(floor(at));
    let weight = fract(at);
    return mix(mix(fetch(origin, source), fetch(origin + vec2i(1, 0), source), weight.x),
               mix(fetch(origin + vec2i(0, 1), source), fetch(origin + vec2i(1, 1), source), weight.x), weight.y);
}
@compute @workgroup_size(16, 16)
fn evaluate(@builtin(global_invocation_id) id: vec3u) {
    let size = textureDimensions(destination);
    if any(id.xy >= size) { return; }
    let pixel = vec2i(id.xy);
    let at = vec3f(vec2f(id.xy) + 0.5, 1.0);
    var value = vec4f(0.0);
    if p.mode.x == 0.0 { value = vec4f(p.solid.rgb * p.solid.a, p.solid.a); }
    if p.mode.x == 1.0 || p.mode.x == 2.0 || p.mode.x == 5.0 {
        let q = vec2f(dot(p.x.xyz, at), dot(p.y.xyz, at));
        value = bilinear(q, p.mode.x == 1.0) * p.mode.w;
        if p.mode.x == 5.0 { value = vec4f(gamut(value.rgb), value.a); }
    }
    if p.mode.x == 3.0 || p.mode.x == 8.0 {
        var foreground = textureLoad(first, pixel, 0);
        if p.mode.x == 8.0 { foreground *= textureLoad(mask, pixel, 0).a; }
        let background = textureLoad(second, pixel, 0);
        value = foreground + background * (1.0 - foreground.a);
    }
    if p.mode.x == 4.0 { value = textureLoad(first, pixel, 0) * p.mode.w; }
    if p.mode.x == 6.0 {
        let input = textureLoad(first, pixel, 0);
        if input.a > 0.0 { value = vec4f(encode(gamut(input.rgb / input.a)) * input.a, input.a); }
    }
    if p.mode.x == 7.0 { value = vec4f(coverage(at.xy)); }
    if p.mode.x == 9.0 {
        let input = textureLoad(first, pixel, 0);
        if input.a > 0.0 {
            let exposed = input.rgb / input.a * p.solid.x;
            let contrasted = (exposed - vec3f(0.18)) * p.solid.y + vec3f(0.18);
            let luminance = dot(p.red.xyz, contrasted);
            value = vec4f(mix(vec3f(luminance), contrasted, p.solid.z) * input.a, input.a);
        }
    }
    textureStore(destination, pixel, value);
}
