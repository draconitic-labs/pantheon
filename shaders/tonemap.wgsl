// HDR canvas to display sRGB. AgX keeps hue through bright highlights (a star
// goes to white through its own colour, not through magenta), bloom is
// composited in linear light, and interleaved-gradient dither hides banding on
// the deep gradients without visible grain.

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

// AgX, polynomial fit (Benjamin Wrensch, MIT-licensed reference implementation)
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2
        + 0.1191 * x - 0.00232;
}

fn agx(c: vec3<f32>) -> vec3<f32> {
    let m = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    var v = m * c;
    let lo = -12.47393;
    let hi = 4.026069;
    v = clamp(log2(max(v, vec3<f32>(1e-10))), vec3<f32>(lo), vec3<f32>(hi));
    v = (v - lo) / (hi - lo);
    return agx_contrast(v);
}

fn agx_look(v: vec3<f32>) -> vec3<f32> {
    // the "punchy" grade: deeper blacks, richer colour for bodies and nebulae
    let w = vec3<f32>(0.2126, 0.7152, 0.0722);
    let p = pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.32));
    let lp = dot(p, w);
    return lp + 1.28 * (p - lp);
}

fn agx_eotf(v: vec3<f32>) -> vec3<f32> {
    let m = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    return pow(max(m * v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(hdr_tex, hdr_smp, in.uv).rgb;
    c = c + textureSample(bloom_tex, hdr_smp, in.uv).rgb * 0.16;
    c = max(c, vec3<f32>(0.0)) * 1.55; // exposure into AgX's middle grey
    var col = agx_eotf(agx_look(agx(c)));
    // a light vignette keeps the eye in the chart
    let r = length(in.uv - vec2<f32>(0.5)) * 1.4142;
    col = col * (1.0 - 0.10 * smoothstep(0.6, 1.1, r));
    // target is an sRGB view; dither in the output's own steps
    let n = ign(in.clip.xy) - 0.5;
    col = col + vec3<f32>(n * 0.0022) * (0.35 + 2.0 * sqrt(max(col, vec3<f32>(0.0))));
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
