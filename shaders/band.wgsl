// Fullscreen scale-band layer: deep-space speckle, procedural Milky Way fog
// (evaluated in true galactocentric kpc coordinates via an affine screen->plane
// map), cosmic-web filaments (supergalactic Mpc), and restricted three-body
// effective-potential contour engraving for Gravity View.

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) px: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    var pts = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    var o: VsOut;
    o.clip = vec4<f32>(pts[vi], 0.0, 1.0);
    o.px = vec2<f32>((pts[vi].x + 1.0) * 0.5 * V.size.x, (1.0 - pts[vi].y) * 0.5 * V.size.y);
    return o;
}

fn speckle(px: vec2<f32>, cell: f32, thresh: f32, seed: f32) -> f32 {
    let c = floor(px / cell);
    let h = hash21(c + seed);
    if (h < thresh) {
        return 0.0;
    }
    let ctr = (c + 0.5 + vec2<f32>(hash21(c + seed + 3.1), hash21(c + seed + 7.7)) - 0.5) * cell;
    let d = length(px - ctr) / cell;
    let b = (h - thresh) / (1.0 - thresh);
    // falloff normalized to the cell so bright stars stay round, never square
    return exp(-d * d * 14.0) * b;
}

fn galaxy_rgb(g: vec2<f32>) -> vec3<f32> {
    // g in kpc, galactocentric, x toward Sun's azimuth reference.
    let r = length(g);
    if (r > 34.0) {
        return vec3<f32>(0.0);
    }
    let theta = atan2(g.y, g.x);
    let pitch = 0.2217; // tan 12.5 deg
    let lr = log(max(r, 0.35) / 3.2) / pitch;
    let a1 = pow(0.5 + 0.5 * cos(2.0 * theta - 2.0 * lr), 2.2);
    let a2 = pow(0.5 + 0.5 * cos(2.0 * theta - 2.0 * lr + PI * 0.55), 2.2);
    let mot = 0.55 + 0.9 * fbm2(g * 0.55 + 4.2);
    var arms = (0.62 * a1 + 0.40 * a2) * mot;
    let radial = exp(-r / 6.2) * smoothstep(34.0, 20.0, r);
    // dust lanes bite the inner arm edges
    let dust = smoothstep(0.55, 0.82, fbm2(g * 0.9 + 8.9));
    arms = arms * (1.0 - 0.55 * dust);
    // bulge and bar (bar at ~27 deg)
    let ba = 0.4712;
    let bx = g.x * cos(ba) + g.y * sin(ba);
    let by = -g.x * sin(ba) + g.y * cos(ba);
    let bar = 1.25 * exp(-pow(bx * bx / 16.0 + by * by / 2.0, 1.25));
    let bulge = 1.7 * exp(-(r * r) / 1.9);
    let disc = 0.16 * exp(-r / 4.6);

    let cool = vec3<f32>(0.58, 0.70, 1.00);
    let warm = vec3<f32>(1.00, 0.86, 0.60);
    var col = cool * arms * radial * 1.15 + warm * (bulge + bar) * 0.85 + mix(cool, warm, 0.5) * disc * radial;
    // HII flecks along arms
    let h2 = fbm2(g * 1.9 - 3.3);
    col = col + vec3<f32>(1.0, 0.45, 0.58) * smoothstep(0.72, 0.86, h2) * arms * radial * 1.4;
    return col;
}

fn cosmic_rgb(c: vec2<f32>) -> vec3<f32> {
    // c in tens of Mpc, supergalactic plane.
    let rg = ridge2(c * 0.85 + 2.7);
    let rg2 = ridge2(c * 1.9 + 9.1);
    let mask = fbm2(c * 0.35 + 1.3);
    var fil = pow(rg, 5.0) * (0.35 + 0.65 * pow(rg2, 2.0)) * smoothstep(0.30, 0.62, mask);
    let sparkle = speckle(c * 90.0, 3.0, 0.9985, 5.0) * fil * 8.0;
    let base = vec3<f32>(0.52, 0.56, 0.95) * fil * 0.85 + vec3<f32>(0.85, 0.80, 1.0) * sparkle;
    return base;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var col = vec3<f32>(0.0);

    // deep space dust — two speckle layers with slow zoom drift
    let wd = V.misc.x;
    if (wd > 0.001) {
        let drift = V.misc.y;
        let s1 = speckle(in.px + drift * 37.0, 2.6, 0.9962, 1.0 + V.misc.z);
        let s2 = speckle(in.px * 0.55 - drift * 19.0, 3.4, 0.9970, 2.0 + V.misc.z);
        col = col + vec3<f32>(0.75, 0.82, 1.0) * s1 * 0.5 * wd + vec3<f32>(1.0, 0.9, 0.75) * s2 * 0.35 * wd;
    }

    // Milky Way fog (soft-knee tone map keeps the bulge luminous, not blown)
    if (V.fog_a.w > 0.001) {
        let g = vec2<f32>(
            V.fog_a.x * in.px.x + V.fog_a.y * in.px.y + V.fog_a.z,
            V.fog_b.x * in.px.x + V.fog_b.y * in.px.y + V.fog_b.z);
        let raw = galaxy_rgb(g) * V.fog_a.w * V.fog_b.w;
        col = col + raw / (vec3<f32>(1.0) + raw * 0.55);
        // Stellar sparkle resolved out of the fog's own density (procedural):
        // grain is chosen in *pixels* via the affine scale, so approaching the
        // Galaxy resolves ever more individual stars instead of a dead clump.
        if (V.misc.w > 0.001) {
            let kpp = max(length(vec2<f32>(V.fog_a.x, V.fog_b.x)), 1e-9); // kpc/px
            let lum = min(dot(raw, vec3<f32>(0.35, 0.45, 0.20)) * 3.5, 1.0);
            let s1 = speckle(g / (kpp * 3.2), 1.0, 0.9960, 11.0);
            let s2 = speckle(g / (kpp * 6.5), 1.0, 0.9980, 23.0);
            col = col
                + (vec3<f32>(0.85, 0.90, 1.0) * s1 * 1.5
                    + vec3<f32>(1.0, 0.88, 0.72) * s2 * 2.0)
                    * lum * V.misc.w;
        }
    }

    // cosmic web
    if (V.cos_a.w > 0.001) {
        let c = vec2<f32>(
            V.cos_a.x * in.px.x + V.cos_a.y * in.px.y + V.cos_a.z,
            V.cos_b.x * in.px.x + V.cos_b.y * in.px.y + V.cos_b.z);
        col = col + cosmic_rgb(c) * V.cos_a.w;
    }

    // gravity view: effective potential contours (rotating frame, CR3BP)
    if (V.grav_b.z > 0.001) {
        let a = V.grav_a.xy;
        let b = V.grav_b.xy;
        let mu = V.grav_a.z;
        let sep = max(V.grav_a.w, 1.0);
        let e1 = (b - a) / sep;
        let e2 = vec2<f32>(-e1.y, e1.x);
        let rel = in.px - a;
        let x = dot(rel, e1) / sep - mu; // barycentric
        let y = dot(rel, e2) / sep;
        let r1 = max(length(vec2<f32>(x + mu, y)), 1e-4);
        let r2 = max(length(vec2<f32>(x - 1.0 + mu, y)), 1e-4);
        let om = 0.5 * (x * x + y * y) + (1.0 - mu) / r1 + mu / r2;
        let t = log2(om - 1.35) * V.grav_b.w;
        let ft = abs(fract(t) - 0.5);
        let w = fwidth(t) * 1.2;
        let line = 1.0 - smoothstep(0.5 * w, 1.6 * w, ft);
        let fade = exp(-max(om - 1.5, 0.0) * 0.55);
        col = col + vec3<f32>(0.83, 0.68, 0.38) * line * fade * 0.34 * V.grav_b.z;
    }

    return vec4<f32>(col, 0.0);
}
