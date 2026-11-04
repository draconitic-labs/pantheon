// Mip-chain bloom (dual filter): a soft knee keeps thin ink lines and faint fog
// out of it, the 13-tap downsample keeps single-pixel stars from flickering, and
// the tent upsample spreads energy into a wide, photographic glow.

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    var pts = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    var o: VsOut;
    o.clip = vec4<f32>(pts[vi], 0.0, 1.0);
    o.uv = vec2<f32>((pts[vi].x + 1.0) * 0.5, (1.0 - pts[vi].y) * 0.5);
    return o;
}

fn s(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src_tex, src_smp, uv, 0.0).rgb;
}

fn down13(uv: vec2<f32>) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src_tex, 0));
    let a = s(uv + t * vec2<f32>(-2.0, -2.0));
    let b = s(uv + t * vec2<f32>(0.0, -2.0));
    let c = s(uv + t * vec2<f32>(2.0, -2.0));
    let d = s(uv + t * vec2<f32>(-2.0, 0.0));
    let e = s(uv);
    let f = s(uv + t * vec2<f32>(2.0, 0.0));
    let g = s(uv + t * vec2<f32>(-2.0, 2.0));
    let h = s(uv + t * vec2<f32>(0.0, 2.0));
    let i = s(uv + t * vec2<f32>(2.0, 2.0));
    let j = s(uv + t * vec2<f32>(-1.0, -1.0));
    let k = s(uv + t * vec2<f32>(1.0, -1.0));
    let l = s(uv + t * vec2<f32>(-1.0, 1.0));
    let m = s(uv + t * vec2<f32>(1.0, 1.0));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

// first level: prefilter with a soft threshold
@fragment
fn fs_prefilter(in: VsOut) -> @location(0) vec4<f32> {
    var c = min(down13(in.uv), vec3<f32>(32.0));
    let lum = max(c.r, max(c.g, c.b));
    let knee = 0.45;
    let thresh = 0.7;
    let soft = clamp(lum - thresh + knee, 0.0, 2.0 * knee);
    let w = max(soft * soft / (4.0 * knee + 1e-4), lum - thresh) / max(lum, 1e-4);
    c = c * w;
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(down13(in.uv), 1.0);
}

// tent upsample, blended additively into the next larger level
@fragment
fn fs_up(in: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src_tex, 0));
    var c = s(uv_off(in.uv, t, -1.0, -1.0)) + s(uv_off(in.uv, t, 1.0, -1.0))
        + s(uv_off(in.uv, t, -1.0, 1.0)) + s(uv_off(in.uv, t, 1.0, 1.0));
    c = c + (s(uv_off(in.uv, t, 0.0, -1.0)) + s(uv_off(in.uv, t, -1.0, 0.0))
        + s(uv_off(in.uv, t, 1.0, 0.0)) + s(uv_off(in.uv, t, 0.0, 1.0))) * 2.0;
    c = c + s(in.uv) * 4.0;
    return vec4<f32>(c / 16.0 * 0.82, 1.0);
}

fn uv_off(uv: vec2<f32>, t: vec2<f32>, x: f32, y: f32) -> vec2<f32> {
    return uv + t * vec2<f32>(x, y);
}
