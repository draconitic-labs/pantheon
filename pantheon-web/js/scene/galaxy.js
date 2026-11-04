// js/scene/galaxy.js — deterministic particle galaxies (Milky Way + generic discs).
// buildMilkyWay returns GALACTOCENTRIC meters pre-rotated into ecliptic-aligned
// axes (SPEC "Galactic frame"); main.js places the cloud group at the
// galactic-center world position. buildGalaxyDisc returns a local-frame disc
// (plane = local XY) that the caller orients and positions.
import * as THREE from 'three';

const PC = 3.0856775814913673e16;
const KPC = 1e3 * PC;
const OBLIQUITY_DEG = 23.4392911;
const TAU = Math.PI * 2;

// Deterministic 32-bit PRNG. Shared seed source for every procedural layer
// (cosmos.js imports this rather than growing its own).
export function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// Standard normal via Box–Muller, driven by the caller's seeded rand.
export function gauss(rand) {
  let u;
  do { u = rand(); } while (u <= 1e-12);
  return Math.sqrt(-2 * Math.log(u)) * Math.cos(TAU * rand());
}

// RA/Dec (deg, ICRS) -> ecliptic unit vector, using the exact SPEC matrix
// u_ecl = M u_icrs (rows [1,0,0],[0,c,s],[0,-s,c]) — transposing it is known
// past bug FND-001.
export function icrsToEclUnit(raDeg, decDeg) {
  const ra = (raDeg * Math.PI) / 180, dec = (decDeg * Math.PI) / 180;
  const x = Math.cos(dec) * Math.cos(ra);
  const y = Math.cos(dec) * Math.sin(ra);
  const z = Math.sin(dec);
  const e = (OBLIQUITY_DEG * Math.PI) / 180, c = Math.cos(e), s = Math.sin(e);
  return new THREE.Vector3(x, c * y + s * z, -s * y + c * z);
}

// Galactic basis expressed in ecliptic axes: +X toward the galactic center,
// +Z toward the north galactic pole, +Y = Z × X (X re-orthonormalized vs Z).
export function galacticBasis() {
  const z = icrsToEclUnit(192.85948, 27.12825);
  const x0 = icrsToEclUnit(266.405, -28.93617);
  const x = x0.clone().addScaledVector(z, -x0.dot(z)).normalize();
  const y = new THREE.Vector3().crossVectors(z, x);
  return { x, y, z };
}

// ---------------------------------------------------------------- palette
const hx = (h) => [((h >> 16) & 255) / 255, ((h >> 8) & 255) / 255, (h & 255) / 255];
// deeper chroma than the UI palette: additive stacking + ACES desaturate,
// so the source colors must overshoot for hue to survive in dense regions
const GOLD = hx(0xd89e42);
const GOLD_PALE = hx(0xe8c88e);
const IVORY = hx(0xe8e2d0);
const ARM_BLUE = hx(0x6f96e8);
const HII_PINK = hx(0xe87792);
const mix = (a, b, t) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];

// ------------------------------------------------------------ core builder
// One generator drives both the Milky Way and the M31/M33 minis: exponential
// disc (thin + thick), triaxial bulge, log-spiral arms with HII sprinkles,
// and an inter-arm dust lane that dims (never blackens) low-|z| points.
function genGalaxy(rand, P) {
  const { counts, basis } = P;
  const n = counts.bulge + counts.thin + counts.thick + counts.arm + counts.hii;
  const positions = new Float32Array(3 * n);
  const colors = new Float32Array(3 * n);
  const sizes = new Float32Array(n);
  const tanPitch = Math.tan((P.pitchDeg * Math.PI) / 180);
  let i = 0;

  const armTheta = (k, r) => (k * TAU) / P.arms + Math.log(r / P.r0) / tanPitch;
  const wrapPi = (a) => {
    a %= TAU;
    return a > Math.PI ? a - TAU : a < -Math.PI ? a + TAU : a;
  };
  // physical (azimuthal) distance to the nearest arm crest, meters
  const armDist = (r, th) => {
    let best = Infinity;
    for (let k = 0; k < P.arms; k++) {
      const d = Math.abs(wrapPi(th - armTheta(k, r))) * r;
      if (d < best) best = d;
    }
    return best;
  };
  // Σ(r) ∝ r·exp(−r/h): sum of two exponential draws, rejected past cutoff
  const discR = (h) => {
    let r;
    do { r = -h * Math.log(rand() * rand() + 1e-12); } while (r >= P.rCut);
    return r;
  };

  const put = (x, y, z, rgb, bright, size) => {
    // dust lane: |z| < 80 pc slab, 3–8 kpc, inter-arm — dimmed AND reddened
    // (extinction reddens; palettes are shared consts, so tint locally)
    let tg = 1, tb = 1;
    if (Math.abs(z) < P.dust.zLane) {
      const r = Math.hypot(x, y);
      if (r > P.dust.rIn && r < P.dust.rOut && armDist(r, Math.atan2(y, x)) > P.dust.armGap) {
        bright *= 0.4 + 0.2 * rand();
        tg = 0.74; tb = 0.5;
      }
    }
    const o = 3 * i;
    if (basis) {
      positions[o] = basis.x.x * x + basis.y.x * y + basis.z.x * z;
      positions[o + 1] = basis.x.y * x + basis.y.y * y + basis.z.y * z;
      positions[o + 2] = basis.x.z * x + basis.y.z * y + basis.z.z * z;
    } else {
      positions[o] = x; positions[o + 1] = y; positions[o + 2] = z;
    }
    colors[o] = rgb[0] * bright;
    colors[o + 1] = rgb[1] * bright * tg;
    colors[o + 2] = rgb[2] * bright * tb;
    sizes[i] = size;
    i++;
  };

  // bulge — triaxial gaussian, warm gold-white, hot dense core
  for (let k = 0; k < counts.bulge; k++) {
    const x = gauss(rand) * P.bulge[0];
    const y = gauss(rand) * P.bulge[1];
    const z = gauss(rand) * P.bulge[2];
    const core = Math.exp(-(x * x + y * y + z * z) / (2 * P.bulge[0] * P.bulge[0]));
    const rgb = mix(GOLD, GOLD_PALE, rand());
    const b = Math.min(0.5 + 0.55 * Math.pow(rand(), 1.6) + 0.35 * core, 1.4);
    put(x, y, z, rgb, b, 0.4 + 1.3 * Math.pow(rand(), 2) + 0.3 * core);
  }

  // thin disc — ivory, gold-tinted inward, mildly flared past 10 kpc,
  // rare bright giants so the field sparkles instead of reading flat
  for (let k = 0; k < counts.thin; k++) {
    const r = discR(P.h), th = rand() * TAU;
    const flare = 1 + Math.max(0, (r - 10 * KPC) / (10 * KPC)) * 0.8;
    const z = gauss(rand) * P.zThin * flare;
    const rgb = mix(IVORY, GOLD_PALE, 0.55 * Math.exp(-r / (5 * KPC)) + 0.1 * rand());
    let b = 0.4 + 0.6 * Math.pow(rand(), 2.2);
    let s = 0.4 + 1.1 * Math.pow(rand(), 3);
    if (rand() < 0.012) { b = 1.1 + 0.3 * rand(); s = 1.6 + 1.0 * rand(); }
    put(r * Math.cos(th), r * Math.sin(th), z, rgb, b, s);
  }

  // thick disc — sparse, old, faint, slightly warmer
  for (let k = 0; k < counts.thick; k++) {
    const r = discR(P.h * 1.15), th = rand() * TAU;
    const z = gauss(rand) * P.zThick;
    const rgb = mix(IVORY, GOLD, 0.3 + 0.2 * rand());
    put(r * Math.cos(th), r * Math.sin(th), z, rgb, 0.4 + 0.35 * Math.pow(rand(), 2), 0.4 + 0.7 * Math.pow(rand(), 2.5));
  }

  // spiral arms — log-spiral crests, gaussian azimuthal width that stays
  // roughly constant in physical units, blue-white with OB sparkle
  const armR = () => {
    let r;
    do { r = discR(P.hArm); } while (r < P.r0 * 0.85);
    return r;
  };
  for (let k = 0; k < counts.arm; k++) {
    const arm = Math.floor(rand() * P.arms);
    const r = armR();
    const sigTh = Math.min(Math.max((0.6 * KPC) / r, 0.045), 0.3);
    const th = armTheta(arm, r) + gauss(rand) * sigTh;
    const rr = r + gauss(rand) * 0.3 * KPC;
    const z = gauss(rand) * P.zArm;
    const rgb = mix(ARM_BLUE, IVORY, 0.55 * rand());
    let b = 0.5 + 0.65 * Math.pow(rand(), 2);
    let s = 0.45 + 1.2 * Math.pow(rand(), 2.5);
    if (rand() < 0.05) { b = 1.1 + 0.3 * rand(); s = 1.5 + 1.1 * rand(); }
    put(rr * Math.cos(th), rr * Math.sin(th), z, rgb, b, s);
  }

  // HII regions — pink beads strung tight along the crests
  for (let k = 0; k < counts.hii; k++) {
    const arm = Math.floor(rand() * P.arms);
    const r = armR();
    const sigTh = Math.min(Math.max((0.6 * KPC) / r, 0.045), 0.3) * 0.45;
    const th = armTheta(arm, r) + gauss(rand) * sigTh;
    const z = gauss(rand) * P.zArm * 0.6;
    put(r * Math.cos(th), r * Math.sin(th), z, HII_PINK, Math.min(0.9 + 0.5 * rand(), 1.4), 1.3 + 1.3 * rand());
  }

  return { positions, colors, sizes };
}

// ---------------------------------------------------------------- exports

// The Milky Way: ~140k points, deterministic for a given seeded rand.
// Galactocentric meters already rotated into ecliptic-aligned axes.
export function buildMilkyWay(seededRand) {
  return genGalaxy(seededRand, {
    counts: { bulge: 24000, thin: 52000, thick: 12000, arm: 50000, hii: 1000 },
    h: 2.6 * KPC,
    hArm: 3.4 * KPC, // arms sampled a touch shallower so they read to the rim
    rCut: 20 * KPC,
    zThin: 300 * PC,
    zThick: 900 * PC,
    bulge: [1.5 * KPC, 1.0 * KPC, 0.8 * KPC],
    arms: 4,
    pitchDeg: 12.5,
    r0: 3.0 * KPC,
    zArm: 170 * PC,
    dust: { rIn: 3 * KPC, rOut: 8 * KPC, zLane: 80 * PC, armGap: 0.9 * KPC },
    basis: galacticBasis(),
  });
}

// Generic disc for the M31/M33 minis (~25k pts): local frame, disc in the XY
// plane (+Z normal), all lengths scaled by `scale` (scale=1 → 40 kpc diameter).
// `tilt` (radians, about local X) is a convenience; the caller owns final
// orientation and position.
export function buildGalaxyDisc(seededRand, { scale = 1, count = 25000, arms = 2, pitchDeg = 13, tilt = 0 } = {}) {
  let basis = null;
  if (tilt) {
    const c = Math.cos(tilt), s = Math.sin(tilt);
    basis = {
      x: new THREE.Vector3(1, 0, 0),
      y: new THREE.Vector3(0, c, s),
      z: new THREE.Vector3(0, -s, c),
    };
  }
  const nb = Math.round(count * 0.18);
  const nk = Math.round(count * 0.08);
  const na = Math.round(count * 0.35);
  const nh = Math.round(count * 0.02);
  return genGalaxy(seededRand, {
    counts: { bulge: nb, thin: count - nb - nk - na - nh, thick: nk, arm: na, hii: nh },
    h: 2.6 * KPC * scale,
    hArm: 3.4 * KPC * scale,
    rCut: 20 * KPC * scale,
    zThin: 300 * PC * scale,
    zThick: 900 * PC * scale,
    bulge: [1.5 * KPC * scale, 1.0 * KPC * scale, 0.8 * KPC * scale],
    arms,
    pitchDeg,
    r0: 3.0 * KPC * scale,
    zArm: 170 * PC * scale,
    dust: { rIn: 3 * KPC * scale, rOut: 8 * KPC * scale, zLane: 80 * PC * scale, armGap: 0.9 * KPC * scale },
    basis,
  });
}

// ---------------------------------------------------------------- field stars
// Synthetic stars from 40 pc to 3 kpc that carry parallax and depth where the Gaia
// catalog ends. Illustrative, labeled 'stars-synthetic', invisible near the Sun.
// Sun-centered ecliptic meters, in the same shape makeStarPoints takes.
export function buildFieldStars(rand, count = 180000) {
  const B = galacticBasis();
  const positions = new Float32Array(3 * count), colors = new Float32Array(3 * count), lum = new Float32Array(count);
  const HOT = [0.62, 0.74, 1.0], SUN = [1.0, 0.95, 0.86], COOL = [1.0, 0.72, 0.5];
  for (let i = 0; i < count; i++) {
    // a disc around the Sun: thin in z, denser close in, never inside 40 pc
    let r, th, z, d;
    do {
      r = 3000 * Math.pow(rand(), 0.72) * PC;
      th = rand() * TAU;
      z = gauss(rand) * (rand() < 0.86 ? 130 : 420) * PC;
      d = Math.hypot(r, z);
    } while (d < 40 * PC);
    const x = r * Math.cos(th), y = r * Math.sin(th);
    positions[3 * i] = B.x.x * x + B.y.x * y + B.z.x * z;
    positions[3 * i + 1] = B.x.y * x + B.y.y * y + B.z.y * z;
    positions[3 * i + 2] = B.x.z * x + B.y.z * y + B.z.z * z;
    const u = rand();
    const mg = u < 0.04 ? -3 + 3 * rand() : u < 0.32 ? 0 + 2.5 * rand() : 2.5 + 3.5 * rand();
    lum[i] = Math.pow(10, -0.4 * mg);
    const c = u < 0.1 ? HOT : u < 0.55 ? SUN : COOL;
    colors.set(c, 3 * i);
  }
  return { count, positions, colors, lum };
}

// ------------------------------------------------------- point-cloud rendering
// Extended emission with constant surface brightness: angular size falls as 1/d and
// sprites fade once they would be sub-pixel or fully resolved (camera inside).
const cloudShared = { uPxScale: { value: 823 }, uPR: { value: 1 } };
export const cloudUniforms = cloudShared;

const CLOUD_VSH = /* glsl */`
  attribute vec3 col; attribute float siz;
  varying vec3 vCol; varying float vA, vRatio, vAng;
  uniform float uBlobPc, uPc, uB, uMaxPx, uPxScale, uPR, uMinPx;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vCol = col;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    float d = length(mv.xyz / uPc);
    float raw = uBlobPc * siz / d * uPxScale;
    float px = clamp(raw, uMinPx, uMaxPx);
    float a = uB;
    if (raw > uMaxPx) a *= (uMaxPx * uMaxPx) / (raw * raw);
    a *= clamp(raw / uMinPx, 0.0, 1.0);
    vA = a;
    float h = fract(float(gl_VertexID) * 0.61803398875);
    vRatio = 0.3 + 0.7 * fract(h * 7.13);
    vAng = h * 6.2831853;
    if (a < 0.0004) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); gl_PointSize = 0.0; return; }
    gl_PointSize = px * uPR;
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const CLOUD_FSH = /* glsl */`
  varying vec3 vCol; varying float vA, vRatio, vAng;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    vec2 pc = gl_PointCoord * 2.0 - 1.0;
    float c = cos(vAng), s = sin(vAng);
    pc = vec2(c * pc.x - s * pc.y, s * pc.x + c * pc.y);
    pc.y /= vRatio;
    float d2 = dot(pc, pc);
    if (d2 > 1.0) discard;
    gl_FragColor = vec4(vCol * (exp(-d2 * 8.0) + 0.22 * exp(-d2 * 2.6) * (1.0 - d2)) * vA, 1.0);
  }
`;

export function makeCloudPoints(cloud, { blobM, bright, maxPx = 9, minPx = 1.4 }) {
  const count = cloud.positions.length / 3;
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(cloud.positions, 3));
  g.setAttribute('col', new THREE.BufferAttribute(cloud.colors, 3));
  g.setAttribute('siz', new THREE.BufferAttribute(cloud.sizes || new Float32Array(count).fill(1), 1));
  g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e26);
  const mat = new THREE.ShaderMaterial({
    vertexShader: CLOUD_VSH, fragmentShader: CLOUD_FSH,
    uniforms: {
      uBlobPc: { value: blobM / PC }, uPc: { value: PC }, uB: { value: bright },
      uMaxPx: { value: maxPx }, uMinPx: { value: minPx },
      uPxScale: cloudShared.uPxScale, uPR: cloudShared.uPR,
    },
    blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
  });
  const pts = new THREE.Points(g, mat);
  pts.frustumCulled = false;
  pts.renderOrder = -6;
  pts.userData.count = count;
  return pts;
}

// ------------------------------------------------------------ volumetric galaxies
// A face-on map (old stars, young stars, dust, HII) is baked once on the GPU with
// the same spiral law as the point cloud. A low-resolution pass then marches each
// galaxy's slab per pixel: emission and dust extinction, so lanes truly occlude.
// Illustrative for all of them; the Milky Way's arms are stylized, not surveyed.
const FS_V = /* glsl */`
  varying vec2 vUv;
  void main() { vUv = uv; gl_Position = vec4(position.xy, 0.0, 1.0); }
`;
const GNOISE = /* glsl */`
  float hash1(vec3 p) {
    p = fract(p * 0.3183099 + vec3(0.1, 0.17, 0.13));
    p *= 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
  }
  float vnoise(vec3 p) {
    vec3 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
      mix(mix(hash1(i), hash1(i + vec3(1,0,0)), f.x), mix(hash1(i + vec3(0,1,0)), hash1(i + vec3(1,1,0)), f.x), f.y),
      mix(mix(hash1(i + vec3(0,0,1)), hash1(i + vec3(1,0,1)), f.x), mix(hash1(i + vec3(0,1,1)), hash1(i + vec3(1,1,1)), f.x), f.y), f.z);
  }
  float fbm(vec3 p) {
    float a = 0.5, s = 0.0;
    for (int i = 0; i < 5; i++) { s += a * vnoise(p); p *= 2.03; a *= 0.5; }
    return s;
  }
`;
const BAKE_F = /* glsl */`
  varying vec2 vUv;
  ${GNOISE}
  float wrapPi(float a) { return a - 6.2831853 * floor((a + 3.14159265) / 6.2831853); }
  void main() {
    vec2 p = (vUv * 2.0 - 1.0) * 22.0;
    vec3 q = vec3(p, 0.0);
    vec2 w = vec2(fbm(q * 0.16 + 3.1), fbm(q * 0.16 + 11.7)) - 0.5;
    vec2 pw = p + w * 2.6;
    float r = max(length(pw), 0.2), rr = length(p), th = atan(pw.y, pw.x);
    float arm = 0.0, inner = 0.0, spur = 0.0;
    for (int k = 0; k < 4; k++) {
      float crest = float(k) * 1.5707963 + log(r / 3.0) / 0.2217;   // tan(12.5 deg), as genGalaxy
      float d = wrapPi(th - crest) * r;
      float sig = 0.5 + 0.06 * r;
      // each arm brightens and frays along its length instead of a clean hose
      float along = fbm(vec3(cos(th + float(k)) * r * 0.22, sin(th + float(k)) * r * 0.22, float(k) * 7.0));
      float str = ((k == 0 || k == 2) ? 1.0 : 0.62) * smoothstep(0.26, 0.62, along + 0.12);
      arm = max(arm, str * exp(-d * d / (2.0 * sig * sig)));
      float dd = d - sig * 1.0;
      inner = max(inner, str * exp(-dd * dd / (2.0 * sig * sig * 0.22)));
      // feathers between the arms, pitched more steeply
      float c2 = float(k) * 1.5707963 + 0.72 + log(r / 3.0) / 0.34;
      float d2 = wrapPi(th - c2) * r;
      spur = max(spur, 0.42 * smoothstep(0.34, 0.7, along) * exp(-d2 * d2 / (2.0 * sig * sig * 0.55)));
    }
    arm = max(arm, spur);
    arm *= smoothstep(2.2, 4.6, rr);
    float cl = fbm(q * 1.15 + 5.0), cl2 = fbm(q * 3.9 + 9.0), cl3 = fbm(q * 10.0 + 2.0);
    float disc = exp(-rr / 3.6) * smoothstep(21.0, 13.0, rr);
    float old = disc * (0.95 + 0.85 * arm) * (0.72 + 0.56 * cl);
    float young = exp(-rr / 7.5) * smoothstep(21.0, 12.0, rr) * pow(arm, 1.15) * (0.08 + 4.2 * cl2 * cl);
    float dust = exp(-rr / 5.2) * smoothstep(1.4, 3.0, rr) * smoothstep(19.0, 9.0, rr)
               * (inner * (0.45 + 1.5 * cl3) + 0.14 * cl2);
    float hii = young * pow(clamp(fbm(q * 7.0 + 13.0) * 1.7 - 0.28, 0.0, 1.0), 6.0) * 5.0;
    float ca = cos(0.47), sa = sin(0.47);
    vec2 b = vec2(ca * p.x + sa * p.y, -sa * p.x + ca * p.y);
    old += exp(-(b.x * b.x / 10.9 + b.y * b.y / 1.1)) * 0.8;
    float lane = exp(-pow(abs(b.y) - 0.72, 2.0) / 0.035) * exp(-b.x * b.x / 9.0);
    dust += lane * 0.6;
    gl_FragColor = vec4(old, young, dust, hii);
  }
`;
const MARCH_F = /* glsl */`
  varying vec2 vUv;
  uniform sampler2D uMap;
  uniform highp sampler3D uNoise;
  uniform int uCount, uSteps;
  uniform mat3 uRot;
  uniform vec2 uTan;
  uniform vec3 uRo[5];
  uniform mat3 uM[5];
  uniform float uI[5];
  uniform float uFrame;
  float ign(vec2 p) { return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715)))); }
  void main() {
    vec3 rdW = normalize(uRot * vec3((vUv * 2.0 - 1.0) * uTan, -1.0));
    vec3 total = vec3(0.0);
    float j = ign(gl_FragCoord.xy + uFrame * vec2(5.3, 1.7));
    for (int g = 0; g < uCount; g++) {
      if (uI[g] <= 0.001) continue;
      vec3 ro = uRo[g], rd = uM[g] * rdW;
      vec3 inv = 1.0 / (rd + vec3(1e-7));
      vec3 t0s = (vec3(-21.5, -21.5, -4.0) - ro) * inv, t1s = (vec3(21.5, 21.5, 4.0) - ro) * inv;
      vec3 tn = min(t0s, t1s), tf = max(t0s, t1s);
      float t0 = max(max(tn.x, max(tn.y, tn.z)), 0.0), t1 = min(tf.x, min(tf.y, tf.z));
      if (t1 <= t0) continue;
      float dt = (t1 - t0) / float(uSteps);
      vec3 L = vec3(0.0);
      float T = 1.0;
      for (int i = 0; i < uSteps; i++) {
        vec3 p = ro + rd * (t0 + dt * (float(i) + j));
        vec4 m = texture2D(uMap, p.xy / 44.0 + 0.5);
        float z = abs(p.z);
        float cb = 0.8914, sb = 0.4529;   // bar angle, 27 deg
        vec2 qb = vec2(cb * p.x + sb * p.y, -sb * p.x + cb * p.y);
        float b2 = qb.x * qb.x / 4.8 + qb.y * qb.y / 1.56 + p.z * p.z / 1.1;
        float bul = exp(-sqrt(b2) * 2.3);
        vec3 E = vec3(1.0, 0.8, 0.56) * (m.r * exp(-z / 0.4) * 1.3 + bul * 3.0)
               + vec3(0.46, 0.66, 1.0) * m.g * exp(-z / 0.16) * 4.2
               + vec3(1.0, 0.3, 0.42) * m.a * exp(-z / 0.1) * 2.2;
        float kap = m.b * exp(-z / 0.1) * 9.0 * (0.25 + 1.5 * texture(uNoise, p * 0.11).r);
        L += T * E * dt;
        T *= exp(-kap * dt);
        if (T < 0.015) break;
      }
      // long paths through the disc saturate softly instead of blowing out
      total += (vec3(1.0) - exp(-L * 0.85)) / 0.85 * uI[g];
    }
    gl_FragColor = vec4(total, 1.0);
  }
`;

export class GalaxyVolume {
  constructor(renderer) {
    this.renderer = renderer;
    this.instances = [];   // {name, center (m), M: Matrix3 world->local, s: m per local unit, I}
    this.steps = 36; this.div = 2; this.frame = 0; this.active = false;
    this.rt = null;
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 3, -1, 0, -1, 3, 0]), 3));
    geo.setAttribute('uv', new THREE.BufferAttribute(new Float32Array([0, 0, 2, 0, 0, 2]), 2));
    this.scene = new THREE.Scene();
    this.cam = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
    this.map = this._bake(geo);
    this.noise = this._noise();
    this.mat = new THREE.ShaderMaterial({
      vertexShader: FS_V, fragmentShader: MARCH_F,
      uniforms: {
        uMap: { value: this.map }, uNoise: { value: this.noise }, uCount: { value: 0 }, uSteps: { value: 36 },
        uRot: { value: new THREE.Matrix3() }, uTan: { value: new THREE.Vector2(1, 1) },
        uRo: { value: Array.from({ length: 5 }, () => new THREE.Vector3()) },
        uM: { value: Array.from({ length: 5 }, () => new THREE.Matrix3()) },
        uI: { value: [0, 0, 0, 0, 0] }, uFrame: { value: 0 },
      },
      depthTest: false, depthWrite: false,
    });
    this.tri = new THREE.Mesh(geo, this.mat);
    this.tri.frustumCulled = false;
    this.scene.add(this.tri);
  }

  _bake(geo) {
    const size = 1536;
    const rt = new THREE.WebGLRenderTarget(size, size, {
      type: THREE.HalfFloatType, format: THREE.RGBAFormat, depthBuffer: false,
      generateMipmaps: true, minFilter: THREE.LinearMipmapLinearFilter, magFilter: THREE.LinearFilter,
    });
    const s = new THREE.Scene();
    s.add(new THREE.Mesh(geo, new THREE.ShaderMaterial({ vertexShader: FS_V, fragmentShader: BAKE_F, depthTest: false, depthWrite: false })));
    s.children[0].frustumCulled = false;
    this.renderer.setRenderTarget(rt);
    this.renderer.render(s, this.cam);
    this.renderer.setRenderTarget(null);
    return rt.texture;
  }

  _noise() {
    const N = 32, d = new Uint8Array(N * N * N), r = mulberry32(90125);
    for (let i = 0; i < d.length; i++) d[i] = Math.floor(r() * 256);
    const t = new THREE.Data3DTexture(d, N, N, N);
    t.format = THREE.RedFormat; t.type = THREE.UnsignedByteType;
    t.minFilter = THREE.LinearFilter; t.magFilter = THREE.LinearFilter;
    t.wrapS = t.wrapT = t.wrapR = THREE.RepeatWrapping;
    t.needsUpdate = true;
    return t;
  }

  setQuality(q) { this.steps = q.vol[0]; this.div = q.vol[1]; this.mat.uniforms.uSteps.value = this.steps; this._size(); }

  setSize(w, h) { this.w = w; this.h = h; this._size(); }

  _size() {
    if (!this.w) return;
    const rw = Math.max(2, Math.round(this.w / this.div)), rh = Math.max(2, Math.round(this.h / this.div));
    if (this.rt && this.rt.width === rw && this.rt.height === rh) return;
    if (this.rt) this.rt.dispose();
    this.rt = new THREE.WebGLRenderTarget(rw, rh, {
      type: THREE.HalfFloatType, format: THREE.RGBAFormat, depthBuffer: false,
      minFilter: THREE.LinearFilter, magFilter: THREE.LinearFilter,
    });
  }

  // basis: {x, y, z} local axes in ecliptic coordinates; s: meters per local unit
  add(name, center, basis, s, I = 1) {
    const M = new THREE.Matrix3().set(
      basis.x.x, basis.x.y, basis.x.z,
      basis.y.x, basis.y.y, basis.y.z,
      basis.z.x, basis.z.y, basis.z.z,
    );
    this.instances.push({ name, center, M, s, I, fade: 0 });
  }

  // pxScale: pixels per unit tangent. Returns how much of each galaxy is drawn.
  update(camWorld, camera, pxScale, nearFade) {
    const u = this.mat.uniforms;
    let any = false;
    const e = camera.matrixWorld.elements;
    u.uRot.value.set(e[0], e[4], e[8], e[1], e[5], e[9], e[2], e[6], e[10]);
    const ty = Math.tan(camera.fov * Math.PI / 360);
    u.uTan.value.set(ty * camera.aspect, ty);
    u.uCount.value = this.instances.length;
    const v = new THREE.Vector3();
    this.instances.forEach((g, i) => {
      v.copy(camWorld).sub(g.center);
      const d = Math.max(v.length(), 1);
      v.divideScalar(g.s).applyMatrix3(g.M);
      u.uRo.value[i].copy(v);
      u.uM.value[i].copy(g.M);
      const px = (20 * g.s / d) * pxScale;
      // fadePx hands over to the far sprite; nearFade hands over to the photograph,
      // which already shows these galaxies from near the Sun
      g.fadePx = THREE.MathUtils.smoothstep(px, 2.5, 9);
      const f = g.fadePx * nearFade;
      g.fade = f;
      // from inside a disc the sky is dark and only the band is bright: the eye is
      // adapted to starlight, not to the whole galaxy at once
      const out = Math.max(Math.abs(v.z), Math.hypot(v.x, v.y) - 16);
      const inside = 1 - THREE.MathUtils.smoothstep(out, 0.4, 5);
      u.uI.value[i] = g.I * f * (1 - 0.78 * inside);
      if (f > 0.001) any = true;
    });
    u.uFrame.value = (this.frame = (this.frame + 1) % 64);
    this.active = any;
    return any;
  }

  render() {
    if (!this.active || !this.rt) return;
    const r = this.renderer;
    r.setRenderTarget(this.rt);
    r.render(this.scene, this.cam);
    r.setRenderTarget(null);
  }
}
