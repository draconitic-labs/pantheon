// Fullscreen ACES tonemap: HDR canvas → display sRGB. Luminous ink can now
// accumulate past 1.0 (bulge, glows, star cores) and roll off filmically
// instead of clipping.

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var hdr_smp: sampler;
@group(0) @binding(2) var bloom_tex: texture_2d<f32>;

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

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(hdr_tex, hdr_smp, in.uv).rgb * 1.12; // exposure
    c = c + textureSample(bloom_tex, hdr_smp, in.uv).rgb * 0.85;
    var col = aces(c);
    // gentle vignette keeps the eye in the chart
    let r = length(in.uv - vec2<f32>(0.5)) * 1.4142;
    col = col * (1.0 - 0.13 * smoothstep(0.55, 1.05, r));
    // fine static grain breaks banding on the deep gradients
    let g = fract(sin(dot(in.uv, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    col = col + vec3<f32>((g - 0.5) * 0.012);
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
