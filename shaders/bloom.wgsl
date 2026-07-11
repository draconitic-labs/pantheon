// Bright-pass spiral bloom: gathers HDR energy above threshold at half res.
// Wide, soft, cheap — the luminous-manuscript glow the ink deserves.

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var hdr_smp: sampler;

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

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var acc = vec3<f32>(0.0);
    // golden-angle spiral, radius to ~2.2% of the frame
    for (var i = 0; i < 18; i = i + 1) {
        let fi = f32(i);
        let ang = fi * 2.399963;
        let r = sqrt((fi + 0.5) / 18.0) * 0.022;
        let off = vec2<f32>(cos(ang), sin(ang)) * r;
        let c = textureSampleLevel(hdr_tex, hdr_smp, in.uv + off, 0.0).rgb;
        acc = acc + max(c - vec3<f32>(0.55), vec3<f32>(0.0));
    }
    return vec4<f32>(acc / 18.0, 1.0);
}
