// Instanced star / marker sprites: gaussian core, exponential halo,
// optional diffraction spikes. Mostly additive light over the black canvas.

struct Inst {
    @location(0) center: vec2<f32>,
    @location(1) shape: vec4<f32>, // core_r, glow_r, spike, quad_half
    @location(2) color: vec4<f32>,
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
    o.clip = vec4<f32>(p.x / V.size.x * 2.0 - 1.0, 1.0 - p.y / V.size.y * 2.0, 0.0, 1.0);
    o.local = c;
    o.shape = inst.shape;
    o.color = inst.color;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let core_r = max(in.shape.x, 0.5);
    let glow_r = max(in.shape.y, 0.6);
    let r = length(in.local);
    let g2 = (r / core_r) * (r / core_r);
    let core = exp(-g2 * 1.6);
    let halo = exp(-r / glow_r) * 0.30;
    var spikes = 0.0;
    if (in.shape.z > 0.01) {
        let ax = min(abs(in.local.x), abs(in.local.y));
        spikes = in.shape.z * exp(-ax * 0.9) * exp(-r / (glow_r * 2.2)) * 0.7;
    }
    let i = (core + halo + spikes) * in.color.a;
    let occ = in.color.a * clamp(core * 1.1, 0.0, 1.0) * 0.85;
    return vec4<f32>(in.color.rgb * i, occ);
}
