// js/scene/cosmos.js — local group catalog, cosmic-web nodes and procedural
// filaments, Oort shell. Catalog tables are ports of src/data.rs
// (LOCAL_GROUP, COSMIC_NODES); the web point layer is PROCEDURAL
// (evidence key 'web-procedural'). All positions sun-centered ecliptic meters.
import * as THREE from 'three';
import { mulberry32, gauss, icrsToEclUnit } from './galaxy.js';

const PC = 3.0856775814913673e16;
const KPC = 1e3 * PC;
const MPC = 1e6 * PC;
const TAU = Math.PI * 2;

const radecToEcl = (raDeg, decDeg, distM) => icrsToEclUnit(raDeg, decDeg).multiplyScalar(distM);

// ------------------------------------------------------------- local group
// port of src/data.rs LOCAL_GROUP: (name, ra, dec, dist kpc, size kpc, spiral)
const LG_ROWS = [
  ['Large Magellanic Cloud', 80.894, -69.756, 50.0, 9.9, false],
  ['Small Magellanic Cloud', 13.187, -72.829, 62.0, 5.8, false],
  ['Sagittarius dSph', 283.831, -30.545, 26.0, 7.0, false],
  ['Draco dSph', 260.052, 57.915, 76.0, 0.7, false],
  ['Ursa Minor dSph', 227.285, 67.222, 76.0, 0.6, false],
  ['Sculptor dSph', 15.039, -33.709, 86.0, 0.8, false],
  ['Sextans dSph', 153.262, -1.615, 95.0, 0.8, false],
  ['Carina dSph', 100.403, -50.966, 105.0, 0.7, false],
  ['Fornax dSph', 39.997, -34.449, 147.0, 2.0, false],
  ['Leo II', 168.370, 22.152, 233.0, 0.7, false],
  ['Leo I', 152.117, 12.306, 254.0, 0.8, false],
  ['Phoenix dwarf', 27.776, -44.445, 415.0, 0.6, false],
  ['NGC 6822', 296.235, -14.789, 500.0, 2.4, false],
  ['IC 10', 5.072, 59.303, 660.0, 1.5, false],
  ['NGC 185', 9.742, 48.337, 617.0, 2.4, false],
  ['NGC 147', 8.301, 48.509, 676.0, 2.7, false],
  ['Andromeda (M31)', 10.685, 41.269, 785.0, 46.0, true],
  ['M32', 10.674, 40.865, 785.0, 2.5, false],
  ['Triangulum (M33)', 23.462, 30.660, 870.0, 18.0, true],
  ['WLM', 0.492, -15.461, 933.0, 3.0, false],
];

export const LOCAL_GROUP = LG_ROWS.map(([name, ra, dec, distKpc, sizeKpc, spiral]) => ({
  name, ra, dec, distKpc, sizeKpc, spiral,
  type: spiral ? 'spiral' : 'dwarf',
  color: spiral ? 0x7ea8d8 : 0x9a8f7a,
  world: radecToEcl(ra, dec, distKpc * KPC),
}));

// ------------------------------------------------------------ cosmic nodes
// port of src/data.rs COSMIC_NODES: (name, ra, dec, dist Mpc, size Mpc)
const NODE_ROWS = [
  ['Virgo Cluster', 187.70, 12.34, 16.5, 4.4],
  ['Fornax Cluster', 54.62, -35.45, 19.0, 2.0],
  ['Ursa Major Cluster', 176.0, 49.0, 18.0, 2.0],
  ['Antlia Cluster', 157.51, -35.32, 41.0, 2.0],
  ['Centaurus Cluster', 192.20, -41.31, 52.0, 3.0],
  ['Pegasus I Cluster', 340.5, 8.2, 53.0, 2.2],
  ['Hydra Cluster', 139.52, -12.10, 58.0, 3.0],
  ['Norma / Great Attractor', 243.9, -60.9, 68.0, 8.0],
  ['Perseus Cluster', 49.95, 41.51, 74.0, 4.5],
  ['Leo Cluster (A1367)', 176.12, 19.84, 92.0, 4.0],
  ['Coma Cluster', 194.95, 27.98, 100.0, 6.0],
  ['Ophiuchus Cluster', 258.1, -23.4, 120.0, 5.0],
  ['Hercules Cluster (A2151)', 241.15, 17.75, 155.0, 5.0],
  ['Shapley Concentration', 202.5, -31.5, 200.0, 12.0],
  ['Horologium-Reticulum', 49.5, -50.6, 210.0, 10.0],
];

export const COSMIC_NODES = NODE_ROWS.map(([name, ra, dec, distMpc, sizeMpc]) => ({
  name, ra, dec, distMpc, sizeMpc,
  world: radecToEcl(ra, dec, distMpc * MPC),
}));

// -------------------------------------------------------------- cosmic web
// ~40k faint points: gaussian blobs at each node plus noisy quadratic-Bezier
// bridges between nodes closer than 40 Mpc (~60 pts/Mpc, 1.5 Mpc jitter).
// Deep blue-violet / ivory mix, brightness 0.15–0.5. PROCEDURAL layer.
const WEB_VIOLET = [0.478, 0.455, 0.784]; // 0x7a74c8
const WEB_IVORY = [0.91, 0.894, 0.847]; //   0xe8e4d8

export function buildCosmicWeb(seededRand = mulberry32(0xc05b1c)) {
  const rand = seededRand;
  const TARGET = 40000;
  const pos = [], col = [], siz = [];

  const putWeb = (v, brightBoost = 0) => {
    pos.push(v.x, v.y, v.z);
    const t = rand() * 0.45;
    const b = Math.min(0.15 + 0.35 * Math.pow(rand(), 1.4) + brightBoost, 0.5);
    col.push(
      (WEB_VIOLET[0] + (WEB_IVORY[0] - WEB_VIOLET[0]) * t) * b,
      (WEB_VIOLET[1] + (WEB_IVORY[1] - WEB_VIOLET[1]) * t) * b,
      (WEB_VIOLET[2] + (WEB_IVORY[2] - WEB_VIOLET[2]) * t) * b,
    );
    siz.push(0.3 + 0.8 * Math.pow(rand(), 2));
  };

  // bridges first (their count is set by geometry), blobs fill to TARGET
  const bridges = [];
  let bridgePts = 0;
  for (let i = 0; i < COSMIC_NODES.length; i++) {
    for (let j = i + 1; j < COSMIC_NODES.length; j++) {
      const d = COSMIC_NODES[i].world.distanceTo(COSMIC_NODES[j].world);
      if (d < 40 * MPC) {
        const npts = Math.round((60 * d) / MPC);
        bridges.push([i, j, npts, d]);
        bridgePts += npts;
      }
    }
  }
  const p = new THREE.Vector3();
  for (const [i, j, npts, d] of bridges) {
    const a = COSMIC_NODES[i].world, b = COSMIC_NODES[j].world;
    // bow the filament: quadratic Bezier whose control point sits off the
    // chord midpoint, perpendicular to the chord
    const chord = new THREE.Vector3().subVectors(b, a).normalize();
    const perp = new THREE.Vector3(gauss(rand), gauss(rand), gauss(rand));
    perp.addScaledVector(chord, -perp.dot(chord)).normalize();
    const c1 = new THREE.Vector3().addVectors(a, b).multiplyScalar(0.5).addScaledVector(perp, (0.08 + 0.12 * rand()) * d);
    for (let k = 0; k < npts; k++) {
      const t = (k + 0.5) / npts;
      const s = 1 - t;
      p.set(
        s * s * a.x + 2 * s * t * c1.x + t * t * b.x,
        s * s * a.y + 2 * s * t * c1.y + t * t * b.y,
        s * s * a.z + 2 * s * t * c1.z + t * t * b.z,
      );
      p.x += gauss(rand) * 1.5 * MPC;
      p.y += gauss(rand) * 1.5 * MPC;
      p.z += gauss(rand) * 1.5 * MPC;
      putWeb(p);
    }
  }

  // node blobs, weighted by cluster size
  const blobTotal = Math.max(TARGET - bridgePts, 5000);
  const wsum = COSMIC_NODES.reduce((s, n) => s + n.sizeMpc, 0);
  for (const node of COSMIC_NODES) {
    const count = Math.round((blobTotal * node.sizeMpc) / wsum);
    const sigma = node.sizeMpc * 0.5 * MPC;
    for (let k = 0; k < count; k++) {
      p.copy(node.world);
      p.x += gauss(rand) * sigma;
      p.y += gauss(rand) * sigma;
      p.z += gauss(rand) * sigma;
      putWeb(p, 0.08 * rand()); // cluster cores glow a touch warmer
    }
  }

  return {
    positions: new Float32Array(pos),
    colors: new Float32Array(col),
    sizes: new Float32Array(siz),
  };
}

// --------------------------------------------------------------- Oort shell
// ~9k faint ivory points, 2e15..1.2e16 m, denser toward the inner edge.
export function buildOort(seededRand = mulberry32(0x0087)) {
  const rand = seededRand;
  const N = 9000;
  const positions = new Float32Array(3 * N);
  const colors = new Float32Array(3 * N);
  const sizes = new Float32Array(N);
  const R_IN = 2e15, R_OUT = 1.2e16;
  for (let i = 0; i < N; i++) {
    const z = 2 * rand() - 1;
    const ph = TAU * rand();
    const rxy = Math.sqrt(Math.max(0, 1 - z * z));
    const r = R_IN + (R_OUT - R_IN) * Math.pow(rand(), 1.7);
    const o = 3 * i;
    positions[o] = r * rxy * Math.cos(ph);
    positions[o + 1] = r * rxy * Math.sin(ph);
    positions[o + 2] = r * z;
    const b = 0.12 + 0.28 * Math.pow(rand(), 2);
    colors[o] = WEB_IVORY[0] * b;
    colors[o + 1] = WEB_IVORY[1] * b;
    colors[o + 2] = WEB_IVORY[2] * b;
    sizes[i] = 0.3 + 0.5 * Math.pow(rand(), 1.5);
  }
  return { positions, colors, sizes };
}
