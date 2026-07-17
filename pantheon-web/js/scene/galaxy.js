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
function galacticBasis() {
  const z = icrsToEclUnit(192.85948, 27.12825);
  const x0 = icrsToEclUnit(266.405, -28.93617);
  const x = x0.clone().addScaledVector(z, -x0.dot(z)).normalize();
  const y = new THREE.Vector3().crossVectors(z, x);
  return { x, y, z };
}

// ---------------------------------------------------------------- palette
const hx = (h) => [((h >> 16) & 255) / 255, ((h >> 8) & 255) / 255, (h & 255) / 255];
const GOLD = hx(0xd8b36a);
const GOLD_PALE = hx(0xe8d0a8);
const IVORY = hx(0xe8e4d8);
const ARM_BLUE = hx(0xa8c4e8);
const HII_PINK = hx(0xe8a0b4);
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
    // dust lane: |z| < 80 pc slab, 3–8 kpc, inter-arm — keep 40–60 % brightness
    if (Math.abs(z) < P.dust.zLane) {
      const r = Math.hypot(x, y);
      if (r > P.dust.rIn && r < P.dust.rOut && armDist(r, Math.atan2(y, x)) > P.dust.armGap) {
        bright *= 0.4 + 0.2 * rand();
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
    colors[o + 1] = rgb[1] * bright;
    colors[o + 2] = rgb[2] * bright;
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
