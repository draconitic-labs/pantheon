// Instanced shaded bodies: orthographic sphere shading with procedural
// vector-ink surfaces. Kinds: 0 ember, 1 rocky, 2 banded giant, 3 star,
// 4 ocean world (Earth; coastlines are CPU vector strokes), 5 icy/cratered.

struct Inst {
    @location(0) center: vec2<f32>,
    @location(1) pr: vec4<f32>,   // radius_px, flatten, kind, seed
    @location(2) sun: vec4<f32>,  // sun dir xyz (view space), ambient
    @location(3) axis: vec4<f32>, // spin axis xyz (view space), rot phase rad
    @location(4) ca: vec4<f32>,
    @location(5) cb: vec4<f32>,
    @location(6) rim: vec4<f32>,  // rim rgb, rim strength
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>, // px offset from center
    @location(1) pr: vec4<f32>,
    @location(2) sun: vec4<f32>,
    @location(3) axis: vec4<f32>,
    @location(4) ca: vec4<f32>,
    @location(5) cb: vec4<f32>,
    @location(6) rim: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let half = inst.pr.x * 1.06 + 2.0;
    let c = corners[vi] * half;
    let p = inst.center + c;
    var o: VsOut;
    o.clip = vec4<f32>(p.x / V.size.x * 2.0 - 1.0, 1.0 - p.y / V.size.y * 2.0, 0.0, 1.0);
    o.local = c;
    o.pr = inst.pr;
    o.sun = inst.sun;
    o.axis = inst.axis;
    o.ca = inst.ca;
    o.cb = inst.cb;
    o.rim = inst.rim;
    return o;
}

fn rotate_about(v: vec3<f32>, axis: vec3<f32>, ang: f32) -> vec3<f32> {
    let c = cos(ang);
    let s = sin(ang);
    return v * c + cross(axis, v) * s + axis * dot(axis, v) * (1.0 - c);
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let radius = max(in.pr.x, 0.5);
    let kind = i32(in.pr.z + 0.5);
    let seed = in.pr.w;
    let axis = normalize(in.axis.xyz);
    let phase = in.axis.w;

    // Oblateness: squash along the screen projection of the axis' perpendicular.
    var p = in.local / radius;
    let ax2 = in.axis.xy;
    if (in.pr.y > 0.001 && length(ax2) > 0.05) {
        let a2 = normalize(ax2);
        let pa = dot(p, a2);
        let pp = dot(p, vec2<f32>(-a2.y, a2.x)) / (1.0 - in.pr.y);
        p = a2 * pa + vec2<f32>(-a2.y, a2.x) * pp;
    }
    let r2 = dot(p, p);
    let rr = sqrt(r2);
    let aa = 1.5 / radius;
    let alpha = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, rr);
    if (alpha <= 0.003) {
        return vec4<f32>(0.0);
    }
    let nz = sqrt(max(1.0 - r2, 0.0));
    let n = vec3<f32>(p.x, -p.y, nz); // view space, y up

    // Surface frame: latitude from axis, longitude in rotating frame.
    let lat = asin(clamp(dot(n, axis), -1.0, 1.0));
    var e1 = cross(axis, vec3<f32>(0.0, 0.0, 1.0));
    if (length(e1) < 0.1) {
        e1 = cross(axis, vec3<f32>(0.0, 1.0, 0.0));
    }
    e1 = normalize(e1);
    let e2 = cross(axis, e1);
    let lon = atan2(dot(n, e2), dot(n, e1)) + phase;
    let q = vec3<f32>(cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat)) + vec3<f32>(seed * 13.7);

    let l = normalize(in.sun.xyz);
    let amb = in.sun.w;
    let diff = smoothstep(-0.06, 0.30, dot(n, l));
    var col = in.ca.rgb;
    var lit = true;

    if (kind == 0) { // ember: tiny far body, softly self-luminous
        col = in.ca.rgb * (0.72 + 0.32 * nz);
        lit = false;
    } else if (kind == 1) { // rocky
        let m = fbm3(q * 3.1);
        col = mix(in.cb.rgb, in.ca.rgb, m);
        let c = fbm3(q * 7.3 + 11.0);
        col = col * (1.0 - smoothstep(0.60, 0.74, c) * 0.38);
    } else if (kind == 2) { // banded giant
        let turb = fbm3(q * 2.4) - 0.5;
        let bands = 0.5 + 0.5 * sin(lat * 11.0 + turb * 3.4 + seed * 5.0);
        col = mix(in.ca.rgb, in.cb.rgb, bands);
        col = col * (0.92 + 0.16 * (fbm3(q * 4.0) - 0.5));
        if (seed > 0.55) { // great storm oval
            let sl = lat + 0.38;
            let so = atan2(sin(lon - 1.3), cos(lon - 1.3));
            let d = sl * sl * 30.0 + so * so * 3.2;
            let spot = exp(-d * 2.2);
            col = mix(col, vec3<f32>(0.82, 0.44, 0.30), spot * 0.75);
        }
    } else if (kind == 3) { // star
        let g = fbm3(q * 5.0 + vec3<f32>(V.size.z * 0.03, 0.0, 0.0));
        let limb = 1.0 - 0.62 * pow(1.0 - nz, 0.72);
        col = in.ca.rgb * (0.82 + 0.36 * g) * limb * 1.5;
        lit = false;
    } else if (kind == 4) { // ocean world
        let ice = smoothstep(1.16, 1.34, abs(lat));
        let cl = fbm3(q * 2.9 + vec3<f32>(V.size.z * 0.008, 0.0, 0.0));
        let cloud = smoothstep(0.52, 0.76, cl) * 0.5;
        col = mix(in.ca.rgb, vec3<f32>(0.92, 0.95, 1.0), max(ice * 0.85, cloud));
        let h = normalize(l + vec3<f32>(0.0, 0.0, 1.0));
        col = col + vec3<f32>(0.9, 0.95, 1.0) * pow(max(dot(n, h), 0.0), 70.0) * 0.45 * (1.0 - cloud);
    } else { // icy / cratered
        let m = fbm3(q * 4.2);
        col = mix(in.cb.rgb, in.ca.rgb, m);
        let c = fbm3(q * 9.0 + 5.0);
        col = col * (1.0 - smoothstep(0.58, 0.75, c) * 0.30);
    }

    if (lit) {
        col = col * (amb + (1.0 - amb) * diff);
        let rimf = pow(1.0 - nz, 2.6) * in.rim.a;
        col = col + in.rim.rgb * rimf * (0.25 + 0.75 * diff);
    }
    return vec4<f32>(col * alpha, alpha);
}
