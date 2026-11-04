// Individual galaxies drawn as light, oriented by their measured position angle
// and inclination where known. The appearance is illustrative (see evidence):
// spiral arms, dust, and clumps are procedural; the position, size, and tilt are
// from the catalog.

struct Inst {
    @location(0) center: vec2<f32>,
    @location(1) m: vec4<f32>,     // 2x2: screen px -> galaxy plane (unit = galaxy radius)
    @location(2) shape: vec4<f32>, // radius_px, kind (0 spiral, 1 irregular, 2 spheroidal), seed, quad_half
    @location(3) tint: vec4<f32>,  // arm rgb, brightness
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) m: vec4<f32>,
    @location(2) shape: vec4<f32>,
    @location(3) tint: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let c = corners[vi] * inst.shape.w;
    let p = inst.center + c;
    var o: VsOut;
    o.clip = vec4<f32>(p.x / V.size.x * 2.0 - 1.0, 1.0 - p.y / V.size.y * 2.0, 0.0, 1.0);
    o.local = c;
    o.m = inst.m;
    o.shape = inst.shape;
    o.tint = inst.tint;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let g = vec2<f32>(
        in.m.x * in.local.x + in.m.y * in.local.y,
        in.m.z * in.local.x + in.m.w * in.local.y);
    let r = length(g);
    let rpx = max(in.shape.x, 0.5);
    let seed = in.shape.z;
    let kind = i32(in.shape.y + 0.5);
    let warm = vec3<f32>(1.0, 0.82, 0.58);
    var col = vec3<f32>(0.0);
    // a bulge is a 3D spheroid: it stays round however the disc is tilted
    let rs = length(in.local) / rpx;
    let fade = 1.0 - smoothstep(0.75, 1.0, r);
    if (kind == 0) {
        let detail = smoothstep(4.0, 22.0, rpx);
        let lr = log(max(r, 0.015) / 0.12) / 0.27;
        let th = atan2(g.y, g.x);
        let warp = (fbm2(g * 3.1 + seed * 19.0) - 0.5) * 2.2;
        let arm = pow(0.5 + 0.5 * cos(2.0 * th - 2.0 * lr + warp + seed * 6.0), 2.6);
        let clump = 0.55 + 0.9 * fbm2(g * 9.0 + seed * 7.0);
        let dust = smoothstep(0.52, 0.72, fbm2(g * 6.0 - seed * 3.0));
        let disc = exp(-r * 3.3);
        let arms = mix(0.5, arm * clump * (1.0 - 0.6 * dust), detail);
        col = in.tint.rgb * arms * disc * fade * 1.5
            + mix(warm, in.tint.rgb, 0.4) * disc * fade * 0.28;
        col = col + warm * (exp(-rs * rs * 110.0) * 3.2 + exp(-rs * 15.0) * 0.55);
    } else if (kind == 1) {
        let n = fbm2(g * 3.4 + seed * 11.0);
        let body = exp(-r * r * 3.6) * (0.45 + 1.1 * n);
        let clumps = smoothstep(0.58, 0.78, fbm2(g * 8.0 + seed * 5.0));
        col = in.tint.rgb * body * 1.1 + vec3<f32>(1.0, 0.55, 0.66) * clumps * body * 0.8;
        col = col + warm * exp(-rs * rs * 40.0) * 0.4;
    } else if (kind == 2) {
        // spheroidal dwarfs: a faint, smooth, old stellar population
        let body = exp(-pow(r * 2.4, 1.25));
        col = mix(warm, vec3<f32>(0.8, 0.82, 0.95), 0.35) * body * fade * 0.5;
    } else {
        // a galaxy's extended halo: no sharp edge, the faintest possible glow
        let n = fbm2(g * 3.0 + seed * 13.0);
        col = in.tint.rgb * exp(-r * r * 4.2) * (0.72 + 0.56 * n) * 0.10;
    }
    return vec4<f32>(col * in.tint.a, 0.0);
}
