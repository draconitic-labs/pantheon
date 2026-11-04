// Shaded bodies: an orthographic sphere per instance, lit by the real sun
// direction. Where imagery is on disk it is read through the body's IAU frame;
// otherwise a procedural surface stands in. Kinds: 0 ember, 1 rocky, 2 banded
// giant, 3 star, 4 ocean world, 5 airless/cratered.

struct Inst {
    @location(0) center: vec2<f32>,
    @location(1) pr: vec4<f32>,    // radius_px, flatten, kind, seed
    @location(2) sun: vec4<f32>,   // sun dir xyz (view space), ambient
    @location(3) axis: vec4<f32>,  // body z (pole) in view space, spin phase rad
    @location(4) bx: vec4<f32>,    // body x (prime meridian) in view space, imagery fade 0..1
    @location(5) by: vec4<f32>,    // body y in view space, cloud longitude offset rad
    @location(6) ca: vec4<f32>,
    @location(7) cb: vec4<f32>,
    @location(8) rim: vec4<f32>,   // atmosphere rgb, strength
    @location(9) fx: vec4<f32>,    // atmosphere height (radii), ring inner, ring outer, relief
    @location(10) flags: vec4<f32>, // has night map, has clouds, has ring shadow, emission
    @location(11) depth: f32,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) pr: vec4<f32>,
    @location(2) sun: vec4<f32>,
    @location(3) axis: vec4<f32>,
    @location(4) bx: vec4<f32>,
    @location(5) by: vec4<f32>,
    @location(6) ca: vec4<f32>,
    @location(7) cb: vec4<f32>,
    @location(8) rim: vec4<f32>,
    @location(9) fx: vec4<f32>,
    @location(10) flags: vec4<f32>,
    @location(11) depth: f32,
}

@group(1) @binding(0) var tex_a: texture_2d<f32>;
@group(1) @binding(1) var tex_b: texture_2d<f32>;
@group(1) @binding(2) var tex_c: texture_2d<f32>;
@group(1) @binding(3) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let star = inst.pr.z > 2.5 && inst.pr.z < 3.5;
    // room outside the silhouette for the atmosphere shell or the corona
    let reach = select(1.0 + inst.fx.x * 5.0, 3.8, star);
    let half_px = inst.pr.x * reach * 1.04 + 2.0;
    let c = corners[vi] * half_px;
    let p = inst.center + c;
    var o: VsOut;
    o.clip = vec4<f32>(p.x / V.size.x * 2.0 - 1.0, 1.0 - p.y / V.size.y * 2.0, inst.depth, 1.0);
    o.local = c;
    o.pr = inst.pr;
    o.sun = inst.sun;
    o.axis = inst.axis;
    o.bx = inst.bx;
    o.by = inst.by;
    o.ca = inst.ca;
    o.cb = inst.cb;
    o.rim = inst.rim;
    o.fx = inst.fx;
    o.flags = inst.flags;
    o.depth = inst.depth;
    return o;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The surface when no imagery is on disk: fBm in the body frame, per-kind.
fn procedural_surface(kind: i32, q: vec3<f32>, lat: f32, lon: f32, in: VsOut) -> vec3<f32> {
    var col = in.ca.rgb;
    let seed = in.pr.w;
    let det = max(in.cb.a, 0.2);
    if (kind == 0) {
        col = in.ca.rgb;
    } else if (kind == 1) {
        let m = fbm3(q * 3.1);
        col = mix(in.cb.rgb, in.ca.rgb, m);
        let c = fbm3(q * 7.3 + 11.0);
        col = col * (1.0 - smoothstep(0.60, 0.74, c) * 0.38 * det);
        let c2 = fbm3(q * 15.0 + 4.7);
        col = col * (1.0 - smoothstep(0.62, 0.78, c2) * 0.22 * det);
        if (in.ca.a > 0.01) { // polar caps
            let edge = 1.22 - 0.10 * fbm3(q * 5.0 + 2.2);
            let cap = smoothstep(edge, edge + 0.10, abs(lat)) * in.ca.a;
            col = mix(col, vec3<f32>(0.95, 0.94, 0.92), cap);
        }
    } else if (kind == 2) { // banded giant
        let turb = fbm3(q * 2.4) - 0.5;
        let bands = 0.5 + 0.5 * sin(lat * 11.0 + turb * 3.4 * det + seed * 5.0);
        col = mix(in.ca.rgb, in.cb.rgb, bands);
        col = col * (0.92 + 0.16 * (fbm3(q * 4.0) - 0.5) * det);
        col = col * (0.90 + 0.10 * (fbm3(q * 9.0 + 6.0) - 0.5) * det);
        col = col * (1.0 - smoothstep(1.15, 1.5, abs(lat)) * 0.22);
        if (seed > 0.55) { // storm oval
            let sl = lat + 0.38;
            let so = atan2(sin(lon - 1.3), cos(lon - 1.3));
            let spot = exp(-(sl * sl * 30.0 + so * so * 3.2) * 2.2);
            col = mix(col, vec3<f32>(0.82, 0.44, 0.30), spot * 0.75);
        }
    } else if (kind == 4) { // ocean world without imagery
        let ice = smoothstep(1.16, 1.34, abs(lat));
        col = mix(in.ca.rgb, vec3<f32>(0.92, 0.95, 1.0), ice * 0.85);
    } else { // cratered
        let m = fbm3(q * 4.2);
        col = mix(in.cb.rgb, in.ca.rgb, m);
        let mare = fbm3(q * 1.15 + seed * 31.0);
        col = col * (1.0 - smoothstep(0.52, 0.72, mare) * 0.30 * det);
        let c = fbm3(q * 9.0 + 5.0);
        col = col * (1.0 - smoothstep(0.58, 0.75, c) * 0.30 * det);
        let c2 = fbm3(q * 18.0 + 8.5);
        col = col * (1.0 - smoothstep(0.64, 0.80, c2) * 0.18 * det);
    }
    return col;
}

@fragment
fn fs(in: VsOut) -> FsOut {
    let radius = max(in.pr.x, 0.5);
    let kind = i32(in.pr.z + 0.5);
    let seed = in.pr.w;
    let ppp = max(V.size.w, 1.0);
    let time = V.size.z;
    let bz = normalize(in.axis.xyz);
    let bx = in.bx.xyz;
    let by = in.by.xyz;

    // Oblateness: the disc is squashed along the projection of the spin axis.
    var p = in.local / radius;
    let ax2 = bz.xy * vec2<f32>(1.0, -1.0);
    let ax_len = length(ax2);
    if (in.pr.y > 0.0005 && ax_len > 0.03) {
        let a2 = ax2 / ax_len;
        let along = dot(p, a2);
        let perp = vec2<f32>(-a2.y, a2.x);
        let squash = 1.0 - in.pr.y * ax_len * ax_len;
        p = a2 * (along / squash) + perp * dot(p, perp);
    }
    let rr = length(p);
    let aa = 1.1 / (radius * ppp);
    let alpha = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, rr);

    // Surface coordinates, computed for the limb as well so derivatives stay sane.
    let nz = sqrt(max(1.0 - rr * rr, 0.0));
    let n = normalize(vec3<f32>(p.x, -p.y, nz));
    let bc = vec3<f32>(dot(n, bx), dot(n, by), dot(n, bz));
    let lat = asin(clamp(bc.z, -1.0, 1.0));
    let lon = atan2(bc.y, bc.x);
    let u0 = lon / TAU + 0.5;
    let v0 = 0.5 - lat / PI;
    // seam-free longitude: pick whichever wrap is continuous at this pixel
    let u1 = fract(u0);
    let u2 = fract(u0 + 0.5) - 0.5;
    let sel = fwidth(u1) <= fwidth(u2) + 1e-6;
    let u = select(u2, u1, sel);
    let gx = vec2<f32>(select(dpdx(u2), dpdx(u1), sel), dpdx(v0));
    let gy = vec2<f32>(select(dpdy(u2), dpdy(u1), sel), dpdy(v0));
    let uv = vec2<f32>(u, v0);

    var o: FsOut;
    o.depth = in.depth;

    // Sample every map up front: sampling must not depend on the branches below.
    let img_fade = in.bx.w;
    let day = textureSampleGrad(tex_a, smp, uv, gx, gy).rgb;
    let night = textureSampleGrad(tex_b, smp, uv, gx, gy).rgb;
    let cloud_uv = vec2<f32>(u - in.by.w / TAU, v0);
    let cl_tex = textureSampleGrad(tex_c, smp, cloud_uv, gx, gy).r;

    let q = vec3<f32>(cos(lat) * cos(lon + in.axis.w), cos(lat) * sin(lon + in.axis.w), sin(lat)) + vec3<f32>(seed * 13.7);
    let l = normalize(in.sun.xyz);
    let amb = in.sun.w;

    // ---- stars ---------------------------------------------------------------
    if (kind == 3) {
        let emission = in.flags.w;
        let mu = nz;
        let limb = 0.36 + 0.64 * pow(mu, 0.62);
        // granulation over the map: slow flow, procedural and marked illustrative
        let flow = fbm3(q * 34.0 + vec3<f32>(time * 0.014, -time * 0.011, time * 0.008));
        let fine = fbm3(q * 120.0 + vec3<f32>(-time * 0.02, time * 0.016, 0.0));
        let cell = fbm3(q * 7.0 + vec3<f32>(0.0, time * 0.006, 0.0));
        let grain = 0.86 + 0.2 * flow + 0.12 * fine;
        var surf: vec3<f32>;
        if (img_fade > 0.001) {
            // the map's mottling read as temperature: orange-gold to near white
            let h = pow(clamp(lum(day), 0.0, 1.0), 0.6);
            let ramp = mix(vec3<f32>(1.0, 0.50, 0.13), vec3<f32>(1.0, 0.90, 0.60), smoothstep(0.2, 0.9, h));
            let img = ramp * (0.72 + 0.72 * h) * grain;
            let pr = in.ca.rgb * (0.7 + 0.5 * cell) * grain;
            surf = mix(pr, img, img_fade);
        } else {
            surf = in.ca.rgb * (0.72 + 0.56 * cell) * grain;
        }
        var col = surf * limb * emission;

        // chromosphere, prominences and corona outside the limb
        var glow = vec3<f32>(0.0);
        if (rr > 0.97) {
            let d = max(rr - 1.0, 0.0);
            let dir = p / max(rr, 1e-4);
            let chrom = exp(-d / 0.007) * 0.8;
            let pn = fbm2(dir * 7.0 + vec2<f32>(seed * 40.0, time * 0.02));
            let height = 0.01 + 0.09 * pow(pn, 3.0);
            let prom = (1.0 - smoothstep(height * 0.5, height, d))
                * smoothstep(0.52, 0.74, fbm2(dir * 26.0 + vec2<f32>(seed * 3.0, d * 30.0 - time * 0.04)));
            glow = in.cb.rgb * vec3<f32>(1.0, 0.42, 0.3) * (chrom + prom * 1.2) * 1.5;
            // a corona: mostly smooth, faintly streamered, gone well before the quad edge
            let streak = fbm2(dir * 2.6 + vec2<f32>(seed * 9.0, 3.0));
            let streak2 = fbm2(dir * 8.0 + vec2<f32>(-seed * 5.0, 7.0));
            let falloff = pow(max(rr, 1.0), -4.2) * (0.5 + 0.8 * streak + 0.3 * streak2);
            let edge = 1.0 - smoothstep(2.2, 3.6, rr);
            glow = glow + vec3<f32>(1.0, 0.92, 0.78) * falloff * edge * 0.36 * min(emission, 3.0);
        }
        o.color = vec4<f32>(col * alpha + glow * (1.0 - alpha * 0.85), alpha);
        o.depth = select(1.0, in.depth, alpha > 0.0);
        return o;
    }

    // ---- planets, moons --------------------------------------------------------
    var albedo = vec3<f32>(0.0);
    if (img_fade < 0.999) {
        albedo = procedural_surface(kind, q, lat, lon + in.axis.w, in);
    }
    if (img_fade > 0.001) {
        albedo = mix(albedo, day, img_fade);
    }

    // relief: shade from image brightness (a look, not elevation data)
    var ns = n;
    if (in.fx.w > 0.001 && img_fade > 0.5) {
        let span = max(max(abs(gx.y), abs(gy.y)), 0.0002);
        let du = span * 1.5 / max(cos(lat), 0.15);
        let dv = span * 1.5;
        let he = lum(textureSampleGrad(tex_a, smp, uv + vec2<f32>(du, 0.0), gx, gy).rgb);
        let hw = lum(textureSampleGrad(tex_a, smp, uv - vec2<f32>(du, 0.0), gx, gy).rgb);
        let hn = lum(textureSampleGrad(tex_a, smp, uv - vec2<f32>(0.0, dv), gx, gy).rgb);
        let hs = lum(textureSampleGrad(tex_a, smp, uv + vec2<f32>(0.0, dv), gx, gy).rgb);
        let east = normalize(cross(bz, n) + vec3<f32>(1e-5));
        let north = cross(n, east);
        ns = normalize(n - (east * (he - hw) + north * (hn - hs)) * in.fx.w);
    }

    let ndl = dot(n, l);
    let ndl_s = dot(ns, l);
    var shade = 0.0;
    var col = vec3<f32>(0.0);
    let atm_h = in.fx.x;

    // the planet's own rings cast a shadow across its disc
    var ring_shadow = 1.0;
    if (in.flags.z > 0.5 && ndl > -0.1) {
        let dpl = dot(l, bz);
        if (abs(dpl) > 0.0005) {
            let t = -dot(n, bz) / dpl;
            if (t > 0.0) {
                let hit = n + l * t;
                let rho = length(hit);
                let ru = (rho - in.fx.y) / (in.fx.z - in.fx.y);
                if (ru > 0.0 && ru < 1.0) {
                    let ta = textureSampleLevel(tex_c, smp, vec2<f32>(ru, 0.5), 2.0).a;
                    ring_shadow = 1.0 - 0.82 * ta;
                }
            }
        }
    }

    if (kind == 0) {
        col = in.ca.rgb * (0.72 + 0.32 * nz);
    } else if (kind == 4) {
        // ---- Earth: day, night, clouds, ocean glint, twilight -----------------
        let day_w = smoothstep(-0.04, 0.20, ndl_s);
        let sunlit = pow(max(ndl_s, 0.0), 0.72);
        var surf = albedo;
        var cloud = 0.0;
        if (in.flags.y > 0.5) {
            cloud = smoothstep(0.015, 0.62, cl_tex) * img_fade;
        }
        // deep oceans read as a colour, not as black
        let water = smoothstep(0.015, 0.09, surf.b - surf.r * 1.05) * smoothstep(0.32, 0.12, surf.g) * img_fade;
        surf = mix(surf, surf * vec3<f32>(0.62, 0.95, 1.0), water);
        surf = mix(surf, pow(surf, vec3<f32>(0.82)) * 1.12, (1.0 - water) * img_fade);
        let h = normalize(l + vec3<f32>(0.0, 0.0, 1.0));
        let ndh = max(dot(n, h), 0.0);
        let spec = pow(ndh, 420.0) * 0.55 + pow(ndh, 40.0) * 0.045;
        var lit = surf * (1.18 * sunlit + 0.008);
        lit = lit + vec3<f32>(1.0, 0.92, 0.78) * spec * water * (1.0 - cloud) * day_w;
        // clouds: bright on the day side, faintly self-shadowing the ground
        let cloud_col = vec3<f32>(0.98, 0.97, 0.96) * (0.05 + 1.15 * sunlit);
        lit = mix(lit * (1.0 - 0.32 * cloud), cloud_col, cloud * 0.95);
        // city lights take over past the terminator
        var dark = vec3<f32>(0.0);
        if (in.flags.x > 0.5) {
            let nl = max(night - vec3<f32>(0.018), vec3<f32>(0.0));
            dark = nl * vec3<f32>(1.0, 0.88, 0.7) * 3.4 * (1.0 - 0.75 * cloud) * img_fade;
        } else {
            dark = vec3<f32>(0.0);
        }
        col = lit * ring_shadow + dark * pow(1.0 - day_w, 2.0);
        // the warm band of sunset along the terminator
        let dusk = exp(-pow((ndl - 0.03) / 0.09, 2.0));
        col = col + vec3<f32>(1.0, 0.36, 0.14) * dusk * 0.10 * (0.25 + 0.75 * pow(1.0 - nz, 2.0));
        shade = day_w;
    } else {
        // ---- generic bodies --------------------------------------------------------
        if (kind == 2) {
            // gases: Lambert with limb darkening, a soft twilight edge
            let lam = smoothstep(-0.05, 1.0, ndl_s);
            shade = pow(lam, 0.9) * (0.58 + 0.42 * pow(nz, 0.5));
        } else {
            // airless rock: Lommel-Seeliger keeps the flat, moonlit full-phase look
            let m = max(ndl_s, 0.0);
            let ls = m / (m + max(nz, 0.05) + 0.001) * 2.0;
            shade = mix(m, ls, 0.72);
            shade = shade * smoothstep(-0.02, 0.07, ndl_s);
        }
        let light = amb * 0.25 + (1.0 - amb * 0.25) * shade * 1.04;
        col = albedo * light * ring_shadow;
    }

    // atmosphere: the limb brightens where sunlight scatters, seen edge-on
    var shell = vec3<f32>(0.0);
    if (in.rim.a > 0.001 && atm_h > 0.0) {
        let lit_side = smoothstep(-0.28, 0.42, ndl);
        let path = pow(1.0 - max(nz, 0.0), 2.6);
        col = mix(col, in.rim.rgb * (0.08 + 1.05 * lit_side), clamp(path * in.rim.a * lit_side, 0.0, 0.92));
        col = col + in.rim.rgb * pow(1.0 - max(nz, 0.0), 6.0) * in.rim.a * 0.45 * lit_side;
        if (rr > 1.0 - aa) {
            let limb_n = normalize(vec3<f32>(p.x, -p.y, 0.0));
            let lit_shell = smoothstep(-0.32, 0.5, dot(limb_n, l));
            let d = max(rr - 1.0, 0.0) / atm_h;
            shell = in.rim.rgb * exp(-d * 2.1) * 0.82 * in.rim.a * lit_shell * (1.0 - smoothstep(4.0, 5.0, d));
        }
    }

    // keep the disc's edge from aliasing on the depth test; shell adds light only
    o.color = vec4<f32>(col * alpha + shell * (1.0 - alpha), alpha);
    o.depth = select(1.0, in.depth, alpha > 0.0);
    return o;
}
