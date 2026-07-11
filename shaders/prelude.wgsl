// Shared prelude: view uniforms + noise toolkit. Concatenated ahead of each stage.

struct View {
    size: vec4<f32>,   // w, h, anim_time_s, unused
    fog_a: vec4<f32>,  // screen px -> galactic-plane kpc: (ax, bx, tx, w_gal)
    fog_b: vec4<f32>,  // (ay, by, ty, fog_dim)
    cos_a: vec4<f32>,  // screen px -> supergalactic Mpc: (ax, bx, tx, w_cosmic)
    cos_b: vec4<f32>,  // (ay, by, ty, unused)
    helio: vec4<f32>,  // sun sx, sy, px_per_au, w_helio
    grav_a: vec4<f32>, // primary sx, sy, mu, sep_px
    grav_b: vec4<f32>, // secondary sx, sy, w_grav, contour_gain
    misc: vec4<f32>,   // w_deep, deep_drift, seed, w_speckle_gal
}
@group(0) @binding(0) var<uniform> V: View;

const PI: f32 = 3.14159265;

// Magnitude-robust hashing: PCG mix over the raw float bits, so noise keeps
// full entropy at galactic/cosmic coordinate scales where f32 fract() dies.
fn pcg(v0: u32) -> u32 {
    var v = v0 * 747796405u + 2891336453u;
    v = ((v >> ((v >> 28u) + 4u)) ^ v) * 277803737u;
    return (v >> 22u) ^ v;
}
fn hash21(p: vec2<f32>) -> f32 {
    let h = pcg(bitcast<u32>(p.x) ^ pcg(bitcast<u32>(p.y)));
    return f32(h & 0x00ffffffu) / 16777216.0;
}
fn hash31(p: vec3<f32>) -> f32 {
    let h = pcg(bitcast<u32>(p.x) ^ pcg(bitcast<u32>(p.y) ^ pcg(bitcast<u32>(p.z))));
    return f32(h & 0x00ffffffu) / 16777216.0;
}
fn vnoise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
fn vnoise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash31(i);
    let n100 = hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash31(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}
fn fbm2(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 5; i = i + 1) {
        v = v + a * vnoise2(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.7);
        a = a * 0.5;
    }
    return v;
}
fn fbm3(p: vec3<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + a * vnoise3(q);
        q = q * 2.07 + vec3<f32>(13.5, 7.2, 3.1);
        a = a * 0.5;
    }
    return v;
}
fn ridge2(p: vec2<f32>) -> f32 {
    return 1.0 - abs(2.0 * fbm2(p) - 1.0);
}
