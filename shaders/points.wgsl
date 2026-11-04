// Instanced star / marker sprites: a tight gaussian core, a Moffat halo (the
// shape of a real telescope's point-spread), and a soft diffraction cross only
// for the handful of stars bright enough to earn one. Additive light.

struct Inst {
    @location(0) center: vec2<f32>,
    @location(1) shape: vec4<f32>, // core_r, glow_r, spike, quad_half
    @location(2) color: vec4<f32>,
    @location(3) depth: f32,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) shape: vec4<f32>,
    @location(2) color: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let c = corners[vi] * inst.shape.w;
    let p = inst.center + c;
    var o: VsOut;
    o.clip = vec4<f32>(p.x / V.size.x * 2.0 - 1.0, 1.0 - p.y / V.size.y * 2.0, inst.depth, 1.0);
    o.local = c;
    o.shape = inst.shape;
    o.color = inst.color;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let core_r = max(in.shape.x, 0.42);
    let glow_r = max(in.shape.y, 0.5);
    let r = length(in.local);
    let core = exp(-(r * r) / (core_r * core_r) * 1.1);
    let g = r / glow_r;
    let halo = 0.26 / pow(1.0 + g * g, 1.7);
    var spikes = 0.0;
    if (in.shape.z > 0.01) {
        let ax = min(abs(in.local.x), abs(in.local.y));
        let reach = glow_r * 2.4;
        spikes = in.shape.z * exp(-ax * ax * 1.6) * exp(-r / reach) * 0.55;
    }
    // fade the halo to zero at the quad edge so the sprite never shows a box
    let edge = 1.0 - smoothstep(0.72, 1.0, r / in.shape.w);
    let i = (core + (halo + spikes) * edge) * in.color.a;
    return vec4<f32>(in.color.rgb * i, 0.0);
}
