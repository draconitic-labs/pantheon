// Pantheon 3D: boot and main loop. The camera never leaves the scene origin; nav.camWorld
// is the f64 world position and every group is re-parked camera-relative each frame
// (floating origin). A log depth buffer spans the 19 orders of magnitude between a
// coastline and the cosmic web.
import * as THREE from 'three';
import * as eph from './eph.js';
import * as data from './data.js';
import { initUI } from './ui.js';
import { Nav, FOV, sst, prettyDist, prettySpeed, niceBar } from './nav.js';
import { Post } from './post.js';
import { TIERS, Governor, guessTier } from './quality.js';
import { TextureBank } from './textures.js';
import { PLACES, FACTS } from './places.js';
import { SolarSystem, bodyMaterial, makeStarObject, updateStarObject } from './scene/system.js';
import { makeStarPoints, pickStar, starUniforms } from './scene/stars.js';
import {
  buildMilkyWay, buildGalaxyDisc, buildFieldStars, mulberry32, galacticBasis, icrsToEclUnit,
  makeCloudPoints, cloudUniforms, GalaxyVolume,
} from './scene/galaxy.js';
import { LOCAL_GROUP, COSMIC_NODES, buildCosmicWeb, buildOort, buildDwarfStars, discBasis } from './scene/cosmos.js';
import { Sky } from './scene/sky.js';

const AU = 1.495978707e11, PC = 3.0856775814913673e16, KPC = 1e3 * PC, MPC = 1e6 * PC, RSUN = 6.957e8;
const DEG = Math.PI / 180;
const params = new URLSearchParams(location.search);
const QA = params.has('qa');
const reduceMotion = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
const ZERO = new THREE.Vector3();
const _fwd = new THREE.Vector3();
const Z = new THREE.Vector3(0, 0, 1);

// ---------- renderer ----------
const canvas = document.getElementById('canvas');
function fatal(msg) {
  const t = document.querySelector('#loader .load-text');
  if (t) { t.textContent = msg; t.style.color = '#e0a070'; }
  throw new Error(msg);
}
let renderer;
try {
  renderer = new THREE.WebGLRenderer({
    canvas, antialias: false, depth: false, stencil: false, alpha: false,
    logarithmicDepthBuffer: true, powerPreference: 'high-performance',
  });
} catch (e) {
  fatal(`This window could not start WebGL2, which Pantheon 3D needs. ${e.message}`);
}
renderer.autoClear = false;
const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(FOV, window.innerWidth / window.innerHeight, 0.5, 1e27);
const post = new Post(renderer, scene, camera);
const textures = new TextureBank(renderer);

// WebKitGTK reports a masked adapter name, so the starting tier can be a guess and
// the governor's measured frame times are what actually decide it.
const glName = (() => {
  try {
    const gl = renderer.getContext();
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    return String(gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
  } catch { return 'unknown'; }
})();
const startTier = guessTier(glName, /swiftshader|software|llvmpipe/i.test(glName));

let quality = TIERS[startTier];
function pixelRatio() { return Math.min(window.devicePixelRatio || 1, 2) * quality.scale; }
let pxScale = 800;
function applySize() {
  const w = window.innerWidth, h = window.innerHeight, pr = pixelRatio();
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
  renderer.setPixelRatio(pr);
  renderer.setSize(w, h, false);
  post.setSize(w * pr, h * pr);
  volume.setSize(w * pr, h * pr);
  starUniforms.uPR.value = pr;
  cloudUniforms.uPR.value = pr;
  pxScale = (h / 2) / Math.tan(FOV * DEG / 2);
  cloudUniforms.uPxScale.value = pxScale;
}
function applyQuality(q) {
  quality = q;
  post.setQuality({ msaa: q.msaa, bloomLevels: q.bloomLevels });
  volume.setQuality(q);
  system.setQuality(q);
  textures.maxSize = q.tex;
  if (fieldPts) fieldPts.geometry.setDrawRange(0, Math.floor(fieldPts.userData.n * q.particles));
  if (webPts) webPts.geometry.setDrawRange(0, Math.floor(webPts.userData.count * q.particles));
  if (webFog) webFog.geometry.setDrawRange(0, Math.floor(webFog.userData.count * q.particles));
  applySize();
}

// ---------- clock (TDB) ----------
const clock = { jd: eph.jdTdb(eph.jdUtcNow()), rate: 60, paused: false };

// ---------- world content ----------
const system = new SolarSystem(scene, textures);
const galBasis = galacticBasis();
const gcWorld = icrsToEclUnit(266.405, -28.93617).multiplyScalar(8.178 * KPC);
const sky = new Sky(scene, galBasis);
const volume = new GalaxyVolume(renderer);
volume.add('Milky Way', gcWorld, galBasis, KPC, 1.0);

const lgByName = (re) => LOCAL_GROUP.find(g => re.test(g.name));
const m31 = lgByName(/Andromeda/), m33 = lgByName(/Triangulum/), lmc = lgByName(/Large Magellanic/), smc = lgByName(/Small Magellanic/);
const m31World = m31.world.clone();
const lgCenter = gcWorld.clone().lerp(m31World, 0.45);
const webCenter = COSMIC_NODES.reduce((s, n) => s.add(n.world), new THREE.Vector3()).multiplyScalar(0.3 / COSMIC_NODES.length);

// Spirals and Magellanic clouds: volume portrait plus a sparkle of sprites, at catalog
// positions with literature orientation. Their structure is illustrative.
const portraits = [
  { g: m31, pa: 38, inc: 77.5, I: 1.0, seed: 31, count: 26000, arms: 2 },
  { g: m33, pa: 23, inc: 55, I: 0.8, seed: 33, count: 12000, arms: 2 },
  { g: lmc, pa: 170, inc: 34, I: 0.55, seed: 50, count: 9000, arms: 1 },
  { g: smc, pa: 45, inc: 60, I: 0.38, seed: 62, count: 5000, arms: 1 },
].map(p => {
  const basis = discBasis(p.g.ra, p.g.dec, p.pa, p.inc);
  const s = (p.g.sizeKpc / 40) * KPC;
  volume.add(p.g.name, p.g.world, basis, s, p.I);
  const disc = buildGalaxyDisc(mulberry32(p.seed), { scale: s / KPC, count: p.count, arms: p.arms });
  const pts = makeCloudPoints(disc, { blobM: 40 * PC * (s / KPC), bright: 0.34, maxPx: 5 });
  const group = new THREE.Group();
  group.quaternion.setFromRotationMatrix(new THREE.Matrix4().makeBasis(basis.x, basis.y, basis.z));
  group.add(pts);
  scene.add(group);
  return { ...p, basis, s, group, pts, world: p.g.world };
});

// Milky Way sparkle, parked at the galactic center
const mwPts = makeCloudPoints(buildMilkyWay(mulberry32(1877)), { blobM: 26 * PC, bright: 0.42, maxPx: 5 });
const mwGroup = new THREE.Group();
mwGroup.add(mwPts);
scene.add(mwGroup);

// Everything else that lives in Sun-centered coordinates
const helioGroup = new THREE.Group();
scene.add(helioGroup);

const oortPts = makeCloudPoints(buildOort(mulberry32(99)), { blobM: 3.2e12, bright: 0.32, maxPx: 2.4 });
helioGroup.add(oortPts);

const dwarfPts = makeCloudPoints(buildDwarfStars(mulberry32(71)), { blobM: 8 * PC, bright: 0.55, maxPx: 3.5 });
helioGroup.add(dwarfPts);

// Far sprites: every Local Group galaxy and catalog cluster as an extended glow, so
// nothing vanishes before the next scale takes over.
const far = (() => {
  const items = [{ name: 'Milky Way', kind: 'galaxy', world: gcWorld, radius: 19 * KPC, col: [1, 0.86, 0.68], I: 0.9, volumeI: 0 }];
  for (const g of LOCAL_GROUP) {
    const vi = portraits.findIndex(p => p.g === g);
    items.push({
      name: g.name, kind: 'galaxy', world: g.world, radius: (g.sizeKpc / 2) * KPC, info: g,
      col: g.spiral ? [1, 0.88, 0.72] : [0.92, 0.84, 0.72], I: g.spiral ? 0.75 : 0.6, volumeI: vi >= 0 ? vi + 1 : -1,
    });
  }
  for (const n of COSMIC_NODES) {
    items.push({ name: n.name, kind: 'node', world: n.world, radius: (n.sizeMpc / 2) * MPC, info: n, col: [1, 0.9, 0.76], I: 0.5, volumeI: -1 });
  }
  const c = items.length;
  const pos = new Float32Array(c * 3), col = new Float32Array(c * 3), siz = new Float32Array(c);
  items.forEach((it, i) => {
    pos.set([it.world.x, it.world.y, it.world.z], 3 * i);
    col.set(it.col.map(v => v * it.I), 3 * i);
    siz[i] = it.radius / PC;
  });
  const pts = makeCloudPoints({ positions: pos, colors: col, sizes: siz }, { blobM: 2 * PC, bright: 1, maxPx: 520, minPx: 1.8 });
  helioGroup.add(pts);
  return { items, pts, col };
})();

// The web is built once; it is several hundred thousand points. Its draw range thins it.
let webPts = null, webFog = null;
const WEB = { g: 1.7, f: 0.06, blobG: 160e3 * PC, blobF: 6 * MPC };
function buildWeb() {
  const w = buildCosmicWeb(mulberry32(4242));
  webPts = makeCloudPoints(w.galaxies, { blobM: WEB.blobG, bright: WEB.g, maxPx: 7, minPx: 1.2 });
  webFog = makeCloudPoints(w.fog, { blobM: WEB.blobF, bright: WEB.f, maxPx: 90, minPx: 3 });
  webPts.geometry.setDrawRange(0, Math.floor(webPts.userData.count * quality.particles));
  webFog.geometry.setDrawRange(0, Math.floor(webFog.userData.count * quality.particles));
  helioGroup.add(webPts, webFog);
}

// Field stars beyond the catalog
let fieldPts = null;
{
  const f = buildFieldStars(mulberry32(808), 180000);
  fieldPts = makeStarPoints(f, { gain: 0, floor: 0.02 });
  fieldPts.userData.n = f.count;
  helioGroup.add(fieldPts);
}

// ---------- catalogs ----------
let starsNear = null;
let exoSystems = [];
function namedPhysical(ci, Mv) {
  // temperature from color index (Ballesteros 2012); radius from luminosity and T
  const T = 4600 * (1 / (0.92 * ci + 1.7) + 1 / (0.92 * ci + 0.62));
  const L = Math.pow(10, -0.4 * (Mv - 4.83));
  return { T, radiusM: Math.max(Math.sqrt(L) * Math.pow(5778 / T, 2), 0.08) * RSUN };
}
// color of a star by temperature, red dwarf to blue-white: a rough blackbody ramp
function tintForT(T) {
  const stops = [[2800, [1, 0.55, 0.3]], [4200, [1, 0.74, 0.5]], [5800, [1, 0.9, 0.72]], [7500, [1, 0.97, 0.94]], [10000, [0.76, 0.85, 1]], [25000, [0.62, 0.74, 1]]];
  if (T <= stops[0][0]) return stops[0][1];
  for (let i = 1; i < stops.length; i++) {
    if (T <= stops[i][0]) {
      const f = (T - stops[i - 1][0]) / (stops[i][0] - stops[i - 1][0]);
      return stops[i][1].map((v, k) => stops[i - 1][1][k] + (v - stops[i - 1][1][k]) * f);
    }
  }
  return stops[stops.length - 1][1];
}
let namedSet = null;
const named = data.NAMED_STARS.map(({ name, ra, dec, distPc, vmag, ci }) => {
  const world = data.icrsToEcl(ra, dec).multiplyScalar(distPc * PC);
  const Mv = vmag - 5 * Math.log10(Math.max(distPc, 0.1) / 10);
  const phys = namedPhysical(ci ?? 0.6, Mv);
  return { name, world, distPc, vmag, ci: ci ?? 0.6, radiusM: phys.radiusM, T: phys.T, tint: tintForT(phys.T) };
});
{
  const n = named.length;
  const pos = new Float32Array(3 * n), col = new Float32Array(3 * n);
  named.forEach((s, i) => {
    pos.set([s.world.x, s.world.y, s.world.z], 3 * i);
    col.set(data.starRgb(s.ci), 3 * i);
  });
  namedSet = { count: n, positions: pos, colors: col, meta: named.map(s => ({ gmag: s.vmag, distPc: s.distPc })) };
}
const starMats = [];
function addStars(set) {
  const pts = makeStarPoints(set);
  starMats.push(pts.material);
  helioGroup.add(pts);
}
addStars(namedSet);
// Distance rings around the Sun in the galactic plane: an engraved scale for the star
// fields, where nothing else tells you how far anything is.
const rings = (() => {
  const group = new THREE.Group();
  const list = [5, 10, 25, 50, 100, 250, 500, 1000, 2500].map((ly) => {
    const R = ly * 9.4607304725808e15, N = 180, pts = new Float32Array((N + 1) * 3);
    for (let i = 0; i <= N; i++) {
      const a = 2 * Math.PI * i / N, c = Math.cos(a) * R, s = Math.sin(a) * R;
      pts[3 * i] = galBasis.x.x * c + galBasis.y.x * s;
      pts[3 * i + 1] = galBasis.x.y * c + galBasis.y.y * s;
      pts[3 * i + 2] = galBasis.x.z * c + galBasis.y.z * s;
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(pts, 3));
    const m = new THREE.LineBasicMaterial({ color: 0xd8b36a, transparent: true, opacity: 0, blending: THREE.AdditiveBlending, depthWrite: false });
    const line = new THREE.Line(g, m);
    line.frustumCulled = false;
    line.renderOrder = -4;
    group.add(line);
    return { ly, R, m, line };
  });
  scene.add(group);
  return {
    list,
    // a ring shows while it is comparable to how far out you are
    update(dPc, cam) {
      group.position.set(-cam.x, -cam.y, -cam.z);
      // edge-on they are only a bundle of lines through the middle of the view
      const open = sst(Math.abs(_fwd.set(0, 0, -1).applyQuaternion(nav.quat).dot(galBasis.z)), 0.08, 0.32);
      for (const r of list) {
        const ratio = Math.log10(Math.max(dPc * 30856775814913673 / r.R, 1e-9));
        const o = 0.2 * open * sst(ratio, -0.9, -0.35) * (1 - sst(ratio, 0.35, 0.9));
        r.m.opacity = o;
        r.line.visible = o > 0.004;
      }
    },
  };
})();
const catalogs = {
  near: data.loadStars().then(s => {
    starsNear = s;
    addStars(s);
    data.nameStars(s);
    resolveExoDistances();
  }).catch(e => { console.warn('gaia nearby', e); ui.toast('The nearby-star catalog did not load'); }),
  bright: data.loadBrightStars().then(s => addStars(s))
    .catch(e => console.warn('gaia bright', e)),
  exo: data.loadExo().then(x => { exoSystems = x; resolveExoDistances(); })
    .catch(e => console.warn('exo', e)),
};
// systems whose archive row lacks sy_dist take it from Gaia by angular match
function resolveExoDistances() {
  if (!starsNear || !exoSystems.length) return;
  const tol = Math.cos(0.05 * DEG);
  for (const sys of exoSystems) {
    if (sys.ra == null || (sys.distPc != null && sys.distPc > 60)) continue;
    const a = sys.ra * DEG, d = sys.dec * DEG, cd = Math.cos(d);
    const ux = cd * Math.cos(a), uy = cd * Math.sin(a), uz = Math.sin(d);
    let best = tol, bi = -1;
    for (let i = 0; i < starsNear.count; i++) {
      const m = starsNear.meta[i];
      const a2 = m.ra * DEG, d2 = m.dec * DEG, cd2 = Math.cos(d2);
      const dot = ux * cd2 * Math.cos(a2) + uy * cd2 * Math.sin(a2) + uz * Math.sin(d2);
      if (dot > best) { best = dot; bi = i; }
    }
    if (bi >= 0) {
      sys.distPc = sys.distPc ?? starsNear.meta[bi].distPc;
      sys.gaiaIdx = bi;
      const p = starsNear.positions;
      sys.world = new THREE.Vector3(p[3 * bi], p[3 * bi + 1], p[3 * bi + 2]);
    }
  }
}
async function loadLandRings(url) {
  const gj = await (await fetch(url)).json();
  const rings = [];
  for (const f of gj.features || []) {
    const g = f && f.geometry;
    if (!g) continue;
    if (g.type === 'Polygon') rings.push(...g.coordinates);
    else if (g.type === 'MultiPolygon') for (const poly of g.coordinates) rings.push(...poly);
  }
  return rings;
}
data.loadTracks().then(t => system.setTracks(t)).catch(() => {});

// ---------- states API (SPICE truth from the Rust core) ----------
let fetching = false, lastFetch = 0;
function maybeFetchStates(now) {
  const c = system.stateCache;
  if (fetching || now - lastFetch < 120) return;
  if (c && Math.abs(clock.jd - c.jd) < 0.2) return;
  fetching = true;
  lastFetch = now;
  fetch(`/api/states?jd=${clock.jd}`).then(r => r.json()).then(j => system.setStates(j))
    .catch(() => {}).finally(() => { fetching = false; });
}

// ---------- star pocket: any star up close, with its known planets ----------
const pocket = {
  group: new THREE.Group(), active: null, star: null, planets: [], lines: [], dots: null, dotVis: null,
};
scene.add(pocket.group);
pocket.star = makeStarObject(system.sphereHi, { tint: [1, 1, 1], intensity: 2.6, glare: true });
pocket.star.group.visible = false;
scene.add(pocket.star.group);
const EXO_PALETTE = [
  { max: 1.6, hot: { kind: 1, a: '#b0603a', b: '#552a1a', p: [0, 1.6, 0, 0], light: 1 }, cold: { kind: 1, a: '#a89888', b: '#5e5650', p: [0, 1.5, 0, 0], light: 1 } },
  { max: 4.5, hot: { kind: 3, a: '#c0a070', b: '#907050', p: [0, 1, 6, 0], light: 2 }, cold: { kind: 3, a: '#7fa8d0', b: '#5f88b8', p: [0, 1, 6, 0], light: 2 } },
  { max: 1e9, hot: { kind: 2, a: '#d0a070', b: '#905838', p: [0, 1, 8, 0], light: 2 }, cold: { kind: 2, a: '#d8c8a0', b: '#a89878', p: [0, 1, 7, 0], light: 2 } },
];
function exoStyle(pl) {
  const r = pl.radE || 1;
  const row = EXO_PALETTE.find(e => r <= e.max);
  return (pl.eqt || 300) > 700 ? row.hot : row.cold;
}
function activatePocket(entry) {   // {name, world, radiusM, tint, exo}
  clearPocket();
  pocket.active = entry;
  pocket.star.mat.uniforms.uTint.value.set(...entry.tint);
  pocket.star.group.visible = true;
  if (!entry.exo) return;
  const list = entry.exo.planets.filter(p => p.aAU);
  list.forEach((pl, i) => {
    const N = 192, pts = new Float32Array((N + 1) * 3);
    for (let k = 0; k <= N; k++) {
      const th = 2 * Math.PI * k / N;
      pts[3 * k] = Math.cos(th) * pl.aAU * AU; pts[3 * k + 1] = Math.sin(th) * pl.aAU * AU;
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(pts, 3));
    const line = new THREE.Line(g, new THREE.LineBasicMaterial({ color: 0xd8b36a, transparent: true, opacity: 0.32, blending: THREE.AdditiveBlending, depthWrite: false }));
    line.frustumCulled = false;
    pocket.group.add(line);
    pocket.lines.push(line);
    const radius = Math.max((pl.radE || 1) * 6.371e6, 1e6);
    const mesh = new THREE.Mesh(system.sphereLo, bodyMaterial(exoStyle(pl), 70 + i * 3.1));
    mesh.scale.setScalar(radius);
    pocket.group.add(mesh);
    pocket.planets.push({ mesh, pl, radius, world: new THREE.Vector3() });
  });
  const n = pocket.planets.length;
  if (n) {
    const pos = new Float32Array(3 * n), col = new Float32Array(3 * n);
    pocket.planets.forEach((p, i) => col.set(p.pl.radE > 4.5 ? [1, 0.9, 0.7] : [0.7, 0.85, 1], 3 * i));
    pocket.dots = makeStarPoints({ count: n, positions: pos, colors: col, lum: new Float32Array(n) }, { perVertexVis: true, maxPx: 4, floor: 0.002 });
    pocket.dotVis = new Float32Array(n);
    pocket.dots.geometry.setAttribute('vis', new THREE.BufferAttribute(pocket.dotVis, 1));
    pocket.group.add(pocket.dots);
  }
}
function clearPocket() {
  for (const o of [...pocket.lines, ...pocket.planets.map(p => p.mesh), pocket.dots]) {
    if (!o) continue;
    pocket.group.remove(o);
    if (o !== null && o.geometry && o.geometry !== system.sphereLo) o.geometry.dispose();
    o.material.dispose();
  }
  pocket.active = null; pocket.planets = []; pocket.lines = []; pocket.dots = null;
  pocket.star.group.visible = false;
}
function starEntryFromGaia(idx) {
  const m = starsNear.meta[idx];
  const world = new THREE.Vector3(starsNear.positions[3 * idx], starsNear.positions[3 * idx + 1], starsNear.positions[3 * idx + 2]);
  const name = (starsNear.names && starsNear.names.get(idx)) || `Gaia DR3 ${m.id}`;
  const Mg = m.gmag - 5 * Math.log10(Math.max(m.distPc, 0.1) / 10);
  const phys = namedPhysical(Math.max(0, Math.min(3.2, m.bprp ?? 0.8)) * 0.9, Mg);
  let exo = exoSystems.find(s => s.gaiaIdx === idx) || null;
  if (!exo) {
    for (const s of exoSystems) {
      if (s.world && s.world.distanceTo(world) < 0.1 * PC) { exo = s; break; }
    }
  }
  return { id: `gaia:${idx}`, name: exo ? exo.host : name, world, radiusM: (exo && exo.stRad ? exo.stRad * RSUN : phys.radiusM), tint: tintForT(phys.T), exo, gaiaIdx: idx };
}
function starEntryFromNamed(s) {
  return { id: `star:${s.name}`, name: s.name, world: s.world, radiusM: s.radiusM, tint: s.tint, named: s };
}
function starEntryFromExo(sys) {
  const world = sys.world || data.icrsToEcl(sys.ra, sys.dec).multiplyScalar(sys.distPc * PC);
  const T = sys.teff || 5200;
  return { id: `exo:${sys.host}`, name: sys.host, world, radiusM: (sys.stRad || 1) * RSUN, tint: tintForT(T), exo: sys, gaiaIdx: sys.gaiaIdx };
}
let pocketItems = [];
function pocketTick(jd, camWorld, tSec) {
  // what should be up close? the star you are focused on, else the nearest catalog star
  let want = null;
  const f = nav.focus;
  if (f && f.starEntry && camWorld.distanceTo(f.starEntry.world) < 3 * PC) want = f.starEntry;
  else {
    const sunDist = camWorld.length();
    if (sunDist > 0.05 * PC) {
      for (const s of named) {
        if (s.world.distanceTo(camWorld) < 0.6 * PC) { want = starEntryFromNamed(s); break; }
      }
      if (!want && starsNear) {
        const p = starsNear.positions;
        let best = 0.6 * PC, bi = -1;
        for (let i = 0; i < starsNear.count; i++) {
          const dx = p[3 * i] - camWorld.x, dy = p[3 * i + 1] - camWorld.y, dz = p[3 * i + 2] - camWorld.z;
          const dd = Math.abs(dx) + Math.abs(dy) + Math.abs(dz);
          if (dd > best * 1.8) continue;
          const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
          if (d < best) { best = d; bi = i; }
        }
        if (bi >= 0) want = pocket.active && pocket.active.gaiaIdx === bi ? pocket.active : starEntryFromGaia(bi);
      }
    }
  }
  if (want && (!pocket.active || pocket.active.id !== want.id)) activatePocket(want);
  else if (!want && pocket.active) clearPocket();
  if (!pocket.active) { pocketItems = []; return; }
  const e = pocket.active;
  pocket.group.position.set(e.world.x - camWorld.x, e.world.y - camWorld.y, e.world.z - camWorld.z);
  updateStarObject(pocket.star, e.world, e.radiusM, camWorld, nav.quat, tSec, pxScale, 1);
  const out = [{ name: e.name, kind: 'star', world: e.world, radius: e.radiusM, entry: e }];
  pocket.planets.forEach((p, i) => {
    const ph = 2 * Math.PI * ((((jd / (p.pl.perD || 365)) % 1) + 1) % 1) + (p.pl.name.length * 1.7);
    const rx = Math.cos(ph) * p.pl.aAU * AU, ry = Math.sin(ph) * p.pl.aAU * AU;
    p.world.set(e.world.x + rx, e.world.y + ry, e.world.z);
    p.mesh.position.set(rx, ry, 0);
    const d = p.world.distanceTo(camWorld);
    const discPx = p.radius / Math.max(d, 1) * pxScale;
    p.mesh.visible = discPx > 0.4;
    const u = p.mesh.material.uniforms;
    u.uSunObj.value.set(-rx, -ry, 0).normalize();
    u.uCamObj.value.copy(camWorld).sub(p.world).divideScalar(p.radius);
    u.uSunI.value = 2.4;
    if (pocket.dots) {
      pocket.dots.geometry.getAttribute('position').setXYZ(i, rx, ry, 0);
      const dp = d / PC;
      pocket.dots.geometry.getAttribute('lum').setX(i, 0.22 * dp * dp * 0.01);
      pocket.dotVis[i] = 1 - sst(discPx, 1.2, 4);
    }
    out.push({ name: p.pl.name, kind: 'exoplanet', world: p.world, radius: p.radius, pl: p.pl, host: e.name, entry: e });
  });
  if (pocket.dots) {
    const g = pocket.dots.geometry;
    g.getAttribute('position').needsUpdate = true;
    g.getAttribute('lum').needsUpdate = true;
    g.getAttribute('vis').needsUpdate = true;
  }
  pocketItems = out;
}

// ---------- targets: everything you can fly to ----------
const ANCHOR = { sun: { pos: () => ZERO }, gc: { pos: () => gcWorld }, lg: { pos: () => lgCenter }, web: { pos: () => webCenter } };
const STAR_CHAIN = [ANCHOR.gc, ANCHOR.lg, ANCHOR.web];
const BODY_CHAIN = [ANCHOR.sun, ...STAR_CHAIN];

// Arrive on the sunlit side with a slice of night showing, a little above the plane.
function litDir(world, u0, { phase = 52, elev = 14, pole = Z } = {}) {
  const s = world.clone().negate().normalize();
  const N = pole.clone();
  if (s.dot(N) < 0 && pole !== Z) N.negate();
  const side = new THREE.Vector3().crossVectors(N, s);
  if (side.lengthSq() < 1e-8) side.set(1, 0, 0);
  side.normalize();
  if (u0 && side.dot(u0) < 0) side.negate();
  return s.multiplyScalar(Math.cos(phase * DEG)).addScaledVector(side, Math.sin(phase * DEG)).addScaledVector(N, Math.tan(elev * DEG)).normalize();
}
// keep the azimuth you came from; set the elevation over a plane
function elevDir(u0, normal, elevDeg) {
  const flat = u0.clone().addScaledVector(normal, -u0.dot(normal));
  if (flat.lengthSq() < 1e-6) flat.set(1, 0, 0).addScaledVector(normal, -normal.x);
  flat.normalize();
  return flat.multiplyScalar(Math.cos(elevDeg * DEG)).addScaledVector(normal, Math.sin(elevDeg * DEG)).normalize();
}
const targets = new Map();
function makeTarget(id) {
  if (targets.has(id)) return targets.get(id);
  const [type, ...rest] = id.split(':');
  const name = rest.join(':');
  let t = null;
  if (type === 'body') {
    const b = system.body(name);
    if (!b) return null;
    if (b.kind === 'craft') {
      t = { id, name, kind: 'craft', pos: () => system.body(name).world, radius: 0, frame: 2e9, minAlt: 1e7, chain: BODY_CHAIN };
    } else {
      const chain = b.kind === 'moon' ? [{ pos: () => system.body(b.parent).world }, ...BODY_CHAIN] : b.kind === 'star' ? STAR_CHAIN : BODY_CHAIN;
      t = {
        id, name, kind: b.kind, pos: () => system.body(name).world, radius: b.radius, frame: b.frame || b.radius, chain,
        fact: FACTS[name], onApproach: () => system.wantTextures(name, true),
        arrivalDir: (u0) => {
          if (name === 'Sun') return elevDir(u0, Z, 18);
          const opts = name === 'Saturn'
            ? { phase: 40, elev: 24, pole: eph.bodyBasis(b.idx, clock.jd).z }
            : {};
          return litDir(system.body(name).world, u0, opts);
        },
      };
    }
  } else if (type === 'star' || type === 'exo' || type === 'gaia') {
    let entry = null;
    if (type === 'star') { const s = named.find(n => n.name === name); if (s) entry = starEntryFromNamed(s); }
    else if (type === 'exo') { const s = exoSystems.find(x => x.host === name); if (s && (s.distPc != null || s.world)) entry = starEntryFromExo(s); }
    else if (starsNear) entry = starEntryFromGaia(+name);
    if (!entry) return null;
    const aMax = entry.exo ? entry.exo.planets.reduce((m, p) => Math.max(m, p.aAU || 0), 0) : 0;
    t = {
      id, name: entry.name, kind: 'star', pos: () => entry.world, radius: entry.radiusM,
      frame: Math.max(entry.radiusM, aMax * AU * 1.15), minAlt: entry.radiusM * 0.05, chain: STAR_CHAIN, starEntry: entry, fact: FACTS[entry.name],
      arrivalDir: (u0) => (aMax ? elevDir(u0, Z, 48) : u0.clone()),
    };
  } else if (type === 'gal') {
    const it = far.items.find(x => x.name === name);
    if (!it) return null;
    const p = portraits.find(q => q.g.name === name);
    const frame = it.radius;
    t = {
      id, name, kind: 'galaxy', pos: () => it.world, radius: 0, frame, minAlt: frame * 0.03, fill: 0.72,
      chain: [ANCHOR.lg, ANCHOR.web], fact: FACTS[name], info: it.info, far: it,
      arrivalDir: (u0) => (p ? elevDir(u0, p.basis.z, 42) : u0.clone()),
    };
  } else if (type === 'node') {
    const it = far.items.find(x => x.name === name);
    if (!it) return null;
    t = {
      id, name, kind: 'node', pos: () => it.world, radius: 0, frame: it.radius * 2.2, minAlt: it.radius * 0.02, fill: 0.8,
      chain: [ANCHOR.web], fact: FACTS[name], far: it,
    };
  } else if (type === 'v') {
    const V = {
      solar: { name: 'Solar system', pos: () => ZERO, frame: 34 * AU, fill: 0.86, minAlt: 1e10, chain: STAR_CHAIN, arrivalDir: (u0) => elevDir(u0, Z, 34) },
      nearby: { name: 'Nearby stars', pos: () => ZERO, frame: 6 * PC, fill: 0.9, minAlt: 1e13, chain: STAR_CHAIN },
      milkyway: { name: 'Milky Way', pos: () => gcWorld, frame: 16 * KPC, fill: 0.86, minAlt: 300 * PC, chain: [ANCHOR.lg, ANCHOR.web], arrivalDir: (u0) => elevDir(u0, galBasis.z, 52) },
      gc: { name: 'Galactic center', pos: () => gcWorld, frame: 4 * KPC, fill: 0.8, minAlt: 30 * PC, chain: [ANCHOR.lg, ANCHOR.web], arrivalDir: (u0) => elevDir(u0, galBasis.z, 30) },
      localgroup: { name: 'Local Group', pos: () => lgCenter, frame: 620 * KPC, fill: 0.86, minAlt: 20 * KPC, chain: [ANCHOR.web] },
      web: { name: 'Cosmic web', pos: () => webCenter, frame: 230 * MPC, fill: 0.86, minAlt: 2 * MPC, chain: [] },
    }[name];
    if (!V) return null;
    t = { id, kind: 'place', radius: 0, fact: FACTS[V.name], ...V };
  }
  if (t) targets.set(id, t);
  return t;
}

// ---------- selection and cards ----------
let selection = null;   // target whose card is open
function cardFor(t) {
  const cam = nav.camWorld;
  const flyable = (nav.flight ? nav.flight.t.id : nav.focus && nav.focus.id) !== t.id;
  const fly = flyable ? () => goTo(t) : null;
  const liveRange = { label: 'Distance', value: prettyDist(Math.max(t.pos().distanceTo(cam) - (t.radius || 0), 0)), live: 'range' };
  if (t.kind === 'planet' || t.kind === 'moon' || (t.kind === 'star' && t.id === 'body:Sun') || t.kind === 'craft') {
    const b = system.body(t.name);
    const c = system.bodyCard(b, cam, clock.jd);
    return { ...c, blurb: t.fact, fly };
  }
  if (t.kind === 'star') {
    const e = t.starEntry;
    const rows = [];
    if (e.named) {
      rows.push({ label: 'From the Sun', value: `${(e.named.distPc * 3.26156).toFixed(1)} light-years`, ev: 'gaia-compiled' });
      rows.push({ label: 'Brightness', value: `V ${e.named.vmag.toFixed(2)}`, ev: 'gaia-compiled' });
    } else if (e.gaiaIdx != null && starsNear) {
      const m = starsNear.meta[e.gaiaIdx];
      rows.push({ label: 'From the Sun', value: `${(m.distPc * 3.26156).toFixed(1)} light-years`, ev: 'gaia-dr3' });
      rows.push({ label: 'Brightness', value: `G ${m.gmag.toFixed(2)}`, ev: 'gaia-dr3' });
      rows.push({ label: 'BP−RP color', value: (m.bprp ?? 0).toFixed(2), ev: 'gaia-dr3' });
    } else if (e.exo && e.exo.distPc != null) {
      rows.push({ label: 'From the Sun', value: `${(e.exo.distPc * 3.26156).toFixed(1)} light-years`, ev: 'exo-archive' });
    }
    if (e.exo && e.exo.teff) rows.push({ label: 'Temperature', value: `${Math.round(e.exo.teff).toLocaleString()} K`, ev: 'exo-archive' });
    rows.push({ label: 'Radius, estimated', value: `${(e.radiusM / RSUN).toFixed(2)} × the Sun`, ev: e.exo && e.exo.stRad ? 'exo-archive' : 'star-surface' });
    if (e.exo) {
      rows.push({ label: 'Known planets', value: `${e.exo.planets.length}`, ev: 'exo-archive' });
      for (const p of e.exo.planets.slice(0, 7)) {
        rows.push({ label: p.name.replace(e.exo.host, '').trim() || p.name, value: `${p.aAU ? `${p.aAU.toFixed(3)} AU` : 'unknown'}, ${p.radE ? `${p.radE.toFixed(2)} Earths` : 'size unknown'}`, ev: p.aAU ? 'exo-archive' : 'exo-unknown-value' });
      }
    }
    rows.push(liveRange);
    return {
      title: t.name, kind: e.exo ? 'Star with known planets' : 'Star', blurb: t.fact, rows, fly,
      note: e.exo ? 'Orbit shapes, tilt and where each planet is along its orbit are illustrative; periods and sizes are catalog values. Planet colors are a guess from size and temperature.' : 'Its surface as drawn is illustrative.',
    };
  }
  if (t.kind === 'galaxy') {
    const i = t.info;
    const rows = [{ label: 'From the Sun', value: prettyDist(t.pos().length()), ev: 'compiled-catalog' }];
    if (i) rows.push({ label: 'Width', value: prettyDist(i.sizeKpc * KPC), ev: 'compiled-catalog' });
    rows.push({ label: 'Portrait', value: 'Illustrative', ev: 'galaxy-portrait' });
    rows.push(liveRange);
    return { title: t.name, kind: i && i.spiral ? 'Spiral galaxy' : 'Galaxy', blurb: t.fact, rows, fly };
  }
  if (t.kind === 'node') {
    return {
      title: t.name, kind: 'Galaxy cluster', blurb: t.fact, fly,
      rows: [
        { label: 'From the Sun', value: prettyDist(t.pos().length()), ev: 'compiled-catalog' },
        { label: 'Filaments', value: 'Synthetic', ev: 'web-procedural' },
        liveRange,
      ],
    };
  }
  const rows = [];
  if (t.name === 'Milky Way' || t.name === 'Galactic center') {
    rows.push({ label: 'Center from the Sun', value: '26,700 light-years', ev: 'compiled-catalog' });
    rows.push({ label: 'Portrait', value: 'Illustrative model', ev: 'galaxy-fog' });
  } else if (t.name === 'Cosmic web') {
    rows.push({ label: 'Named clusters', value: 'Catalog positions', ev: 'compiled-catalog' });
    rows.push({ label: 'Filaments', value: 'Synthetic', ev: 'web-procedural' });
  } else if (t.name === 'Nearby stars') {
    rows.push({ label: 'Stars', value: 'Gaia DR3', ev: 'gaia-dr3' });
  } else if (t.name === 'Solar system') {
    rows.push({ label: 'Positions', value: system.stateSrc(clock.jd), ev: system.usingKernels(clock.jd) ? 'de440' : 'approx-elements' });
  } else if (t.name === 'Local Group') {
    rows.push({ label: 'Positions', value: 'Catalog', ev: 'compiled-catalog' });
    rows.push({ label: 'Portraits', value: 'Illustrative', ev: 'galaxy-portrait' });
  }
  return { title: t.name, kind: 'Place', blurb: t.fact, rows, fly };
}
function select(t) {
  selection = t;
  ui.showCard(cardFor(t));
}

// ---------- journeys ----------
const backStack = [];
function pushBack() {
  if (!nav.focus) return;
  backStack.push({ target: nav.focus, dist: nav.camWorld.distanceTo(nav.pivot), dir: nav.dir.clone() });
  if (backStack.length > 40) backStack.shift();
}
function goTo(t, { speed = 1, keepTour = false } = {}) {
  if (!t) return;
  if (!keepTour) stopTour();
  pushBack();
  if (t.onApproach) t.onApproach();
  nav.flyTo(t, { speed, aspect: camera.aspect });
  ui.showFact('');
}
function goHome() { goTo(makeTarget('body:Earth')); }
function goBack() {
  const b = backStack.pop();
  if (!b) { ui.toast('Nothing to go back to yet'); return; }
  stopTour();
  const t = { ...b.target, arrive: b.dist, arrivalDir: () => b.dir.clone() };
  nav.flyTo(t, { aspect: camera.aspect });
}
function stopAll() {
  if (nav.flight) nav.cancelFlight();
  stopTour('Tour ended');
}

// ---------- free flight ----------
function nearestAnchorTarget(cam) {
  const near = system.nearestSurface(cam);
  if (near.body && near.dist < near.body.radius * 400) return makeTarget(`body:${near.body.name}`);
  if (pocket.active) { const t = makeTarget(pocket.active.id); if (t) return t; }
  const dSun = cam.length();
  if (dSun < 400 * AU) return makeTarget('v:solar');
  if (dSun < 1000 * PC) return makeTarget('v:nearby');
  if (dSun < 120 * KPC) return makeTarget('v:milkyway');
  if (dSun < 3 * MPC) return makeTarget('v:localgroup');
  return makeTarget('v:web');
}
function setFree(on, { quiet = false } = {}) {
  if (on === (nav.mode === 'free')) return;
  stopTour();
  if (on) {
    if (nav.flight) nav.cancelFlight();
    nav.setMode('free');
    nav.speedMult = 1;
    if (!quiet) ui.toast('Free flight: W A S D to fly, drag to look. Press F to leave.', 4200);
  } else {
    if (nav.locked) document.exitPointerLock?.();
    nav.setMode('orbit');
    nav.retarget(nearestAnchorTarget(nav.camWorld));
    if (!quiet) ui.toast(`Back to orbiting ${nav.focus.name}`);
  }
}

// ---------- UI + navigation wiring ----------
const ui = initUI({
  places: PLACES,
  goPlace: (id) => { const t = makeTarget(id); if (t) { goTo(t); select(t); } else ui.toast('That place is not loaded yet'); },
  goBack, goHome: () => { goHome(); },
  recenter: () => nav.recenter(),
  stop: stopAll,
  search: runSearch,
  pick(res) { const t = makeTarget(res.id); if (t) { goTo(t); select(t); } },
  setRate(r) { clock.rate = r; clock.paused = false; ui.setRateActive(r, false); },
  togglePause,
  goNow() { clock.jd = eph.jdTdb(eph.jdUtcNow()); },
  tour: () => (tour ? stopTour('Tour ended') : startTour()),
  tourSkip,
  freeFlight: () => setFree(nav.mode !== 'free'),
  audio: () => toggleAmbience(),
  setQuality(id) { gov.set(id); ui.setQuality(gov.mode, TIERS[gov.tier].label); saveQuality(id); },
  cardClosed() { selection = null; },
});
function togglePause() {
  clock.paused = !clock.paused;
  ui.setRateActive(clock.rate, clock.paused);
}

const nav = new Nav(canvas, {
  blocked: () => ui.isTyping() || ui.helpOpen(),
  onClick(nx, ny) {
    const hit = pickAll([nx, ny]);
    if (!hit) { if (selection) ui.hideCard(); return; }
    const t = hit.target();
    if (!t) return;
    if (nav.focus && nav.focus.id === t.id && !nav.flight) { select(t); return; }
    goTo(t);
    select(t);
  },
  onDouble() { nav.recenter(); },
  onInput() {
    ui.dismissHint();
    if (tour) stopTour('Tour ended');
  },
  surface: (cam) => ({ dist: surfaceDist(cam) }),
  upAt(d, out) {
    const w = sst(Math.log10(Math.max(d, 1)), 17, 19);
    out.copy(Z).lerp(galBasis.z, w).normalize();
  },
  onFlyKey() { setFree(true); },
  onLock(on) {
    ui.setBanner(on ? 'Mouse-look is on. Move the mouse to turn. Esc releases it.' : '');
  },
  onLockFail() { ui.toast('This window refused mouse-look. Drag to look instead.', 4000); },
  onSpeed(m) { ui.toast(`Flight speed × ${m < 1 ? m.toFixed(2) : m.toFixed(1)}`, 1200); },
  onArrive(t) {
    if (!t.arrive) ui.showFact(t.fact || '');
    if (tour) { tour.phase = 'dwell'; tour.t0 = tSec; nav.autoSpin = reduceMotion() ? 0 : 0.035; ui.showFact(t.fact || ''); }
    if (selection && selection.id === t.id) ui.showCard(cardFor(selection));
  },
  reducedMotion: reduceMotion,
});
function surfaceDist(cam) {
  let s = system.nearestSurface(cam).dist;
  if (pocket.active) s = Math.min(s, Math.max(pocket.active.world.distanceTo(cam) - pocket.active.radiusM, 1));
  for (const st of named) s = Math.min(s, st.world.distanceTo(cam));
  s = Math.min(s, Math.max(cam.distanceTo(gcWorld) * 0.5, 40 * PC));
  for (const p of portraits) s = Math.min(s, Math.max(cam.distanceTo(p.world) - p.s * 6, p.s));
  return s;
}

// keys that belong to the app, with one consistent Esc ladder
window.addEventListener('keydown', (e) => {
  if (e.ctrlKey || e.metaKey || e.altKey || ui.isTyping()) return;
  if (ui.helpOpen()) { if (e.code === 'Escape') ui.setHelp(false); return; }
  switch (e.code) {
    case 'Escape':
      if (ui.closePanels()) break;
      if (tour) { stopTour('Tour ended'); if (nav.flight) nav.cancelFlight(); }
      else if (nav.flight) nav.cancelFlight();
      else if (nav.locked) document.exitPointerLock?.();
      else if (nav.mode === 'free') setFree(false);
      else if (selection) ui.hideCard();
      break;
    case 'KeyF': setFree(nav.mode !== 'free'); break;
    case 'KeyL':
      if (nav.locked) document.exitPointerLock?.();
      else { setFree(true, { quiet: true }); nav.lockMouse(); }
      break;
    case 'KeyH': goHome(); break;
    case 'KeyB': case 'Backspace': e.preventDefault(); goBack(); break;
    case 'KeyP': if (tour) stopTour('Tour ended'); else startTour(); break;
    case 'KeyM': toggleAmbience(); break;
    case 'Space':
      if (document.activeElement && /BUTTON|INPUT/.test(document.activeElement.tagName)) break;   // it presses that control
      e.preventDefault();
      togglePause();
      break;
    case 'BracketLeft': { const r = ui.stepRate(clock.rate, -1); clock.rate = r; ui.setRateActive(r, clock.paused); break; }
    case 'BracketRight': { const r = ui.stepRate(clock.rate, 1); clock.rate = r; ui.setRateActive(r, clock.paused); break; }
    default: break;
  }
});

// ---------- search ----------
const fold = (s) => s.toLowerCase().replace(/α/g, 'alpha ').replace(/ε/g, 'epsilon ').replace(/[^a-z0-9*+ -]/g, ' ').replace(/\s+/g, ' ').trim();
const SEARCH_ALIASES = [
  ['sagittarius a', 'v:gc', 'Galactic center'], ['sgr a', 'v:gc', 'Galactic center'], ['galactic center', 'v:gc', 'Galactic center'],
  ['galaxy', 'v:milkyway', 'Milky Way'], ['lmc', 'gal:Large Magellanic Cloud', 'Large Magellanic Cloud'], ['smc', 'gal:Small Magellanic Cloud', 'Small Magellanic Cloud'],
  ['universe', 'v:web', 'Cosmic web'], ['home', 'body:Earth', 'Earth'], ['neptune moon', 'body:Triton', 'Triton'], ['triton', 'body:Triton', 'Triton'],
];
// edit distance counting a swap of neighbours as one slip ("satrun" -> saturn)
function editDistance(a, b, max) {
  if (Math.abs(a.length - b.length) > max) return max + 1;
  let prev2 = null;
  let prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 1; i <= a.length; i++) {
    const cur = [i];
    let best = i;
    for (let j = 1; j <= b.length; j++) {
      cur[j] = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
      if (prev2 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) cur[j] = Math.min(cur[j], prev2[j - 2] + 1);
      best = Math.min(best, cur[j]);
    }
    if (best > max) return max + 1;
    prev2 = prev;
    prev = cur;
  }
  return prev[b.length];
}
function searchIndex() {
  const out = [];
  for (const b of system.bodies()) {
    const detail = b.kind === 'moon' ? `Moon of ${b.parent}` : b.kind === 'star' ? 'Our star' : b.kind === 'craft' ? 'Spacecraft' : 'Planet';
    out.push({ name: b.name, key: fold(b.name), id: `body:${b.name}`, detail, weight: 5, typo: true });
  }
  for (const s of named) {
    out.push({ name: s.name, key: fold(s.name), id: `star:${s.name}`, detail: `Star, ${(s.distPc * 3.26156).toFixed(s.distPc < 30 ? 1 : 0)} light-years`, weight: 4, typo: true });
  }
  for (const it of far.items) {
    if (it.name === 'Milky Way') continue;
    out.push({ name: it.name, key: fold(it.name), id: `${it.kind === 'node' ? 'node' : 'gal'}:${it.name}`, detail: it.kind === 'node' ? 'Galaxy cluster' : 'Galaxy', weight: 4, typo: true });
  }
  for (const id of ['v:solar', 'v:nearby', 'v:milkyway', 'v:localgroup', 'v:web', 'v:gc']) {
    const t = makeTarget(id);
    out.push({ name: t.name, key: fold(t.name), id, detail: 'Place', weight: 6, typo: true });
  }
  for (const [alias, id, name] of SEARCH_ALIASES) out.push({ name, key: alias, id, detail: 'Place', weight: 5, typo: false });
  return out;
}
let searchCache = null;
function runSearch(q) {
  if (!searchCache) searchCache = searchIndex();
  const fq = fold(q);
  if (!fq) return [];
  const scored = new Map();
  const add = (name, id, detail, score) => {
    const prev = scored.get(id);
    if (!prev || prev.score < score) scored.set(id, { name, id, detail, score });
  };
  for (const e of searchCache) {
    let s = 0;
    if (e.key === fq) s = 100;
    else if (e.key.startsWith(fq)) s = 80;
    else if (e.key.split(' ').some(w => w.startsWith(fq))) s = 60;
    else if (e.key.includes(fq)) s = 40;
    else if (e.typo && fq.length >= 4) {
      const max = fq.length >= 7 ? 2 : 1;
      if (e.key.split(' ').some(w => editDistance(w, fq, max) <= max) || editDistance(e.key, fq, max) <= max) s = 20;
    }
    if (s) add(e.name, e.id, e.detail, s + e.weight);
  }
  for (const sys of exoSystems) {
    if (sys.distPc == null && !sys.world) continue;
    const h = fold(sys.host);
    if (h === fq) add(sys.host, `exo:${sys.host}`, `Star with ${sys.planets.length} planet${sys.planets.length > 1 ? 's' : ''}`, 95);
    else if (h.startsWith(fq)) add(sys.host, `exo:${sys.host}`, `Star with ${sys.planets.length} planet${sys.planets.length > 1 ? 's' : ''}`, 50);
    else if (fq.length >= 3) {
      const p = sys.planets.find(x => fold(x.name).includes(fq));
      if (p) add(p.name, `exo:${sys.host}`, `Exoplanet of ${sys.host}`, fold(p.name) === fq ? 90 : 38);
    }
  }
  if (/^\d{6,}$/.test(fq) && starsNear) {
    const i = starsNear.meta.findIndex(m => m.id === fq);
    if (i >= 0) add(`Gaia DR3 ${fq}`, `gaia:${i}`, 'Star', 100);
  }
  return [...scored.values()].sort((a, b) => b.score - a.score).slice(0, 10);
}

// ---------- grand tour ----------
const TOUR = ['body:Moon', 'body:Mars', 'body:Jupiter', 'body:Saturn', 'body:Sun', 'exo:TRAPPIST-1', 'v:milkyway', 'gal:Andromeda (M31)', 'v:web', 'body:Earth'];
let tour = null;
function startTour() {
  if (nav.mode === 'free') setFree(false, { quiet: true });
  pushBack();
  if (selection) ui.hideCard();
  tour = { i: -1, phase: 'dwell', t0: -99 };
  ui.setTourOn(true);
}
function stopTour(msg) {
  if (!tour) return;
  tour = null;
  nav.autoSpin = 0;
  ui.setTourOn(false);
  ui.setTourStep('');
  if (msg) ui.toast(msg);
}
function tourSkip() { if (tour) { if (nav.flight) nav.cancelFlight(); tour.phase = 'dwell'; tour.t0 = -99; } }
function tourTick() {
  if (!tour) return;
  if (tour.phase === 'fly') { if (!nav.flight) { tour.phase = 'dwell'; tour.t0 = tSec; } return; }
  if (tSec - tour.t0 < 6.5) return;
  tour.i++;
  if (tour.i >= TOUR.length) { stopTour('Tour complete'); return; }
  const t = makeTarget(TOUR[tour.i]);
  if (!t) { tour.t0 = -99; return; }
  nav.autoSpin = 0;
  ui.setTourStep(`Stop ${tour.i + 1} of ${TOUR.length}: ${t.name}`);
  goTo(t, { speed: 1.25, keepTour: true });
  tour.phase = 'fly';
}

// ---------- ambience (WebAudio; started only from a user gesture) ----------
let amb = null;
async function toggleAmbience() {
  if (!amb) {
    try {
      const m = await import('./audio.js');
      amb = m.initAmbience();
    } catch (e) { ui.toast('Sound is not available in this window'); console.warn('audio', e); return; }
  }
  ui.setSound(amb.toggle());
}

// ---------- picking ----------
const _p = new THREE.Vector3();
function project(world, out) {   // -> {x,y (CSS px), dist, ok}
  _p.copy(world).sub(nav.camWorld);
  out.dist = _p.length();
  // in front of the camera? projected depth is useless here: with far = 1e27 it rounds
  // to exactly 1 past about 1e16 m, so ask the view direction instead
  out.ok = _p.dot(_fwd.set(0, 0, -1).applyQuaternion(camera.quaternion)) > 0;
  _p.project(camera);
  out.x = (_p.x + 1) * window.innerWidth / 2;
  out.y = (1 - _p.y) * window.innerHeight / 2;
  return out;
}
let lastLabels = [];
function pickAll(ndc) {
  const mx = (ndc[0] + 1) * window.innerWidth / 2, my = (1 - ndc[1]) * window.innerHeight / 2;
  // a label is the biggest target on screen
  for (const l of lastLabels) {
    if (l.target && mx >= l.x - l.w / 2 - 4 && mx <= l.x + l.w / 2 + 4 && my >= l.y - l.h - 2 && my <= l.y + 3) return { name: l.text, target: l.target };
  }
  let best = null, bestScore = 14;
  const pr = {};
  const consider = (name, world, radius, target, weight = 1) => {
    project(world, pr);
    if (!pr.ok) return;
    const appPx = radius / pr.dist * pxScale;
    const score = Math.max(0, Math.hypot(pr.x - mx, pr.y - my) - appPx) / weight;
    if (score < bestScore) { bestScore = score; best = { name, target }; }
  };
  for (const b of system.bodies()) {
    if (b.kind === 'moon') {
      const parent = system.body(b.parent);
      const sep = b.world.distanceTo(parent.world) / Math.max(b.world.distanceTo(nav.camWorld), 1) * pxScale;
      if (sep < 18) continue;
    }
    if (b.kind === 'craft' && nav.camWorld.length() > 3e14) continue;
    consider(b.name, b.world, b.radius, () => makeTarget(`body:${b.name}`), b.kind === 'moon' ? 0.8 : 1.3);
  }
  for (const it of pocketItems) {
    consider(it.name, it.world, it.radius, () => (it.entry.exo ? makeTarget(`exo:${it.entry.exo.host}`) : pocketTarget(it.entry)));
  }
  const dSun = nav.camWorld.length();
  if (dSun > 1 * KPC) {
    for (const it of far.items) {
      if (it.name === 'Milky Way') { consider('Milky Way', it.world, 0, () => makeTarget('v:milkyway')); continue; }
      if (it.kind === 'node' && dSun < 1 * MPC) continue;
      consider(it.name, it.world, it.radius, () => makeTarget(`${it.kind === 'node' ? 'node' : 'gal'}:${it.name}`));
    }
  }
  for (const s of named) consider(s.name, s.world, s.radiusM, () => makeTarget(`star:${s.name}`), 0.9);
  if (!best && starsNear && dSun < 2000 * PC) {
    const hit = pickStar(starsNear, ndc, nav.camWorld, camera, 7);
    if (hit) {
      const m = starsNear.meta[hit.idx];
      best = { name: (starsNear.names && starsNear.names.get(hit.idx)) || `Gaia DR3 ${m.id}`, target: () => makeTarget(`gaia:${hit.idx}`) };
    }
  }
  return best;
}
function pocketTarget(entry) {
  if (entry.named) return makeTarget(`star:${entry.named.name}`);
  if (entry.gaiaIdx != null) return makeTarget(`gaia:${entry.gaiaIdx}`);
  return null;
}

// hover feedback: name tip + pointer cursor (throttled pick in the frame loop)
const hover = { px: -1, py: -1, tick: 0, moved: false };
canvas.addEventListener('pointermove', (e) => { hover.px = e.clientX; hover.py = e.clientY; hover.moved = true; });
canvas.addEventListener('pointerleave', () => { hover.px = -1; ui.setHover(null); });

// ---------- labels: priorities, then greedy overlap removal ----------
const pr = {};
function buildLabels(systemView) {
  const cam = nav.camWorld;
  const dSun = cam.length();
  const items = [];
  // nothing is labeled through the disc of the body you are looking at
  const occ = {};
  const fo = nav.focus && nav.focus.radius ? nav.focus : null;
  if (fo) { project(fo.pos(), occ); occ.r = fo.radius / Math.max(occ.dist, 1) * pxScale; }
  const add = (id, text, kind, size, world, radius, prio, target, alpha = 1) => {
    project(world, pr);
    if (fo && occ.ok && pr.dist > occ.dist && Math.hypot(pr.x - occ.x, pr.y - occ.y) < occ.r) return;
    if (!pr.ok || pr.x < -40 || pr.x > window.innerWidth + 40 || pr.y < -20 || pr.y > window.innerHeight + 20) return;
    const appPx = radius / Math.max(pr.dist, 1) * pxScale;
    if (appPx > window.innerHeight * 0.8) return;
    if (pr.x < 190 && pr.y > 60 && pr.y < 620) return;
    if (pr.x < 340 && pr.y > window.innerHeight - 200) return;
    const w = text.length * (size === 'l' ? 8 : size === 'm' ? 7 : 6.2);
    items.push({ id, text, kind, size, x: pr.x, y: pr.y - Math.min(appPx, window.innerHeight) - 5, w, h: size === 'l' ? 20 : 17, prio, target, alpha, discPx: appPx });
  };
  for (const b of system.bodies()) {
    if (b.kind === 'star') {
      if (dSun > 1 * MPC) add('Sun', 'You started here', 'home', 'm', gcWorld, 0, 9, () => makeTarget('body:Earth'));
      else if (dSun > 3e17) add('Sun', dSun > 3 * KPC ? 'Sun, where you started' : 'Sun', 'home', 'm', b.world, 0, 9, () => makeTarget('body:Sun'));
      else add('Sun', 'Sun', 'body', 'm', b.world, b.radius, 8, () => makeTarget('body:Sun'));
    } else if (b.kind === 'planet') {
      if (dSun < 2.5e14) add(b.name, b.name, 'body', 'm', b.world, b.radius, 7, () => makeTarget(`body:${b.name}`));
    } else if (b.kind === 'moon') {
      const parent = system.body(b.parent);
      const d = b.world.distanceTo(cam);
      const sep = b.world.distanceTo(parent.world) / Math.max(d, 1) * pxScale;
      if (sep > 34 && d < parent.radius * 400) add(b.name, b.name, 'body', 's', b.world, b.radius, 5, () => makeTarget(`body:${b.name}`));
    } else if (b.kind === 'craft' && systemView > 0.4 && dSun < 3e14) {
      add(b.name, b.name, 'craft', 's', b.world, 0, 3, () => makeTarget(`body:${b.name}`));
    }
  }
  for (const it of pocketItems) {
    add(`p:${it.name}`, it.name, it.kind === 'star' ? 'star' : 'body', it.kind === 'star' ? 'm' : 's', it.world, it.radius, it.kind === 'star' ? 8 : 6, null);
  }
  if (dSun > 2e15 && dSun < 1200 * PC) {
    for (const s of named) {
      const d = s.world.distanceTo(cam) / PC;
      const m = s.vmag + 5 * Math.log10(Math.max(d, 1e-3) / Math.max(s.distPc, 0.1));
      if (m < 5.2) add(`s:${s.name}`, s.name, 'star', 's', s.world, 0, 4 - m * 0.2, () => makeTarget(`star:${s.name}`));
    }
  }
  if (dSun > 120 * PC && dSun < 3 * MPC) add('gc', 'Galactic center', 'galaxy', 's', gcWorld, 0, 5, () => makeTarget('v:gc'));
  if (dSun > 25 * KPC) {
    for (const it of far.items) {
      if (it.kind === 'node') {
        if (dSun > 4 * MPC) add(it.name, it.name, 'galaxy', 's', it.world, 0, 6, () => makeTarget(`node:${it.name}`));
      } else if (dSun < 12 * MPC) {
        const big = it.radius > 4 * KPC;
        if (it.name === 'Milky Way') add(it.name, it.name, 'galaxy', 'm', it.world, 0, 7, () => makeTarget('v:milkyway'));
        else if (big || dSun < 1.2 * MPC) add(it.name, it.name, 'galaxy', big ? 'm' : 's', it.world, 0, big ? 7 : 3, () => makeTarget(`gal:${it.name}`));
      }
    }
  }
  // the destination gets the top label, unless it is a whole region or already fills
  // the view: then the title at the corner says it, and three "Mars" add nothing
  const f = nav.flight ? nav.flight.t : selection;
  if (f && f.pos && f.kind !== 'place') add(f.name, f.name, 'body', 'l', f.pos(), f.radius || 0, 11, () => f);
  items.sort((a, b) => b.prio - a.prio);
  const placed = [];
  const focusId = nav.focus && nav.focus.name;
  for (const it of items) {
    if (placed.length >= 24) break;
    if (it.text === focusId && it.discPx > 60 && !nav.flight) continue;
    if (placed.some(o => o.id === it.id)) continue;
    const hit = placed.some(o => Math.abs(o.x - it.x) < (o.w + it.w) / 2 + 3 && Math.abs((o.y - o.h / 2) - (it.y - it.h / 2)) < (o.h + it.h) / 2);
    if (!hit) placed.push(it);
  }
  lastLabels = placed;
  ui.renderLabels(placed);
}

// ---------- where am I ----------
function bandName(dSun) {
  if (dSun < 3000 * AU) return 'Solar system';
  if (dSun < 1 * KPC) return 'Nearby stars';
  if (dSun < 140 * KPC) return 'Milky Way';
  if (dSun < 5 * MPC) return 'Local Group';
  return 'Cosmic web';
}
function updatePlace() {
  ui.setFreeOn(nav.mode === 'free');
  const cam = nav.camWorld;
  const dSun = cam.length();
  const f = nav.focus;
  let name, dist;
  if (nav.flight) {
    name = `Flying to ${nav.flight.t.name}`;
    dist = `${prettyDist(Math.max(nav.flight.t.pos().distanceTo(cam) - (nav.flight.t.radius || 0), 0))} to go`;
  } else if (nav.mode === 'free') {
    const near = system.nearestSurface(cam);
    if (near.body && near.dist < near.body.radius * 200) { name = near.body.name; dist = `${prettyDist(near.dist)} above the surface`; }
    else { name = bandName(dSun); dist = `${prettyDist(dSun)} from the Sun`; }
  } else {
    const d = nav.dist();
    // still "at" the focus? near a body, or within a few framings of anything else
    const parent = f && f.chain && f.chain[0] ? f.pos().distanceTo(f.chain[0].pos()) : Infinity;
    const atFocus = f && (f.radius ? d < Math.min(0.35 * parent, 4000 * f.radius) : d < 12 * (f.frame || 1));
    if (atFocus && f.kind !== 'place') {
      name = f.name;
      if (f.radius) dist = `${prettyDist(Math.max(cam.distanceTo(f.pos()) - f.radius, 0))} above the surface`;
      else dist = `${prettyDist(cam.distanceTo(f.pos()))} away`;
    } else {
      name = atFocus ? f.name : bandName(dSun);
      if (name === 'Cosmic web' || name === 'Local Group') dist = `${prettyDist(dSun)} from the Milky Way`;
      else dist = `${prettyDist(dSun)} from the Sun`;
    }
  }
  ui.setPlace({
    name, distance: dist,
    speed: nav.mode === 'free' && nav.speedNow > 0.5 ? prettySpeed(nav.speedNow) : '',
    back: backStack.length > 0, flying: !!nav.flight, lookedAway: nav.lookedAway,
  });
  // scale bar, measured at the distance of what you are looking at
  const depth = Math.max(nav.mode === 'free' ? surfaceDist(cam) : cam.distanceTo(nav.pivot), 1);
  const nice = niceBar(130 * depth / pxScale);
  ui.setScale(nice.meters * pxScale / depth, nice.label);
  // ruler position: distance from Earth, piecewise-log between places
  const earth = system.body('Earth');
  const ld = Math.log10(Math.max(cam.distanceTo(earth.world), 1));
  let pos = 0;
  for (let i = 0; i < PLACES.length - 1; i++) {
    const a = Math.log10(PLACES[i].at), b = Math.log10(PLACES[i + 1].at);
    if (ld <= b) { pos = i + Math.max(0, (ld - a) / (b - a)); break; }
    pos = i + 1;
  }
  ui.setRail(pos);
  if (selection && selection.pos) ui.setLive('range', prettyDist(Math.max(selection.pos().distanceTo(cam) - (selection.radius || 0), 0)));
}

// what you see right now, and how real it is
function updateTruth(panoA) {
  const cam = nav.camWorld;
  const dSun = cam.length();
  const layers = [];
  let summary, cls;
  const kernels = system.usingKernels(clock.jd);
  const stateEv = kernels ? 'de440' : 'approx-elements';
  const add = (label, ev) => layers.push({ label, ev });
  let credit = '';
  if (panoA > 0.02) credit = 'Sky photograph: ESO/S. Brunier';
  if (dSun < 3000 * AU) {
    const nearest = system.nearestSurface(cam);
    // the thing you are looking at, not whichever moon happens to be closest
    const fb = nav.focus && nav.focus.radius && !nav.flight ? system.body(nav.focus.name) : null;
    const near = fb && fb.kind !== 'craft' ? { body: fb, dist: fb.world.distanceTo(cam) - fb.radius } : nearest;
    const rec = near.body && system.groups.get(near.body.name);
    add(kernels ? 'Positions: DE440s and satellite kernels' : 'Positions: mean elements (no kernels)', stateEv);
    if (near.dist < near.body.radius * 300) {
      if (rec && rec.textured) { add(`${near.body.name}: mission imagery`, 'surface-imagery'); credit = `Surface: Solar System Scope, NASA data. ${credit}`; }
      else if (near.body.name === 'Earth' && rec.paintedLand) add('Earth: real outlines, painted colors', 'coastlines');
      else if (near.body.name !== 'Sun') add(`${near.body.name}: procedural surface`, 'procedural-surface');
      if (rec && rec.atmoMat) add('Atmosphere: scattering model', 'atmosphere-model');
      if (rec && rec.cloudsReal) add('Clouds: one saved snapshot', 'cloud-layer');
      if (near.body.name === 'Sun') add('Corona: illustrative', 'corona-illustrative');
    }
    add('Stars: Gaia DR3', 'gaia-dr3');
    if (panoA > 0.02) add('Sky: ESO photograph', 'milkyway-pano');
    if (!sky.hasPano) add('Sky band: illustrative model', 'galaxy-fog');
    const real = rec && (rec.textured || near.body.name === 'Sun');
    summary = kernels && real ? 'Real positions and imagery; lighting modeled' : kernels ? 'Real positions; surfaces procedural here' : 'Modeled positions; no kernels loaded';
    cls = kernels && real ? 'measured' : 'modeled';
  } else if (dSun < 150 * PC) {
    add('Stars: Gaia DR3', 'gaia-dr3');
    if (panoA > 0.02) add('Sky: ESO photograph', 'milkyway-pano');
    if (!sky.hasPano) add('Sky band: illustrative model', 'galaxy-fog');
    if (dSun < 1e16) add('Oort cloud points: synthetic', 'oort-procedural');
    if (dSun > 15 * PC) add('Distant field stars: synthetic', 'stars-synthetic');
    if (pocket.active) add('This star up close: illustrative surface', 'star-surface');
    summary = 'Star positions are real; the rest of the sky fades out here';
    cls = 'measured';
  } else if (dSun < 140 * KPC) {
    add('The Milky Way: illustrative model', 'galaxy-fog');
    add('Nearby stars: Gaia DR3', 'gaia-dr3');
    add('Field stars: synthetic', 'stars-synthetic');
    add('Our position in it: measured', 'compiled-catalog');
    summary = 'Illustrative model of the Galaxy; the Sun\'s place in it is measured';
    cls = 'illustrative';
  } else if (dSun < 5 * MPC) {
    add('Galaxy positions and sizes: catalog', 'compiled-catalog');
    add('Galaxy portraits: illustrative', 'galaxy-portrait');
    summary = 'Real positions; galaxy portraits are illustrative';
    cls = 'illustrative';
  } else {
    add('Named clusters: catalog positions', 'compiled-catalog');
    add('Filaments and field galaxies: synthetic', 'web-procedural');
    summary = 'Named clusters are real; the web between them is synthetic';
    cls = 'illustrative';
  }
  add('Bloom and eye adaptation: display effects', 'display-effects');
  ui.setTruth(summary, cls, layers);
  ui.setCredit(credit.trim());
}

// ---------- date formatting ----------
function fmtDate(jdTdb) {
  const [y, m, d, h, mi] = eph.jdToCal(eph.jdUtcFromTdb(jdTdb));
  const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  return `${y} ${MON[Math.min(Math.max(m, 1) - 1, 11)]} ${d}, ${String(h).padStart(2, '0')}:${String(mi).padStart(2, '0')} UTC`;
}

// ---------- quality ----------
function saveQuality(mode) { try { localStorage.setItem('pantheon.quality', mode); } catch { /* private mode */ } }
function loadQuality() { try { return localStorage.getItem('pantheon.quality'); } catch { return null; } }
const gov = new Governor(startTier, (q, key) => {
  applyQuality(q);
  ui.setQuality(gov.mode, q.label);
  if (QA) console.log(`[pantheon] quality tier ${key}`);
});
textures.onUpload = () => gov.pause(1800);
textures.onUploaded = (file, ms) => { if (QA) console.log(`[pantheon] upload ${file}: ${ms.toFixed(0)} ms`); };
// a window drag sends many resizes: reallocate the render targets once it settles
let resizeT = 0;
window.addEventListener('resize', () => {
  clearTimeout(resizeT);
  resizeT = setTimeout(() => { applySize(); gov.pause(1500); }, 120);
});

applyQuality(quality);
{
  const wanted = params.get('quality') || loadQuality();
  if (wanted && (wanted === 'auto' || TIERS[wanted])) { gov.mode = wanted; if (wanted !== 'auto') gov.set(wanted); }
  ui.setQuality(gov.mode, TIERS[gov.tier].label);
}

// ---------- boot ----------
const earthT = makeTarget('body:Earth');
{
  const b = system.body('Earth');
  const dir = litDir(b.world, null, { phase: 64, elev: 16 });
  const d = nav.arriveDistance(earthT, camera.aspect);
  nav.place(earthT, d - b.radius, dir);
}
ui.setRateActive(clock.rate, false);
ui.setSound(false);

const meta = { spice: false, textures: [] };
async function boot() {
  ui.setLoading('Loading kernels and catalogs');
  let metaOk = false;
  try {
    const m = await (await fetch('/api/meta')).json();
    meta.spice = !!m.spice; meta.textures = m.textures || [];
    metaOk = true;
  } catch (e) { console.warn('meta', e); }
  textures.setAvailable(meta.textures);
  // the opening view first; everything else waits until you approach it
  for (const rec of system.groups.values()) rec.wantTex = false;
  system.wantTextures('Earth', true);
  if (textures.has('eso_milkyway.jpg')) textures.request('eso_milkyway.jpg', (t) => sky.setPano(t));

  const lines = [];
  const missingMaps = !textures.has('8k_earth_daymap.jpg');
  if (missingMaps) lines.push('No surface maps in assets/data/textures: planets are procedural, Earth is real outlines with painted colors, and the sky is the illustrative Galaxy model instead of a photograph.');
  if (!meta.spice) lines.push('No SPICE kernels in assets/data/spice: planets and moons sit where mean orbital elements put them, thousands of kilometres off at best, and moon phases are approximate.');
  if (lines.length) lines.push('Run tools/fetch_data.py to download them, then restart.');
  if (!metaOk) lines.unshift('The data server did not answer, so the catalogs may be missing too.');
  ui.setNotice(lines);

  ui.setLoading('Placing the stars');
  await Promise.allSettled([catalogs.near, catalogs.bright, catalogs.exo]);
  searchCache = null;
  await new Promise(r => setTimeout(r, 30));
  ui.setLoading('Spinning the cosmic web');
  await new Promise(r => setTimeout(r, 30));
  try { buildWeb(); } catch (e) { console.warn('cosmic web', e); }
  Promise.all([data.loadCoast('/data/ne_110m_coastline.geojson'), loadLandRings('/data/ne_110m_land.geojson')])
    .then(([coast, land]) => system.addEarthExtras(coast, land, mulberry32(7)))
    .catch(e => console.warn('coast', e));
  // wait a moment for Earth's maps so the opening is the real planet, but never long
  const t0 = performance.now();
  while (textures.pending() && performance.now() - t0 < 5000) await new Promise(r => setTimeout(r, 100));
  ui.hideLoader();
  // one orchestrated moment: glide in on Earth
  if (!reduceMotion()) {
    nav.lnAlt = nav.lnGoal + 1.15;
    nav.zoomRate = 0.75;
    setTimeout(() => { nav.zoomRate = 10; }, 5200);
  }
  setTimeout(() => ui.showHint(), 1600);
  if (QA) console.log(`[pantheon] ready; spice ${meta.spice}, textures ${meta.textures.length}`);
}

// ---------- main loop ----------
let last = performance.now();
let tSec = 0;
const frameMs = new Float32Array(240);
let frameN = 0;
let captureName = null;
const fpsEl = params.has('fps') ? Object.assign(document.createElement('div'), { id: 'fps' }) : null;
if (fpsEl) document.body.appendChild(fpsEl);
function frameStats() {
  const n = Math.min(frameN, frameMs.length);
  if (!n) return { n: 0 };
  const a = Array.from(frameMs.subarray(0, n)).sort((x, y) => x - y);
  const sum = a.reduce((s, v) => s + v, 0);
  return { n, avg: sum / n, p50: a[Math.floor(n * 0.5)], p95: a[Math.floor(n * 0.95)], max: a[n - 1], fps: 1000 / (sum / n) };
}
let lastLog = 0;
let workMs = 0, workSamples = 0;
const gl = renderer.getContext();
function frame(now) {
  requestAnimationFrame(frame);
  const rawMs = now - last;
  last = now;
  const dt = Math.min(rawMs / 1000, QA ? 0.5 : 0.1);
  if (dt <= 0) return;
  frameMs[frameN++ % frameMs.length] = rawMs;
  tSec += dt;
  if (!clock.paused) clock.jd += clock.rate * dt / 86400;
  maybeFetchStates(now);
  textures.tick();
  system.updatePositions(clock.jd);
  nav.update(dt);
  tourTick();

  // no body may be entered, whatever got us here
  {
    const near = system.nearestSurface(nav.camWorld);
    if (near.body) {
      const floor = near.body.radius * 1.002;
      const d = nav.camWorld.distanceTo(near.body.world);
      if (d < floor && d > 0) nav.camWorld.copy(near.body.world).addScaledVector(nav.camWorld.clone().sub(near.body.world).normalize(), floor);
    }
  }

  // camera
  const fov = FOV + (nav.warp * 9);
  if (Math.abs(camera.fov - fov) > 0.01) { camera.fov = fov; camera.updateProjectionMatrix(); }
  camera.quaternion.copy(nav.quat);
  camera.position.set(0, 0, 0);
  camera.updateMatrixWorld();
  post.warp = nav.warp;

  // scale fades: each layer hands over to the next with no gap
  const cam = nav.camWorld;
  const dPc = cam.length() / PC;
  const haveSky = sky.hasPano;
  const panoA = haveSky ? 1 - sst(Math.log10(Math.max(dPc, 1e-6)), Math.log10(15), Math.log10(150)) : 0;
  const nearFade = haveSky ? sst(Math.log10(Math.max(dPc, 1e-6)), Math.log10(15), Math.log10(150)) : 1;
  // away from the Sun's glare the eye opens up: stars read brighter and hold on longer
  const starOpen = sst(Math.log10(Math.max(dPc, 1e-6)), Math.log10(0.3), Math.log10(9));
  for (const m of starMats) m.uniforms.uGain.value = 1 + 1.9 * starOpen;
  fieldPts.material.uniforms.uGain.value = (1 + 1.9 * starOpen) * sst(Math.log10(Math.max(dPc, 1e-6)), Math.log10(15), Math.log10(120));
  rings.update(dPc, cam);
  mwPts.material.uniforms.uB.value = 0.42 * sst(Math.log10(Math.max(dPc, 1e-6)), Math.log10(300), Math.log10(4000));
  oortPts.material.uniforms.uB.value = 0.3 * (1 - sst(dPc, 0.3, 3));
  far.pts.material.uniforms.uB.value = nearFade;
  dwarfPts.material.uniforms.uB.value = 0.55 * nearFade;
  const dMpc = cam.length() / MPC;
  const webF = sst(Math.log10(Math.max(dMpc, 1e-6)), Math.log10(0.2), Math.log10(5));
  if (webPts) { webPts.material.uniforms.uB.value = WEB.g * webF; webFog.material.uniforms.uB.value = WEB.f * webF; }
  helioGroup.position.set(-cam.x, -cam.y, -cam.z);
  mwGroup.position.set(gcWorld.x - cam.x, gcWorld.y - cam.y, gcWorld.z - cam.z);
  for (const p of portraits) p.group.position.set(p.world.x - cam.x, p.world.y - cam.y, p.world.z - cam.z);

  const { systemView } = system.updateRender(clock.jd, cam, nav.quat, tSec, pxScale);
  pocketTick(clock.jd, cam, tSec);

  // volumes: each hands over to its far sprite as it shrinks
  const volOn = volume.update(cam, camera, pxScale, nearFade);
  far.items.forEach((it, i) => {
    if (it.volumeI === undefined || it.volumeI < 0) return;
    const k = it.I * (1 - volume.instances[it.volumeI].fadePx);
    const a = far.pts.geometry.getAttribute('col');
    a.setXYZ(i, it.col[0] * k, it.col[1] * k, it.col[2] * k);
    a.needsUpdate = true;
  });
  for (const p of portraits) p.group.visible = volume.instances[portraits.indexOf(p) + 1].fade > 0.01;
  mwGroup.visible = volume.instances[0].fade > 0.01;
  if (volOn) volume.render();
  sky.update(camera, panoA, volOn ? volume.rt : null);

  buildLabels(systemView);

  // throttled: hover pick, reticle, readouts
  hover.tick++;
  if (hover.tick % 6 === 0) {
    if (hover.px >= 0 && !nav.dragging && !nav.locked && !nav.flight) {
      const nx = (hover.px / window.innerWidth) * 2 - 1, ny = -((hover.py / window.innerHeight) * 2 - 1);
      const hit = pickAll([nx, ny]);
      ui.setHover(hit ? { text: hit.name, x: hover.px, y: hover.py } : null);
      canvas.style.cursor = hit ? 'pointer' : 'default';
    } else {
      ui.setHover(null);
      canvas.style.cursor = nav.dragging ? 'grabbing' : 'default';
    }
    updatePlace();
    ui.setDate(fmtDate(clock.jd));
    if (selection && selection.pos && selection !== nav.focus && !nav.flight) {
      const q = {};
      project(selection.pos(), q);
      const r = (selection.radius || 0) / Math.max(q.dist, 1) * pxScale;
      if (q.ok && r < 100) ui.setReticle({ x: q.x, y: q.y, r: Math.max(r * 1.25 + 8, 14) });
      else ui.setReticle(null);
    } else ui.setReticle(null);
  }
  if (hover.tick % 30 === 0) {
    updateTruth(panoA);
    const s = frameStats();
    if (s.n) {
      const line = `${s.fps.toFixed(0)} fps, ${s.avg.toFixed(1)} ms per frame, ${s.p95.toFixed(1)} ms slowest 5%, ${workMs.toFixed(1)} ms render, ${quality.label}`;
      ui.setFrameRead(line);
      if (fpsEl) fpsEl.textContent = `${line}\n${renderer.domElement.width}×${renderer.domElement.height}, max ${s.max.toFixed(0)} ms`;
      if (QA && params.has('fps') && now - lastLog > 5000) {
        lastLog = now;
        console.log(`[pantheon] frame: ${line}, ${renderer.domElement.width}x${renderer.domElement.height}, max ${s.max.toFixed(0)} ms`);
      }
    }
  }
  // WebKitGTK has no GPU timer and its frame interval can hide GPU load, so every
  // 40th frame (every frame with ?fps) is timed through glFinish: the true cost.
  const measure = fpsEl || hover.tick % 40 === 0;
  const work0 = measure ? performance.now() : 0;
  post.render(dt);
  if (measure) {
    gl.finish();
    workMs += ((performance.now() - work0) - workMs) * (workSamples++ ? 0.25 : 1);
  }
  gov.tick(rawMs, workMs);
  if (captureName) {
    const name = captureName;
    captureName = null;
    const url = canvas.toDataURL('image/jpeg', 0.92);
    fetch(`/api/shot?name=${encodeURIComponent(name)}`, { method: 'POST', body: url.slice(url.indexOf(',') + 1) }).catch(() => {});
  }
}
requestAnimationFrame(frame);
document.addEventListener('visibilitychange', () => { if (!document.hidden) { last = performance.now(); gov.pause(1500); } });
boot().catch(e => { console.error('boot', e); ui.hideLoader(); });

// test handle: camera state and frame times, read from inside the real window
window.__pantheon = {
  renderer, scene, camera, nav, clock, system, post, gov, volume, sky, ui, makeTarget, runSearch, goTo, tiers: TIERS,
  stats: () => ({ ...frameStats(), workMs }),
  resetStats() { frameN = 0; },
  state() {
    return {
      mode: nav.mode, focus: nav.focus && nav.focus.name, flying: !!nav.flight, flightTo: nav.flight && nav.flight.t.name, locked: nav.locked,
      cam: nav.camWorld.toArray(), dist: nav.camWorld.distanceTo(nav.pivot), altKm: nav.focus && nav.focus.radius ? (nav.camWorld.distanceTo(nav.focus.pos()) - nav.focus.radius) / 1e3 : null,
      tier: gov.tier, mode_q: gov.mode, tour: tour && { i: tour.i, phase: tour.phase }, back: backStack.length,
      sel: selection && selection.name, size: [innerWidth, innerHeight], canvas: [canvas.width, canvas.height],
      volOn: volume.active, panoHas: sky.hasPano, pending: textures.pending(), jd: clock.jd,
    };
  },
  // jump without a flight: for repeatable test views
  view(id, k = 1, opts = {}) {
    const t = makeTarget(id);
    if (!t) return false;
    if (t.onApproach) t.onApproach();
    const d = nav.arriveDistance(t, camera.aspect) * k;
    const dir = opts.dir ? new THREE.Vector3(...opts.dir) : (t.arrivalDir ? t.arrivalDir(nav.dir.clone()) : nav.dir.clone());
    nav.place(t, Math.max(d - (t.radius || 0), 1), dir);
    return true;
  },
  capture(name) { captureName = name; },
  labels: () => lastLabels.map(l => ({ text: l.text, x: l.x, y: l.y, h: l.h })),
};
