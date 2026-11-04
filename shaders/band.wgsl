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
    // no photograph applies (not on disk, or far from where it was taken):
    // a procedural band and star grain, labeled as such
    let warp = fbm2(vec2<f32>(l * 2.2, b * 5.0) + 3.1);
    let band = exp(-pow(abs(b) * (5.0 + 3.0 * warp), 1.6));
    let core = exp(-(l * l * 2.2 + b * b * 90.0));
    let dust = smoothstep(0.5, 0.72, fbm2(vec2<f32>(l * 6.0, b * 22.0)));
    let inv = mat2x2<f32>(vec2<f32>(1.0 / k, 0.0), vec2<f32>(0.0, 1.0 / k));
    let stars = star_octaves(vec2<f32>(l, b), k, inv, 5.0, 5.0, 0.5);
    let glow = (vec3<f32>(0.55, 0.60, 0.72) * band * 0.035 + vec3<f32>(0.9, 0.72, 0.5) * core * 0.08) * (1.0 - 0.7 * dust * band);
    return glow + star_tint(stars.y) * stars.x * (0.22 + 0.5 * band);
}

// ---- the Milky Way --------------------------------------------------------

const ARM_PITCH: f32 = 0.2126; // tan 12 deg

// Four-arm logarithmic spiral. Radii along the Sun's azimuth (5.0, 7.0, 9.8,
// 13.7 kpc) put Sol between the Sagittarius and Perseus arms. Stylized, but winding
// as measured: ln(R/Rref) = -(az-azref) tan(pitch), Reid et al. 2014, with azimuth growing
// the way the Galaxy turns, so the arms trail.
fn arm_field(g: vec2<f32>, r: f32) -> vec3<f32> {
    let th = atan2(g.y, -g.x); // azimuth measured from the Sun's direction
    let phase = 4.0 * (th + log(max(r, 0.5) / 5.0) / ARM_PITCH);
    // angular crest + a dust lane offset toward the inner edge of each arm
    let crest = 0.5 + 0.5 * cos(phase);
    let lane = 0.5 + 0.5 * cos(phase + 0.85);
    let major = 0.72 + 0.28 * cos(phase * 0.5);
    return vec3<f32>(pow(crest, 2.1) * major, pow(lane, 10.0), crest);
}

// Texture at the scale of the screen that stays put as you zoom: two adjacent
// world-locked octaves, cross-faded, so detail appears without ever sliding.
fn fine_detail(g: vec2<f32>, kpp: f32) -> f32 {
    let lk = log2(max(kpp * 60.0, 1e-6));
    let k0 = floor(lk);
    let f = lk - k0;
    let s0 = pow(2.0, k0);
    return mix(fbm2(g / s0 + 17.0), fbm2(g / (s0 * 2.0) + 17.0), f);
}

fn galaxy_rgb(gi: vec2<f32>, kpp: f32) -> vec3<f32> {
    let r0 = length(gi);
    if (r0 > 40.0) {
        return vec3<f32>(0.0);
    }
    // gentle domain warp: real arms are never mathematical
    let warp = vec2<f32>(fbm2(gi * 0.21 + 11.3), fbm2(gi * 0.21 + 37.9)) - 0.5;
    let g = gi + warp * 2.4;
    let r = length(g);
    let a = arm_field(g, r);
    let n1 = fbm2(g * 0.62 + 4.2);
    let n2 = fbm2(g * 1.9 - 3.3);
    let n3 = fbm2(g * 6.3 + 8.8);
    // up close the glow breaks into clouds, so a near view is never a flat wash
    let micro = fine_detail(gi, kpp);
    let close = smoothstep(0.03, 0.006, kpp);

    let outer = smoothstep(24.0, 13.0, r);
    let inner_hole = smoothstep(1.2, 4.0, r);
    let radial = exp(-r / 6.0) * outer * inner_hole;
    let clump = 0.38 + 1.25 * n1 * n1 + 0.55 * (n2 - 0.5) + 0.3 * (n3 - 0.5);
    let arms = a.x * max(clump, 0.05) * radial;

    // cool young stars on the crests; a warm old disc everywhere between them
    let young = vec3<f32>(0.50, 0.65, 1.0) * arms * 2.9;
    let disc = vec3<f32>(0.98, 0.78, 0.58) * exp(-r / 4.1) * outer * (0.62 + 0.5 * n1);
    var col = young + disc * 0.62;

    // dust: dark lanes on the inner edge of each arm, breaking up at small scale,
    // tinting the light behind them rather than cutting it to black
    let dust = a.y * smoothstep(0.34, 0.66, n2) * smoothstep(1.0, 3.6, r) * outer;
    let wisps = smoothstep(0.50, 0.76, n3) * a.z * smoothstep(0.0, 1.0, n2) * outer;
    let extinction = clamp(0.72 * dust + 0.26 * wisps, 0.0, 0.85);
    col = col * mix(vec3<f32>(1.0), vec3<f32>(0.26, 0.17, 0.12), extinction);

    // HII regions: pink knots strung along the crests, with their own glow
    let knots = smoothstep(0.66, 0.84, fbm2(g * 2.7 + 21.0)) * a.x * mix(1.0, smoothstep(0.44, 0.70, micro), close);
    col = col + vec3<f32>(1.0, 0.30, 0.44) * knots * radial * 1.5;
    // blue open clusters
    let clusters = smoothstep(0.70, 0.86, fbm2(g * 3.6 - 13.0)) * a.x;
    col = col + vec3<f32>(0.66, 0.80, 1.0) * clusters * radial * 1.2;

    // the bar (near end toward l ~ +27 deg) and the bulge
    let ba = -0.4712;
    let bx = gi.x * cos(ba) + gi.y * sin(ba);
    let by = -gi.x * sin(ba) + gi.y * cos(ba);
    let bar = exp(-pow(bx * bx / 12.0 + by * by / 1.3, 1.05));
    let bulge = exp(-pow(r0 / 1.15, 1.25));
    col = col + vec3<f32>(1.0, 0.72, 0.44) * bar * 0.62 * (0.82 + 0.36 * n2);
    col = col + vec3<f32>(1.0, 0.82, 0.60) * bulge * 1.9;
    // a faint stellar halo so the disc doesn't end on a hard edge
    col = col + vec3<f32>(0.72, 0.72, 0.82) * exp(-r0 / 9.0) * 0.012;
    // and the eye adapts: inside the bright disc the glow must not wash out
    let adapted = smoothstep(0.022, 0.004, kpp);
    col = col * mix(1.0, 0.22 + 1.3 * smoothstep(0.22, 0.8, micro), close) * mix(1.0, 0.2, adapted);
    // bright regions roll off harder up close, so stars stand out from the glow
    return col / (1.0 + adapted * 2.4 * dot(col, vec3<f32>(0.3, 0.5, 0.2)));
}

fn fog_density(g: vec2<f32>) -> f32 {
    let r = length(g);
    if (r > 26.0) {
        return exp(-r / 7.0) * 0.1;
    }
    let a = arm_field(g, r);
    return clamp((0.24 + 1.5 * a.x) * exp(-r / 7.0) + 2.4 * exp(-r * r / 8.0), 0.0, 1.0);
}

// ---- distant galaxies ------------------------------------------------------

// A faint little galaxy in one world-locked cell: elongated, randomly turned.
fn cell_galaxy(g: vec2<f32>, cell: f32, seed: f32, occupancy: f32, inv: mat2x2<f32>) -> vec2<f32> {
    let p = g / cell;
    let c = floor(p);
    let h = hash21(c + vec2<f32>(seed, seed * 1.37));
    if (h > occupancy) {
        return vec2<f32>(0.0);
    }
    let j = vec2<f32>(hash21(c + vec2<f32>(seed + 3.1, 0.7)), hash21(c + vec2<f32>(0.3, seed + 7.7)));
    let d0 = inv * ((p - c - (0.25 + 0.5 * j)) * cell);
    let ang = hash21(c + vec2<f32>(seed + 9.1, 2.2)) * PI;
    let ca = cos(ang);
    let sa = sin(ang);
    let d = vec2<f32>(d0.x * ca + d0.y * sa, -d0.x * sa + d0.y * ca);
    let b = hash21(c + vec2<f32>(seed + 11.3, seed + 5.9));
    let size = 0.75 + 1.5 * b * b;
    let flat_ = 1.0 + 2.4 * hash21(c + vec2<f32>(4.4, seed));
    let e = (d.x * d.x + d.y * d.y * flat_ * flat_) / (size * size);
    return vec2<f32>(exp(-e) * (0.28 + 0.9 * b), hash21(c + vec2<f32>(seed, 6.6)));
}

fn deep_field(d: vec2<f32>, upp: f32, inv: mat2x2<f32>) -> vec3<f32> {
    let lk = log2(max(upp * 22.0, 1e-12));
    let k0 = floor(lk);
    let f = lk - k0;
    let c0 = pow(2.0, k0);
    let a = cell_galaxy(d, c0, 73.0 + k0 * 19.0, 0.5, inv);
    let b = cell_galaxy(d, c0 * 2.0, 73.0 + (k0 + 1.0) * 19.0, 0.5, inv);
    let elliptical = vec3<f32>(1.0, 0.80, 0.60);
    let spiral = vec3<f32>(0.62, 0.74, 1.0);
    return mix(elliptical, spiral, a.y) * a.x * (1.0 - f) + mix(elliptical, spiral, b.y) * b.x;
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
    let warp2 = vec2<f32>(fbm2(c * 0.9 + 5.1), fbm2(c * 0.9 + 1.9)) - 0.5;
    let q = c + warp * 3.8 + warp2 * 0.55;
    let big = web_layer(q / 4.6, 0.020);
    let mid = web_layer(q / 1.8 + 7.3, 0.030);
    let fine = web_layer(q / 0.66 + 3.1, 0.050);
    // the largest structures come and go: some walls are rich, others thin out
    let rich = smoothstep(0.30, 0.72, fbm2(c * 0.13 + 1.3));
    let streak = 0.45 + 1.1 * fbm2(q * 1.4 + 8.0);
    var density = big.x * (0.35 + 1.0 * rich) * streak + big.y * (0.5 + 1.2 * rich) * 0.9;
    density = density + mid.x * (0.09 + 0.6 * big.x + 0.2 * rich) * streak + mid.y * big.x * 0.5;
    density = density + fine.x * (0.028 + 0.24 * mid.x);

    // named clusters sit in knots with filaments radiating from them
    for (var k = 0; k < 16; k = k + 1) {
        let n = V.nodes[k];
        if (n.w <= 0.0) {
            continue;
        }
        let dv = c - n.xy;
        let d = length(dv) / n.z;
        if (d < 14.0) {
            let dir = dv / max(length(dv), 1e-4);
            let wob = fbm2(c * 1.1 + f32(k) * 3.7) * 2.6;
            let ang = atan2(dir.y, dir.x) + wob;
            let spokes = pow(0.5 + 0.5 * cos(ang * (3.0 + f32(k % 3))), 26.0);
            density = density + n.w * (exp(-d * d * 0.7) * 1.25 + spokes * exp(-d * 0.42) * 0.44);
        }
    }

    let sheet = vec3<f32>(0.24, 0.30, 0.78);
    let fil = vec3<f32>(0.66, 0.54, 0.98);
    let knot = vec3<f32>(1.0, 0.74, 0.50);
    var col = mix(sheet, fil, smoothstep(0.08, 0.55, density));
    col = mix(col, knot, smoothstep(0.55, 1.5, density));
    var light = col * pow(density, 1.35) * 0.42;

    // every speck is a galaxy; they trace the web and thin out in the voids
    let dots = star_octaves(c, upp, inv, 41.0, 3.4, 0.7);
    let on_web = clamp(pow(density, 0.8) * 1.5 + 0.012, 0.0, 1.5);
    light = light + mix(fil, vec3<f32>(1.0, 0.9, 0.78), dots.y) * dots.x * on_web * 0.72;
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
    let stretch = 1.0 + smoothstep(-20.0, 220.0, along) * 0.7;
    let rr = length(vec2<f32>(along / stretch, side));
    let shell = exp(-pow((rr - 122.0) / 9.0, 2.0));
    let fill = smoothstep(126.0, 90.0, rr);
    return vec3<f32>(0.30, 0.44, 0.92) * (shell * 0.12 + fill * 0.020) * V.helio.w;
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
            // once individual stars can't be resolved they should read as grain,
            // not as a blizzard over the light they make up
            let resolve = clamp(0.012 / max(kpp, 1e-5), 0.14, 1.0);
            // up close, dust clouds dim the stars behind them; the old inner
            // Galaxy is warm, the outer disc bluer
            let clouds = mix(1.0, 0.3 + 1.0 * smoothstep(0.22, 0.8, fine_detail(g, kpp)), smoothstep(0.03, 0.006, kpp));
            let tint = mix(star_tint(dust.y), vec3<f32>(1.0, 0.80, 0.56), exp(-length(g) / 3.2) * 0.85);
            let light = dust.x * clamp(dens * 1.6 + 0.05, 0.0, 0.95) * outside * resolve * clouds;
            col = col + tint * light * V.fog_b.w * 0.9;
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
        col = col + deep_field(d, upp, inv) * 0.75 * V.deep_a.w;
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
        col = col + vec3<f32>(0.83, 0.68, 0.38) * line * fade * 0.17 * V.grav_b.z;
    }

    return vec4<f32>(col, 0.0);
}
