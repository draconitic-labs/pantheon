// Fullscreen background, one per scale band:
//  - the real sky (ESO all-sky panorama, or a procedural stand-in) behind the
//    solar system, looked up by view direction so it never crawls with zoom;
//  - the heliosphere's glow and the Oort cloud's faint shell (illustrative);
//  - the Milky Way (procedural, in true galactocentric kpc via an affine map);
//  - star dust and distant galaxies at world-locked octaves: zooming reveals
//    more of them and the ones you already see stay exactly where they are;
//  - the cosmic web (procedural Voronoi filaments, supergalactic plane);
//  - restricted three-body potential contours for Gravity View.

@group(1) @binding(0) var sky_tex: texture_2d<f32>;
@group(1) @binding(3) var sky_smp: sampler;

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

// A sparse point in one world-locked grid cell, measured in screen pixels so it
// stays round under any tilt. inv maps plane units to pixels.
// Returns (intensity, hash).
fn cell_star(g: vec2<f32>, cell: f32, seed: f32, occupancy: f32, inv: mat2x2<f32>, size_px: f32) -> vec2<f32> {
    let p = g / cell;
    let c = floor(p);
    let h = hash21(c + vec2<f32>(seed, seed * 1.37));
    if (h > occupancy) {
        return vec2<f32>(0.0);
    }
    let j = vec2<f32>(hash21(c + vec2<f32>(seed + 3.1, 0.7)), hash21(c + vec2<f32>(0.3, seed + 7.7)));
    let dp = (p - c - (0.22 + 0.56 * j)) * cell;
    let d = inv * dp;
    let b = hash21(c + vec2<f32>(seed + 11.3, seed + 5.9));
    let bright = b * b * b;
    let s = size_px * (0.6 + 0.8 * bright);
    return vec2<f32>(exp(-dot(d, d) / (s * s)) * (0.25 + 1.6 * bright), b);
}

// Three octaves around a target pixel spacing; fine octaves fade in as you zoom
// in, so nothing ever slides or pops. Returns (light, colour hash).
fn star_octaves(g: vec2<f32>, upp: f32, inv: mat2x2<f32>, seed: f32, spacing_px: f32, occ: f32) -> vec2<f32> {
    let lk = log2(max(upp * spacing_px, 1e-12));
    let k0 = floor(lk);
    let f = lk - k0;
    let c0 = pow(2.0, k0);
    let a = cell_star(g, c0, seed + k0 * 17.0, occ, inv, 0.72);
    let b = cell_star(g, c0 * 2.0, seed + (k0 + 1.0) * 17.0, occ, inv, 0.8);
    let c = cell_star(g, c0 * 4.0, seed + (k0 + 2.0) * 17.0, occ, inv, 0.9);
    let light = a.x * (1.0 - f) + b.x + c.x;
    let tone = max(a.y * step(0.001, a.x), max(b.y * step(0.001, b.x), c.y * step(0.001, c.x)));
    return vec2<f32>(light, tone);
}

fn star_tint(h: f32) -> vec3<f32> {
    // stellar colours from a blackbody-like ramp, weighted to what's common
    return mix(mix(vec3<f32>(1.0, 0.76, 0.56), vec3<f32>(1.0, 0.95, 0.86), smoothstep(0.1, 0.55, h)),
        vec3<f32>(0.7, 0.82, 1.0), smoothstep(0.6, 0.98, h));
}

// ---- real sky -----------------------------------------------------------

fn sky_color(px: vec2<f32>) -> vec3<f32> {
    let k = V.sky_u.w;
    let dx = (px.x - V.size.x * 0.5) * k;
    let dy = (V.size.y * 0.5 - px.y) * k;
    let d = normalize(V.sky_f.xyz + V.sky_r.xyz * dx + V.sky_u.xyz * dy);
    let l = atan2(d.y, d.x);
    let b = asin(clamp(d.z, -1.0, 1.0));
    if (V.sky_f.w > 0.5) {
        // ESO panorama: galactic centre in the middle, l increasing to the left
        let uv = vec2<f32>(0.5 - l / TAU, 0.5 - b / PI);
        // the map is low-frequency next to the screen, so level 0 at any zoom
        let c = textureSampleLevel(sky_tex, sky_smp, uv, 0.0).rgb;
        // settle the mosaic's noise floor to true black, keep the band
        return max(c - vec3<f32>(0.006), vec3<f32>(0.0)) * 0.62;
    }
    // no panorama on disk: a procedural band, clearly the lesser stand-in
    let warp = fbm2(vec2<f32>(l * 2.2, b * 5.0) + 3.1);
    let band = exp(-pow(abs(b) * (5.0 + 3.0 * warp), 1.6));
    let core = exp(-(l * l * 2.2 + b * b * 90.0));
    let dust = smoothstep(0.5, 0.72, fbm2(vec2<f32>(l * 6.0, b * 22.0)));
    return (vec3<f32>(0.55, 0.60, 0.72) * band * 0.035 + vec3<f32>(0.9, 0.72, 0.5) * core * 0.08) * (1.0 - 0.7 * dust * band);
}

// ---- the Milky Way --------------------------------------------------------

const ARM_PITCH: f32 = 0.2126; // tan 12 deg

// Four-arm logarithmic spiral. Radii along the Sun's azimuth (5.0, 7.0, 9.8,
// 13.7 kpc) put Sol between the Sagittarius and Perseus arms. Stylized.
fn arm_field(g: vec2<f32>, r: f32) -> vec3<f32> {
    let th = atan2(g.y, -g.x); // azimuth measured from the Sun's direction
    let phase = 4.0 * (th - log(max(r, 0.5) / 5.0) / ARM_PITCH);
    // angular crest + a dust lane offset toward the inner edge of each arm
    let crest = 0.5 + 0.5 * cos(phase);
    let lane = 0.5 + 0.5 * cos(phase + 0.85);
    let major = 0.72 + 0.28 * cos(phase * 0.5);
    return vec3<f32>(pow(crest, 3.0) * major, pow(lane, 14.0), crest);
}

fn galaxy_rgb(gi: vec2<f32>, kpp: f32) -> vec3<f32> {
    let r0 = length(gi);
    if (r0 > 40.0) {
        return vec3<f32>(0.0);
    }
    // gentle domain warp: real arms are never mathematical
    let warp = vec2<f32>(fbm2(gi * 0.21 + 11.3), fbm2(gi * 0.21 + 37.9)) - 0.5;
    let g = gi + warp * 2.6;
    let r = length(g);
    let a = arm_field(g, r);
    let n1 = fbm2(g * 0.62 + 4.2);
    let n2 = fbm2(g * 1.9 - 3.3);
    let n3 = fbm2(g * 6.3 + 8.8);
    // detail octave tied to zoom, so a close view never looks like a flat wash
    let detail = fbm2(g / max(kpp * 42.0, 0.04) + 1.7);

    let outer = smoothstep(23.0, 12.0, r);
    let inner_hole = smoothstep(1.6, 3.8, r);
    let radial = exp(-r / 5.2) * outer * inner_hole;
    let clump = 0.32 + 1.5 * n1 * n1 + 0.6 * (n2 - 0.5);
    let arms = a.x * clump * radial;

    // cool young stars on the crests; warm old disc everywhere
    let young = vec3<f32>(0.46, 0.62, 1.0) * arms * 3.1;
    let disc = vec3<f32>(0.95, 0.80, 0.62) * exp(-r / 3.9) * outer * 0.55 * (0.72 + 0.56 * n1);
    var col = young + disc;

    // dust lanes bite the inner edge of each arm, breaking up at small scale
    let dust = a.y * smoothstep(0.32, 0.62, n2) * smoothstep(1.2, 4.0, r) * outer;
    let filaments = smoothstep(0.54, 0.74, n3) * a.z;
    col = col * (1.0 - clamp(0.86 * dust + 0.34 * filaments * outer, 0.0, 0.92));

    // HII regions: pink knots strung along the crests
    let knots = smoothstep(0.68, 0.84, fbm2(g * 2.7 + 21.0)) * a.x;
    col = col + vec3<f32>(1.0, 0.33, 0.48) * knots * radial * 2.4 * (0.5 + n3);
    // blue clusters
    let clusters = smoothstep(0.72, 0.86, fbm2(g * 3.6 - 13.0)) * a.x;
    col = col + vec3<f32>(0.68, 0.82, 1.0) * clusters * radial * 2.2;

    // the bar (near end toward l ~ +27 deg) and the bulge, in true HDR
    let ba = -0.4712;
    let bx = gi.x * cos(ba) + gi.y * sin(ba);
    let by = -gi.x * sin(ba) + gi.y * cos(ba);
    let bar = exp(-pow(bx * bx / 13.0 + by * by / 1.15, 1.1));
    let bulge = exp(-pow(r0 / 1.25, 1.35));
    col = col + vec3<f32>(1.0, 0.76, 0.48) * bar * 1.15 * (0.78 + 0.44 * n2);
    col = col + vec3<f32>(1.0, 0.84, 0.62) * bulge * 2.6;
    // faint stellar halo so the disc doesn't end on a hard edge
    col = col + vec3<f32>(0.72, 0.72, 0.82) * exp(-r0 / 9.0) * 0.012;
    return col * (0.86 + 0.28 * detail);
}

fn fog_density(g: vec2<f32>) -> f32 {
    let r = length(g);
    if (r > 26.0) {
        return exp(-r / 7.0) * 0.1;
    }
    let a = arm_field(g, r);
    return clamp((0.24 + 1.5 * a.x) * exp(-r / 7.0) + 2.4 * exp(-r * r / 8.0), 0.0, 1.0);
}

// ---- the cosmic web -------------------------------------------------------

// Worley distances to the three nearest jittered points.
fn worley3(p: vec2<f32>) -> vec3<f32> {
    let i = floor(p);
    let f = p - i;
    var d1 = 9.0;
    var d2 = 9.0;
    var d3 = 9.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let n = vec2<f32>(f32(x), f32(y));
            let c = i + n;
            let o = vec2<f32>(hash21(c), hash21(c + vec2<f32>(5.2, 1.3)));
            let d = length(n + o - f);
            if (d < d1) {
                d3 = d2;
                d2 = d1;
                d1 = d;
            } else if (d < d2) {
                d3 = d2;
                d2 = d;
            } else if (d < d3) {
                d3 = d;
            }
        }
    }
    return vec3<f32>(d1, d2, d3);
}

// One Voronoi layer: sheets + filaments + nodes. Thin where voids meet.
fn web_layer(p: vec2<f32>, width: f32) -> vec2<f32> {
    let w = worley3(p);
    let edge = w.y - w.x;
    let node = w.z - w.x;
    return vec2<f32>(exp(-edge / width), exp(-node / (width * 2.6)));
}

fn cosmic_rgb(c: vec2<f32>, upp: f32, inv: mat2x2<f32>) -> vec3<f32> {
    // c is in tens of Mpc. Largest voids ~40 Mpc across.
    let warp = vec2<f32>(fbm2(c * 0.17 + 2.7), fbm2(c * 0.17 + 9.1)) - 0.5;
    let q = c + warp * 3.4;
    let big = web_layer(q / 5.0, 0.055);
    let mid = web_layer(q / 1.9 + 7.3, 0.075);
    let fine = web_layer(q / 0.72 + 3.1, 0.10);
    let large_scale = 0.45 + 1.1 * fbm2(c * 0.09 + 1.3);
    var density = big.x * 0.9 + big.y * 1.5
        + mid.x * (0.16 + 0.8 * big.x) * 0.75 + mid.y * big.x * 0.7
        + fine.x * (0.05 + 0.5 * mid.x) * 0.4;
    density = density * large_scale;

    // named clusters sit in bright knots with filaments radiating from them
    for (var k = 0; k < 16; k = k + 1) {
        let n = V.nodes[k];
        if (n.w <= 0.0) {
            continue;
        }
        let dv = c - n.xy;
        let d = length(dv) / n.z;
        if (d < 9.0) {
            let ang = atan2(dv.y, dv.x);
            let spokes = pow(0.5 + 0.5 * cos(ang * (4.0 + f32(k % 3)) + f32(k) * 2.4 + fbm2(c * 0.9) * 3.0), 8.0);
            density = density + n.w * (exp(-d * d * 0.8) * 2.8 + spokes * exp(-d * 0.55) * 0.42);
        }
    }

    let voidc = vec3<f32>(0.10, 0.13, 0.36);
    let sheet = vec3<f32>(0.30, 0.34, 0.86);
    let fil = vec3<f32>(0.78, 0.60, 0.98);
    let knot = vec3<f32>(1.0, 0.76, 0.50);
    var col = mix(voidc, sheet, smoothstep(0.02, 0.28, density));
    col = mix(col, fil, smoothstep(0.26, 0.85, density));
    col = mix(col, knot, smoothstep(0.85, 2.4, density));
    var light = col * pow(density, 1.25) * 0.5;

    // every speck is a galaxy; they trace the web and thin out in the voids
    let dots = star_octaves(c, upp, inv, 41.0, 4.2, 0.6);
    light = light + mix(fil, vec3<f32>(1.0, 0.9, 0.78), dots.y) * dots.x * clamp(density * 1.2 + 0.015, 0.0, 1.6) * 0.8;
    return light;
}

// ---- solar neighbourhood glows ---------------------------------------------

fn helio_rgb(px: vec2<f32>) -> vec3<f32> {
    let rel = px - V.helio.xy;
    let au = V.helio.z;
    let tail = V.helio_b.xy;
    let along = dot(rel, tail) / au; // AU downstream of the Sun
    let side = length(rel - tail * dot(rel, tail)) / au;
    // a blunt nose at ~120 AU upstream, drawn out into a modest tail: a sketch
    let stretch = 1.0 + smoothstep(-20.0, 260.0, along) * 1.1;
    let rr = length(vec2<f32>(along / stretch, side));
    let shell = exp(-pow((rr - 122.0) / 10.0, 2.0));
    let fill = smoothstep(126.0, 88.0, rr);
    let fade = exp(-max(along, 0.0) / 520.0);
    return vec3<f32>(0.30, 0.44, 0.92) * (shell * 0.14 + fill * 0.022) * fade * V.helio.w;
}

fn oort_rgb(px: vec2<f32>) -> vec3<f32> {
    let rel = px - V.helio.xy;
    let r = length(rel) / V.helio.z; // AU
    // a very soft, faint shell between ~2,000 and ~100,000 AU
    let shell = smoothstep(1500.0, 16000.0, r) * smoothstep(100000.0, 42000.0, r);
    let n = fbm2(rel / V.helio.z / 21000.0 + 3.3);
    return vec3<f32>(0.34, 0.43, 0.62) * shell * (0.3 + n) * 0.036 * V.helio_b.z;
}

// --------------------------------------------------------------------------

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var col = vec3<f32>(0.0);

    // the sky at infinity, a fixed backdrop behind the solar system
    if (V.sky_r.w > 0.001) {
        col = col + sky_color(in.px) * V.sky_r.w;
    }
    if (V.helio.w > 0.001) {
        col = col + helio_rgb(in.px);
    }
    if (V.helio_b.z > 0.001) {
        col = col + oort_rgb(in.px);
    }

    // Milky Way
    if (V.fog_a.w > 0.001 || V.fog_b.w > 0.001) {
        let g = vec2<f32>(
            V.fog_a.x * in.px.x + V.fog_a.y * in.px.y + V.fog_a.z,
            V.fog_b.x * in.px.x + V.fog_b.y * in.px.y + V.fog_b.z);
        let m = mat2x2<f32>(vec2<f32>(V.fog_a.x, V.fog_b.x), vec2<f32>(V.fog_a.y, V.fog_b.y));
        let det = determinant(m);
        let inv = mat2x2<f32>(vec2<f32>(m[1].y, -m[0].y), vec2<f32>(-m[1].x, m[0].x)) * (1.0 / det);
        let kpp = sqrt(abs(det));
        if (V.fog_a.w > 0.001) {
            col = col + galaxy_rgb(g, kpp) * V.fog_a.w;
        }
        if (V.fog_b.w > 0.001) {
            // stars the density model predicts, kept out of the sphere where
            // this atlas has real Gaia measurements
            let dens = fog_density(g);
            let outside = smoothstep(V.sun_g.z * 0.9, V.sun_g.z * 1.12, length(g - V.sun_g.xy));
            let dust = star_octaves(g, kpp, inv, 11.0, 4.0, 0.62);
            let light = dust.x * clamp(dens * 2.2 + 0.05, 0.0, 1.4) * outside;
            col = col + star_tint(dust.y) * light * V.fog_b.w * 0.9;
        }
    }

    // distant background galaxies, world-locked on a camera-facing plane
    if (V.deep_a.w > 0.001) {
        let d = vec2<f32>(
            V.deep_a.x * in.px.x + V.deep_a.y * in.px.y + V.deep_a.z,
            V.deep_b.x * in.px.x + V.deep_b.y * in.px.y + V.deep_b.z);
        let m = mat2x2<f32>(vec2<f32>(V.deep_a.x, V.deep_b.x), vec2<f32>(V.deep_a.y, V.deep_b.y));
        let det = determinant(m);
        let inv = mat2x2<f32>(vec2<f32>(m[1].y, -m[0].y), vec2<f32>(-m[1].x, m[0].x)) * (1.0 / det);
        let upp = sqrt(abs(det));
        let far = star_octaves(d, upp, inv, 73.0, 9.0, 0.5);
        col = col + star_tint(far.y * 0.8 + 0.1) * far.x * 0.16 * V.deep_a.w;
    }

    // the cosmic web, on the supergalactic plane
    if (V.cos_a.w > 0.001) {
        let c = vec2<f32>(
            V.cos_a.x * in.px.x + V.cos_a.y * in.px.y + V.cos_a.z,
            V.cos_b.x * in.px.x + V.cos_b.y * in.px.y + V.cos_b.z);
        let m = mat2x2<f32>(vec2<f32>(V.cos_a.x, V.cos_b.x), vec2<f32>(V.cos_a.y, V.cos_b.y));
        let det = determinant(m);
        let inv = mat2x2<f32>(vec2<f32>(m[1].y, -m[0].y), vec2<f32>(-m[1].x, m[0].x)) * (1.0 / det);
        col = col + cosmic_rgb(c, sqrt(abs(det)), inv) * V.cos_a.w;
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
        let t = log2(max(om - 1.35, 1e-3)) * V.grav_b.w;
        let ft = abs(fract(t) - 0.5);
        let w = fwidth(t) * 1.2;
        let line = 1.0 - smoothstep(0.5 * w, 1.6 * w, ft);
        let fade = exp(-max(om - 1.5, 0.0) * 0.55);
        col = col + vec3<f32>(0.83, 0.68, 0.38) * line * fade * 0.34 * V.grav_b.z;
    }

    return vec4<f32>(col, 0.0);
}
