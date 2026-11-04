// Planetary rings as one analytic disc: every fragment finds where its view ray
// meets the ring plane, reads the radial brightness/opacity profile, and tests
// the planet's shadow. The far half draws under the planet, the near half over.

struct Inst {
    @location(0) center: vec2<f32>,
    @location(1) r: vec4<f32>,    // radius_px, inner, outer (in planet radii), near flag
    @location(2) pole: vec4<f32>, // pole xyz (view space), depth
    @location(3) sun: vec4<f32>,  // sun xyz (view space), brightness
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) r: vec4<f32>,
    @location(2) pole: vec4<f32>,
    @location(3) sun: vec4<f32>,
}

@group(1) @binding(0) var tex_a: texture_2d<f32>;
@group(1) @binding(3) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let half_px = inst.r.x * inst.r.z * 1.03 + 2.0;
    let c = corners[vi] * half_px;
    let p = inst.center + c;
    var o: VsOut;
    o.clip = vec4<f32>(p.x / V.size.x * 2.0 - 1.0, 1.0 - p.y / V.size.y * 2.0, inst.pole.w, 1.0);
    o.local = c;
    o.r = inst.r;
    o.pole = inst.pole;
    o.sun = inst.sun;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let q = in.local / max(in.r.x, 0.5);
    let x = q.x;
    let y = -q.y;
    let pole = normalize(in.pole.xyz);
    let pz = select(min(pole.z, -0.004), max(pole.z, 0.004), pole.z >= 0.0);
    let z = -(x * pole.x + y * pole.y) / pz;
    let pos = vec3<f32>(x, y, z);
    let rho = length(pos);
    let u = (rho - in.r.y) / (in.r.z - in.r.y);
    let du = vec2<f32>(dpdx(u), dpdy(u));

    let near = in.r.w > 0.5;
    if ((near && z < 0.0) || (!near && z >= 0.0) || u < 0.0 || u > 1.0) {
        discard;
    }
    let t = textureSampleGrad(tex_a, smp, vec2<f32>(u, 0.5), vec2<f32>(du.x, 0.0), vec2<f32>(du.y, 0.0));
    if (t.a < 0.004) {
        discard;
    }
    let l = normalize(in.sun.xyz);
    // the planet's cylindrical shadow across the rings
    let along = dot(pos, l);
    let perp = length(pos - l * along);
    let aa = 2.0 / max(in.r.x, 1.0);
    let shadow = select(1.0, smoothstep(1.0 - aa, 1.0 + aa + 0.02, perp), along < 0.0);
    // sunlit face vs. seen from the unlit side, where light only filters through
    let same_side = sign(dot(pole, l)) == sign(pz);
    let opening = abs(dot(pole, l));
    var lit = 0.75 + 0.7 * opening;
    if (!same_side) {
        lit = (0.18 + 0.75 * (1.0 - t.a)) * (0.6 + opening);
    }
    let col = t.rgb * lit * shadow * in.sun.w;
    return vec4<f32>(col * t.a, t.a);
}
