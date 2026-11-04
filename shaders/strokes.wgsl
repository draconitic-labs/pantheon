// Anti-aliased luminous vector strokes with glow and dash.
// Premultiplied blending: glow adds light (alpha 0), core occludes (alpha > 0).

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) da: vec2<f32>,     // signed perp distance px, arclength px
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>, // half_width, glow, dash_period, dash_duty
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) da: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
}

@vertex
fn vs(in: VsIn) -> VsOut {
    var o: VsOut;
    let ndc = vec2<f32>(in.pos.x / V.size.x * 2.0 - 1.0, 1.0 - in.pos.y / V.size.y * 2.0);
    o.clip = vec4<f32>(ndc, 0.0, 1.0);
    o.da = in.da;
    o.color = in.color;
    o.params = in.params;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let hw = in.params.x;
    let glow = in.params.y;
    let ad = abs(in.da.x);
    let aa = 0.75 / max(V.size.w, 1.0); // one physical pixel of edge, at any DPI
    // a hairline thinner than the edge filter fades in intensity, never width
    let core = (1.0 - smoothstep(hw - aa, hw + aa, ad)) * min(hw * 2.0 / (aa * 1.2), 1.0);
    var g = 0.0;
    if (glow > 0.01) {
        g = exp(-max(ad - hw, 0.0) / glow) * 0.42;
    }
    var dash = 1.0;
    if (in.params.z > 0.5) {
        let m = fract(in.da.y / in.params.z);
        let edge = 0.08;
        dash = smoothstep(0.0, edge, m) * (1.0 - smoothstep(in.params.w, in.params.w + edge, m));
    }
    let a = in.color.a * dash;
    let lum = in.color.rgb * a * (core + g);
    return vec4<f32>(lum, a * core * 0.5);
}
