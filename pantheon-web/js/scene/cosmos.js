// js/scene/cosmos.js — local group catalog, cosmic-web nodes and procedural
// filaments, Oort shell. Catalog tables are ports of src/data.rs
// (LOCAL_GROUP, COSMIC_NODES); the web layers are PROCEDURAL
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
  ['Pegasus I Cluster', 350.13, 8.19, 53.0, 2.2],
  // Abell 1060, which NASA NED resolves "Hydra Cluster" to; not Hydra A (Abell 780)
  ['Hydra Cluster', 159.18, -27.53, 58.0, 3.0],
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
// PROCEDURAL. Real catalog clusters anchor a synthetic network: random seed nodes
// in a 420 Mpc sphere joined to their nearest neighbours, with galaxies strung
// along the edges and piled into the nodes. Returns a sharp galaxy layer and a soft
// fog layer along the same filaments. Order is shuffled so a draw range can thin it.
const WEB_VIOLET = [0.34, 0.42, 1.0];
const WEB_ICE = [0.72, 0.84, 1.0];
const WEB_WARM = [1.0, 0.78, 0.55];
const WEB_IVORY = [0.91, 0.894, 0.847];

export function buildCosmicWeb(seededRand = mulberry32(0xc05b1c), { galaxies = 340000, fog = 26000 } = {}) {
  const rand = seededRand;
  const nodes = COSMIC_NODES.map(n => ({ p: n.world.clone(), w: 1.5 + n.sizeMpc * 0.35, real: true }));
  const SEEDS = 620;
  while (nodes.length < SEEDS) {
    const v = new THREE.Vector3(gauss(rand), gauss(rand), gauss(rand)).normalize();
    const r = 420 * Math.cbrt(rand()) * MPC;
    if (r < 12 * MPC) continue;
    nodes.push({ p: v.multiplyScalar(r), w: 0.35 + 1.6 * Math.pow(rand(), 3), real: false });
  }
  // k-nearest-neighbour graph, deduplicated
  const edges = new Map();
  for (let i = 0; i < nodes.length; i++) {
    const near = [];
    for (let j = 0; j < nodes.length; j++) {
      if (j !== i) near.push([nodes[i].p.distanceToSquared(nodes[j].p), j]);
    }
    near.sort((a, b) => a[0] - b[0]);
    const k = 2 + Math.floor(rand() * 2.4);
    for (let n = 0; n < k; n++) {
      const j = near[n][1], key = i < j ? `${i}:${j}` : `${j}:${i}`;
      const d = Math.sqrt(near[n][0]);
      if (d < 130 * MPC) edges.set(key, [Math.min(i, j), Math.max(i, j), d]);
    }
  }
  const edgeList = [...edges.values()];
  let edgeWeight = 0;
  for (const [a, b, d] of edgeList) edgeWeight += (d / MPC) * (nodes[a].w + nodes[b].w);

  const pos = new Float32Array(galaxies * 3), col = new Float32Array(galaxies * 3), siz = new Float32Array(galaxies);
  const fpos = new Float32Array(fog * 3), fcol = new Float32Array(fog * 3), fsiz = new Float32Array(fog);
  let ng = 0, nf = 0;
  const put = (x, y, z, c, b, s) => {
    if (ng >= galaxies) return;
    pos[3 * ng] = x; pos[3 * ng + 1] = y; pos[3 * ng + 2] = z;
    col[3 * ng] = c[0] * b; col[3 * ng + 1] = c[1] * b; col[3 * ng + 2] = c[2] * b;
    siz[ng++] = s;
  };
  const mix3 = (a, b, t) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
  const c = new THREE.Vector3(), perp = new THREE.Vector3(), chord = new THREE.Vector3();
  const filamentShare = 0.62, clusterShare = 0.32;
  const filamentCount = galaxies * filamentShare;
  for (const [a, b, d] of edgeList) {
    const A = nodes[a].p, B = nodes[b].p;
    const dM = d / MPC, weight = dM * (nodes[a].w + nodes[b].w);
    const n = Math.round(filamentCount * weight / edgeWeight);
    chord.subVectors(B, A).normalize();
    perp.set(gauss(rand), gauss(rand), gauss(rand));
    perp.addScaledVector(chord, -perp.dot(chord)).normalize();
    c.addVectors(A, B).multiplyScalar(0.5).addScaledVector(perp, (0.06 + 0.14 * rand()) * d);
    const width = (1.0 + 1.6 * rand()) * MPC;
    for (let k = 0; k < n; k++) {
      const t = rand(), s = 1 - t;
      const w = width * (0.6 + 0.8 * Math.sin(Math.PI * t));
      put(s * s * A.x + 2 * s * t * c.x + t * t * B.x + gauss(rand) * w,
        s * s * A.y + 2 * s * t * c.y + t * t * B.y + gauss(rand) * w,
        s * s * A.z + 2 * s * t * c.z + t * t * B.z + gauss(rand) * w,
        mix3(WEB_VIOLET, WEB_ICE, rand()), 0.55 + 0.75 * Math.pow(rand(), 2.5), 0.45 + 1.1 * Math.pow(rand(), 3));
    }
    const nFog = Math.round(fog * 0.9 * weight / edgeWeight);
    for (let k = 0; k < nFog && nf < fog; k++) {
      const t = rand(), s = 1 - t;
      fpos[3 * nf] = s * s * A.x + 2 * s * t * c.x + t * t * B.x + gauss(rand) * width * 0.8;
      fpos[3 * nf + 1] = s * s * A.y + 2 * s * t * c.y + t * t * B.y + gauss(rand) * width * 0.8;
      fpos[3 * nf + 2] = s * s * A.z + 2 * s * t * c.z + t * t * B.z + gauss(rand) * width * 0.8;
      const hue = mix3(WEB_VIOLET, WEB_ICE, rand() * 0.5);
      fcol.set(hue, 3 * nf);
      fsiz[nf++] = 0.6 + 1.1 * rand();
    }
  }
  // clusters: warm, dense, sized by node weight
  let wsum = 0;
  for (const n of nodes) wsum += n.w * n.w;
  for (const nd of nodes) {
    const n = Math.round(galaxies * clusterShare * nd.w * nd.w / wsum);
    const sigma = (0.8 + 1.5 * nd.w) * MPC;
    for (let k = 0; k < n; k++) {
      const q = Math.pow(rand(), 1.6);
      put(nd.p.x + gauss(rand) * sigma * q, nd.p.y + gauss(rand) * sigma * q, nd.p.z + gauss(rand) * sigma * q,
        mix3(WEB_WARM, WEB_IVORY, rand() * 0.7), 0.75 + 1.1 * Math.pow(rand(), 2), 0.6 + 1.5 * Math.pow(rand(), 2.5));
    }
  }
  // the rest fill the voids thinly
  while (ng < galaxies) {
    const v = new THREE.Vector3(gauss(rand), gauss(rand), gauss(rand)).normalize().multiplyScalar(440 * Math.cbrt(rand()) * MPC);
    put(v.x, v.y, v.z, mix3(WEB_VIOLET, WEB_IVORY, rand()), 0.2 + 0.25 * rand(), 0.35 + 0.5 * rand());
  }
  // shuffle, so setDrawRange thins evenly on weaker GPUs
  for (let i = galaxies - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    for (let k = 0; k < 3; k++) {
      let t = pos[3 * i + k]; pos[3 * i + k] = pos[3 * j + k]; pos[3 * j + k] = t;
      t = col[3 * i + k]; col[3 * i + k] = col[3 * j + k]; col[3 * j + k] = t;
    }
    const t = siz[i]; siz[i] = siz[j]; siz[j] = t;
  }
  return {
    galaxies: { positions: pos, colors: col, sizes: siz },
    fog: { positions: fpos.subarray(0, nf * 3), colors: fcol.subarray(0, nf * 3), sizes: fsiz.subarray(0, nf) },
  };
}

// Field stars of the dwarf satellites: gaussian swarms at catalog positions.
export function buildDwarfStars(rand, perDwarf = 1400) {
  const dwarfs = LOCAL_GROUP.filter(g => !g.spiral && !/Magellanic/.test(g.name));
  const n = dwarfs.length * perDwarf;
  const positions = new Float32Array(3 * n), colors = new Float32Array(3 * n), sizes = new Float32Array(n);
  let i = 0;
  for (const g of dwarfs) {
    const sigma = g.sizeKpc * 0.28 * KPC;
    for (let k = 0; k < perDwarf; k++, i++) {
      const q = Math.pow(rand(), 1.4);
      positions[3 * i] = g.world.x + gauss(rand) * sigma * q;
      positions[3 * i + 1] = g.world.y + gauss(rand) * sigma * q;
      positions[3 * i + 2] = g.world.z + gauss(rand) * sigma * q;
      const b = 0.5 + 0.7 * Math.pow(rand(), 3);
      colors[3 * i] = 1.0 * b; colors[3 * i + 1] = 0.86 * b; colors[3 * i + 2] = 0.68 * b;
      sizes[i] = 0.4 + 0.9 * rand();
    }
  }
  return { positions, colors, sizes };
}

// Disc normal from sky position, position angle and inclination (literature values
// for these three). The sign of the near side is a convention.
export function discBasis(raDeg, decDeg, paDeg, incDeg) {
  const L = icrsToEclUnit(raDeg, decDeg);
  const N = icrsToEclUnit(raDeg, decDeg + 0.5).sub(L).normalize();
  const E = new THREE.Vector3().crossVectors(N, L).normalize();
  const pa = paDeg * Math.PI / 180, inc = incDeg * Math.PI / 180;
  const major = N.clone().multiplyScalar(Math.cos(pa)).addScaledVector(E, Math.sin(pa));
  const minor = N.clone().multiplyScalar(-Math.sin(pa)).addScaledVector(E, Math.cos(pa));
  const z = L.clone().multiplyScalar(-Math.cos(inc)).addScaledVector(minor, Math.sin(inc)).normalize();
  const x = major.clone().addScaledVector(z, -major.dot(z)).normalize();
  return { x, y: new THREE.Vector3().crossVectors(z, x), z };
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

// -------------------------------------------------------------- far sprites
// Every catalog galaxy and cluster as extended light: constant surface brightness
// while resolved, flux-conserving below a pixel. Positions are catalog values; the
// glow is a stand-in. `vis` lets the volume pass take over close up.
const FAR_V = /* glsl */`
  attribute vec3 col; attribute float rad; attribute float gain; attribute float vis;
  varying vec3 vCol; varying float vA, vRatio, vAng;
  uniform float uPc, uPxScale, uPR;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vCol = col;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    float d = length(mv.xyz / uPc);
    float raw = 2.0 * rad / d * uPxScale;
    float px = clamp(raw, 2.2, 900.0);
    float a = gain * vis * clamp(raw / 2.2, 0.03, 1.0);
    a *= 1.0 - smoothstep(600.0, 900.0, raw);
    vA = a;
    float h = fract(float(gl_VertexID) * 0.61803398875);
    vRatio = 0.45 + 0.55 * fract(h * 7.13);
    vAng = h * 6.2831853;
    if (a < 0.0005) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); gl_PointSize = 0.0; return; }
    gl_PointSize = px * uPR;
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const FAR_F = /* glsl */`
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
    gl_FragColor = vec4(vCol * (exp(-d2 * 7.0) + 0.35 * exp(-d2 * 2.2) * (1.0 - d2)) * vA, 1.0);
  }
`;

export function makeFarSprites(items, pxScaleU, prU) {
  const n = items.length;
  const pos = new Float32Array(3 * n), col = new Float32Array(3 * n), rad = new Float32Array(n), gain = new Float32Array(n), vis = new Float32Array(n).fill(1);
  items.forEach((it, i) => {
    pos.set([it.world.x, it.world.y, it.world.z], 3 * i);
    col.set(it.col, 3 * i);
    rad[i] = it.radius / PC;
    gain[i] = it.gain;
  });
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('col', new THREE.BufferAttribute(col, 3));
  g.setAttribute('rad', new THREE.BufferAttribute(rad, 1));
  g.setAttribute('gain', new THREE.BufferAttribute(gain, 1));
  g.setAttribute('vis', new THREE.BufferAttribute(vis, 1));
  const mat = new THREE.ShaderMaterial({
    vertexShader: FAR_V, fragmentShader: FAR_F,
    uniforms: { uPc: { value: PC }, uPxScale: pxScaleU, uPR: prU },
    blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
  });
  const pts = new THREE.Points(g, mat);
  pts.frustumCulled = false;
  pts.renderOrder = -6;
  pts.userData.vis = g.getAttribute('vis');
  return pts;
}
