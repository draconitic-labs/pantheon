// Pantheon 3D — boot + main loop. The camera never leaves the scene origin;
// nav.camWorld is the f64 world position and every group is re-parked
// camera-relative each frame (floating origin). Log depth buffer spans the
// ~19 orders of magnitude between a coastline and the cosmic web.
import * as THREE from 'three';
import { EffectComposer } from 'three/addons/postprocessing/EffectComposer.js';
import { RenderPass } from 'three/addons/postprocessing/RenderPass.js';
import { UnrealBloomPass } from 'three/addons/postprocessing/UnrealBloomPass.js';
import { OutputPass } from 'three/addons/postprocessing/OutputPass.js';
import * as eph from './eph.js';
import * as data from './data.js';
import { initUI } from './ui.js';
import { Nav, prettyDist, prettySpeed } from './nav.js';
import { SolarSystem } from './scene/system.js';
import { makeStarPoints, pickStar } from './scene/stars.js';
import { buildMilkyWay, buildGalaxyDisc, mulberry32 } from './scene/galaxy.js';
import { LOCAL_GROUP, COSMIC_NODES, buildCosmicWeb, buildOort } from './scene/cosmos.js';

const AU = 1.495978707e11, PC = 3.0856775814913673e16, KPC = 1e3 * PC, RSUN = 6.957e8;

// ---------- renderer ----------
const canvas = document.getElementById('canvas');
const renderer = new THREE.WebGLRenderer({
  canvas, antialias: true, logarithmicDepthBuffer: true, powerPreference: 'high-performance',
});
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.05;
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x000000);
const camera = new THREE.PerspectiveCamera(60, window.innerWidth / window.innerHeight, 0.5, 1e27);
const composer = new EffectComposer(renderer);
composer.addPass(new RenderPass(scene, camera));
const bloom = new UnrealBloomPass(new THREE.Vector2(window.innerWidth, window.innerHeight), 0.65, 0.45, 1.0);
composer.addPass(bloom);
composer.addPass(new OutputPass());
window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
  composer.setSize(window.innerWidth, window.innerHeight);
});

// ---------- clock (TDB) ----------
const clock = {
  jd: eph.jdTdb(eph.jdUtcNow()),
  rate: 60,             // sim seconds per real second
  prevRate: 60,
};

// ---------- world content ----------
const system = new SolarSystem(scene);
const rand = mulberry32(1877);
const cloudMats = [];   // fog-cloud materials needing per-frame uPxScale

// galactic frame: ecliptic-axis vectors for placing the galaxy
const gcWorld = galacticCenterWorld();
function galacticCenterWorld() {
  const dir = data.icrsToEcl(266.405, -28.93617);
  return dir.multiplyScalar(8.178 * KPC);
}
const mwCloud = buildMilkyWay(rand);
const mwPts = cloudToPoints(mwCloud, 55 * PC, 0.5, 17);
const mwGroup = new THREE.Group();
mwGroup.add(mwPts);
scene.add(mwGroup);

// M31 — a smaller disc at Andromeda's true position, roughly oriented
let m31Group = null, m31World = null;
const webGroup = new THREE.Group();  // sun-centered procedural cosmos
scene.add(webGroup);
let webMat = null;
try {
  const web = buildCosmicWeb(mulberry32(4242));
  const webPts = cloudToPoints(web, 1.4e3 * KPC, 0.16, 20);
  webMat = webPts.material;
  webGroup.add(webPts);
  const oort = buildOort(mulberry32(99));
  webGroup.add(cloudToPoints(oort, 3.0e12, 0.45, 3));
} catch (e) { console.warn('cosmos build', e); }
try {
  const m31 = LOCAL_GROUP.find(g => /andromeda|m ?31/i.test(g.name));
  if (m31 && m31.world) {
    m31World = m31.world.clone();
    const disc = buildGalaxyDisc(mulberry32(31), { scale: 1.3 });
    m31Group = new THREE.Group();
    m31Group.add(cloudToPoints(disc, 90 * PC, 0.55, 15));
    m31Group.rotation.set(1.0, 0.4, 0.7);
    scene.add(m31Group);
  }
} catch (e) { console.warn('m31 build', e); }

// local-group galaxies + cosmic nodes as physically-fading glow points
const farGlows = makeFarGlows();
scene.add(farGlows.group);
function makeFarGlows() {
  const items = [];
  for (const g of LOCAL_GROUP) {
    if (!g.world || /milky/i.test(g.name)) continue;
    items.push({ name: g.name, kind: 'galaxy', world: g.world, lum: g.spiral ? 3.0e8 : 2.0e6, col: g.spiral ? [1.0, 0.9, 0.72] : [0.85, 0.8, 0.7], radius: ((g.sizeKpc || 10) * KPC) / 2, info: g });
  }
  for (const n of COSMIC_NODES) {
    items.push({ name: n.name, kind: 'node', world: n.world, lum: 6.0e10, col: [0.72, 0.78, 1.0], radius: 2000 * KPC, info: n });
  }
  const n = items.length;
  const pos = new Float32Array(n * 3), col = new Float32Array(n * 3), lum = new Float32Array(n);
  items.forEach((it, i) => {
    pos.set([it.world.x, it.world.y, it.world.z], 3 * i);
    col.set(it.col, 3 * i);
    lum[i] = it.lum;
  });
  // galaxies/clusters at Mpc range ARE point sources — the star shader's
  // flux model is exactly right for them (lum encoded via synthetic meta)
  const set = {
    count: n, positions: pos, colors: col,
    meta: items.map(it => ({ gmag: -2.5 * Math.log10(it.lum), distPc: 10 })),
  };
  const group = new THREE.Group();
  group.add(makeStarPoints(set, 8.0));
  return { group, items };
}

// Fog clouds are EXTENDED emission, not point sources: constant surface
// brightness, angular size ∝ blob/d — so a galaxy holds its glow from any
// distance, and dims away naturally when the camera is inside it (a blob
// bigger than uMaxPx is "resolved" and fades by the capped-area ratio).
function cloudToPoints(cloud, blobM, bright, maxPx) {
  const count = cloud.positions.length / 3;
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(cloud.positions, 3));
  g.setAttribute('col', new THREE.BufferAttribute(cloud.colors, 3));
  g.setAttribute('siz', new THREE.BufferAttribute(cloud.sizes || new Float32Array(count).fill(1), 1));
  g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e26);
  const mat = new THREE.ShaderMaterial({
    vertexShader: mwShaderV(), fragmentShader: mwShaderF(),
    uniforms: {
      uBlobPc: { value: blobM / PC }, uPc: { value: PC }, uB: { value: bright },
      uMaxPx: { value: maxPx }, uPxScale: { value: 430 },
    },
    blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
  });
  cloudMats.push(mat);
  const pts = new THREE.Points(g, mat);
  pts.frustumCulled = false;
  return pts;
}
// soft fog-sprite shader (wider falloff than stars)
function mwShaderV() {
  return /* glsl */`
    attribute vec3 col; attribute float siz;
    varying vec3 vCol; varying float vA;
    uniform float uBlobPc; uniform float uPc; uniform float uB; uniform float uMaxPx; uniform float uPxScale;
    #include <common>
    #include <logdepthbuf_pars_vertex>
    void main() {
      vCol = col;
      vec4 mv = modelViewMatrix * vec4(position, 1.0);
      // divide BEFORE length: squaring meter-scale components overflows f32
      float d = length(mv.xyz / uPc);
      float pxRaw = uBlobPc * siz / d * uPxScale;
      float px = clamp(pxRaw, 0.75, uMaxPx);
      float a = uB;
      if (pxRaw > uMaxPx) a *= (uMaxPx * uMaxPx) / (pxRaw * pxRaw);  // resolved out
      float sub = clamp(pxRaw / 1.3, 0.0, 1.0);
      a *= sub * sub;                                                // sub-pixel energy
      vA = a;
      gl_PointSize = px;
      gl_Position = projectionMatrix * mv;
      #include <logdepthbuf_vertex>
    }`;
}
function mwShaderF() {
  return /* glsl */`
    varying vec3 vCol; varying float vA;
    #include <common>
    #include <logdepthbuf_pars_fragment>
    void main() {
      #include <logdepthbuf_fragment>
      vec2 pc = gl_PointCoord * 2.0 - 1.0;
      float d2 = dot(pc, pc);
      if (d2 > 1.0) discard;
      gl_FragColor = vec4(vCol * exp(-d2 * 3.0) * vA, 1.0);
    }`;
}

// star catalogs (async)
let starsNear = null, starsBright = null, starPtsNear = null, starPtsBright = null;
const starGroup = new THREE.Group();
scene.add(starGroup);
let exoSystems = [];
// compiled bright stars — Gaia saturates on Sirius-class stars, so these carry
// their own glow points and are the canonical label/search source
const named = (data.NAMED_STARS || []).map(({ name, ra, dec, distPc, vmag, ci }) => {
  const world = data.icrsToEcl(ra, dec).multiplyScalar(distPc * PC);
  const Mv = vmag - 5 * Math.log10(Math.max(distPc, 0.1) / 10);
  const lum = Math.pow(10, -0.4 * Mv);
  return { name, world, distPc, vmag, ci: ci ?? 0.6, lum, radiusM: Math.sqrt(Math.max(lum, 1e-4)) * RSUN };
});
{
  const n = named.length;
  const pos = new Float32Array(3 * n), col = new Float32Array(3 * n);
  named.forEach((s, i) => {
    pos.set([s.world.x, s.world.y, s.world.z], 3 * i);
    const c = data.starRgb ? data.starRgb(s.ci) : [1, 0.95, 0.85];
    col.set(c, 3 * i);
  });
  const set = { count: n, positions: pos, colors: col, meta: named.map(s => ({ gmag: s.vmag, distPc: s.distPc })) };
  starGroup.add(makeStarPoints(set, 8.0));
}
data.loadStars().then(s => {
  starsNear = s;
  starPtsNear = makeStarPoints(s, 8.0);
  starGroup.add(starPtsNear);
  if (data.nameStars) data.nameStars(s);   // names used for pick/card titles
  resolveExoDistances();
}).catch(e => console.warn('gaia nearby', e));
// systems whose archive default row lacks sy_dist get it from Gaia by
// angular match (<0.05 deg) — TRAPPIST-1 lives in gaia_nearby, for instance
function resolveExoDistances() {
  if (!starsNear || !exoSystems.length) return;
  const DEG = Math.PI / 180, tol = Math.cos(0.05 * DEG);
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
      // catalog position is authoritative — the archive RA/Dec can sit
      // thousands of AU off at these distances, stranding the pocket
      const p = starsNear.positions;
      sys.world = new THREE.Vector3(p[3 * bi], p[3 * bi + 1], p[3 * bi + 2]);
    }
  }
}
data.loadBrightStars().then(s => {
  starsBright = s;
  starPtsBright = makeStarPoints(s, 6.5);
  starGroup.add(starPtsBright);
}).catch(e => console.warn('gaia bright', e));
data.loadExo().then(x => { exoSystems = x; resolveExoDistances(); }).catch(e => console.warn('exo', e));
async function loadLandRings(url) {   // Polygon/MultiPolygon rings (even-odd)
  const r = await fetch(url);
  const gj = await r.json();
  const rings = [];
  for (const f of gj.features || []) {
    const g = f && f.geometry;
    if (!g) continue;
    if (g.type === 'Polygon') rings.push(...g.coordinates);
    else if (g.type === 'MultiPolygon') for (const poly of g.coordinates) rings.push(...poly);
  }
  return rings;
}
Promise.all([data.loadCoast('/data/ne_110m_coastline.geojson'), loadLandRings('/data/ne_110m_land.geojson')])
  .then(([coast, land]) => system.addEarthExtras(coast, land, mulberry32(7)))
  .catch(e => console.warn('coast', e));
data.loadTracks().then(t => system.setTracks(t)).catch(() => {});

// ---------- states API (SPICE truth from the Rust core) ----------
let fetching = false;
function maybeFetchStates() {
  const c = system.stateCache;
  if (fetching) return;
  if (c && Math.abs(clock.jd - c.jd) < 0.2) return;
  fetching = true;
  fetch(`/api/states?jd=${clock.jd}`).then(r => r.json()).then(j => {
    system.setStates(j);
  }).catch(() => {}).finally(() => { fetching = false; });
}

// ---------- star pocket (any star up close; exoplanets if the host has them) ----------
const pocket = {
  group: new THREE.Group(), active: null, starMesh: null, planets: [], orbits: [],
};
scene.add(pocket.group);
function starPhysical(idx) {
  const m = starsNear.meta[idx];
  const mg = m.gmag - 5 * Math.log10(Math.max(m.distPc, 0.1) / 10);
  const L = Math.pow(10, -0.4 * (mg - 4.67));
  const t = Math.max(0.1, Math.min(3.0, m.bprp ?? 0.8));
  const T = 8500 / (t + 1.05);                     // very rough bp_rp -> Teff
  const R = Math.sqrt(Math.max(L, 1e-6)) * Math.pow(5772 / T, 2);
  return { L, T, R: Math.max(R, 0.08) };
}
function activatePocket(entry) {   // {name, world, radiusM, colorHex, exo}
  clearPocket();
  pocket.active = entry;
  const geo = new THREE.SphereGeometry(1, 64, 48);
  const mat = new THREE.ShaderMaterial({
    vertexShader: starMeshV(), fragmentShader: starMeshF(),
    uniforms: {
      uCol: { value: new THREE.Color(entry.colorHex || '#ffd9a0') },
      uTime: { value: 0 },
    },
  });
  pocket.starMesh = new THREE.Mesh(geo, mat);
  pocket.starMesh.scale.setScalar(entry.radiusM);
  pocket.group.add(pocket.starMesh);
  if (entry.exo) {
    for (const pl of entry.exo.planets) {
      if (!pl.aAU) continue;
      const N = 128, pts = [];
      for (let k = 0; k <= N; k++) {
        const th = 2 * Math.PI * k / N;
        pts.push(new THREE.Vector3(Math.cos(th) * pl.aAU * AU, Math.sin(th) * pl.aAU * AU, 0));
      }
      const g = new THREE.BufferGeometry().setFromPoints(pts);
      const line = new THREE.Line(g, new THREE.LineBasicMaterial({
        color: 0xd8b36a, transparent: true, opacity: 0.35, blending: THREE.AdditiveBlending, depthWrite: false,
      }));
      line.frustumCulled = false;
      pocket.group.add(line);
      pocket.orbits.push(line);
      const pr = Math.max((pl.radE || 1) * 6.371e6, 1e6);
      const pm = new THREE.Mesh(geo, new THREE.MeshBasicMaterial({ color: 0x9a9284 }));
      pm.scale.setScalar(pr);
      pocket.group.add(pm);
      pocket.planets.push({ mesh: pm, pl, radius: pr });
    }
  }
}
function clearPocket() {
  pocket.group.clear();
  pocket.active = null; pocket.starMesh = null; pocket.planets = []; pocket.orbits = [];
}
function starMeshV() {
  return /* glsl */`
    varying vec3 vObj; varying vec3 vN; varying vec3 vV;
    #include <common>
    #include <logdepthbuf_pars_vertex>
    void main() {
      vObj = position;
      vec4 mv = modelViewMatrix * vec4(position, 1.0);
      vN = normalize(normalMatrix * normal);
      vV = normalize(mv.xyz);
      gl_Position = projectionMatrix * mv;
      #include <logdepthbuf_vertex>
    }`;
}
function starMeshF() {
  return /* glsl */`
    varying vec3 vObj; varying vec3 vN; varying vec3 vV;
    uniform vec3 uCol; uniform float uTime;
    #include <common>
    #include <logdepthbuf_pars_fragment>
    float hash1(vec3 p){p=fract(p*0.3183099+vec3(0.1,0.17,0.13));p*=17.0;return fract(p.x*p.y*p.z*(p.x+p.y+p.z));}
    float vnoise(vec3 p){vec3 i=floor(p),f=fract(p);f=f*f*(3.0-2.0*f);
      return mix(mix(mix(hash1(i),hash1(i+vec3(1,0,0)),f.x),mix(hash1(i+vec3(0,1,0)),hash1(i+vec3(1,1,0)),f.x),f.y),
                 mix(mix(hash1(i+vec3(0,0,1)),hash1(i+vec3(1,0,1)),f.x),mix(hash1(i+vec3(0,1,1)),hash1(i+vec3(1,1,1)),f.x),f.y),f.z);}
    void main() {
      #include <logdepthbuf_fragment>
      vec3 n = normalize(vObj);
      float g = vnoise(n * 12.0 + uTime * 0.02) * 0.6 + vnoise(n * 34.0) * 0.4;
      float limb = pow(max(dot(-vV, vN), 0.0), 0.5);
      gl_FragColor = vec4(uCol * (0.5 + 0.9 * g) * (0.5 + 1.0 * limb) * 2.6, 1.0);
    }`;
}
function pocketUpdate(jd, camWorld, tSec) {
  if (!pocket.active) return [];
  const e = pocket.active;
  pocket.group.position.set(e.world.x - camWorld.x, e.world.y - camWorld.y, e.world.z - camWorld.z);
  if (pocket.starMesh) pocket.starMesh.material.uniforms.uTime.value = tSec;
  const out = [{ name: e.name, kind: 'star', world: e.world, radius: e.radiusM, exo: e.exo }];
  for (const p of pocket.planets) {
    const ph = 2 * Math.PI * (((jd / p.pl.perD) % 1 + 1) % 1) + (p.pl.name.length * 1.7);
    const rel = new THREE.Vector3(Math.cos(ph) * p.pl.aAU * AU, Math.sin(ph) * p.pl.aAU * AU, 0);
    p.mesh.position.copy(rel);
    out.push({
      name: p.pl.name, kind: 'exoplanet',
      world: e.world.clone().add(rel), radius: p.radius, pl: p.pl, host: e.name,
    });
  }
  return out;
}
function nearestCatalogStar(camWorld) {
  if (!starsNear) return null;
  const p = starsNear.positions;
  let best = Infinity, bi = -1;
  for (let i = 0; i < starsNear.count; i++) {
    const dx = p[3 * i] - camWorld.x, dy = p[3 * i + 1] - camWorld.y, dz = p[3 * i + 2] - camWorld.z;
    const d2 = dx * dx + dy * dy + dz * dz;
    if (d2 < best) { best = d2; bi = i; }
  }
  return bi >= 0 ? { idx: bi, dist: Math.sqrt(best) } : null;
}
function starEntry(idx) {
  const m = starsNear.meta[idx];
  const world = new THREE.Vector3(starsNear.positions[3 * idx], starsNear.positions[3 * idx + 1], starsNear.positions[3 * idx + 2]);
  const name = (starsNear.names && starsNear.names.get(idx)) || `Gaia DR3 ${m.id}`;
  const phys = starPhysical(idx);
  // exo host? matched during distance resolution, or by proximity
  let exo = exoSystems.find(s => s.gaiaIdx === idx) || null;
  if (!exo) {
    for (const s of exoSystems) {
      if (!s.distPc || !s.world) continue;
      if (s.world.distanceTo(world) < 0.1 * PC) { exo = s; break; }
    }
  }
  const c = starsNear.colors;
  const colorHex = new THREE.Color(Math.min(c[3 * idx], 1), Math.min(c[3 * idx + 1], 1), Math.min(c[3 * idx + 2], 1)).getHexString();
  return { name: exo ? exo.host : name, world, radiusM: (exo && exo.stRad ? exo.stRad : phys.R) * RSUN, colorHex: `#${colorHex}`, exo, gaiaIdx: idx };
}
function exoHostEntry(sys) {
  const world = sys.world || data.icrsToEcl(sys.ra, sys.dec).multiplyScalar(sys.distPc * PC);
  return { name: sys.host, world, radiusM: (sys.stRad || 1) * RSUN, colorHex: '#ffe0b0', exo: sys, gaiaIdx: sys.gaiaIdx };
}

// ---------- UI ----------
const backStack = [];
let selection = null;    // {name, kind, world (Vector3 or fn), radius, ...}
const ui = initUI({
  search(q) {
    const out = [], ql = q.toLowerCase();
    if (!ql) return out;
    for (const b of system.bodies()) {
      if (b.name.toLowerCase().startsWith(ql)) out.push({ name: b.name, kind: b.kind, detail: b.parent || '', ref: b });
    }
    for (const s of named) {
      if (s.name.toLowerCase().includes(ql)) out.push({ name: s.name, kind: 'star', detail: `${s.distPc.toFixed(1)} pc`, ref: { namedStar: s } });
    }
    for (const s of exoSystems) {
      if (s.distPc == null) continue;      // unresolvable distance — can't fly there
      if (s.host.toLowerCase().includes(ql)) { out.push({ name: s.host, kind: 'exo system', detail: `${s.planets.length} planets`, ref: { exo: s } }); }
      else if (s.planets.some(p => p.name.toLowerCase().includes(ql))) out.push({ name: s.planets.find(p => p.name.toLowerCase().includes(ql)).name, kind: 'exoplanet', detail: s.host, ref: { exo: s } });
      if (out.length > 24) break;
    }
    for (const it of farGlows.items) {
      if (it.name.toLowerCase().includes(ql)) out.push({ name: it.name, kind: it.kind, detail: '', ref: { glow: it } });
    }
    return out.slice(0, 12);
  },
  pick(res) {
    const r = res.ref;
    pushBack();
    if (r.namedStar) {
      const s = r.namedStar;
      selection = { name: s.name, kind: 'star', world: s.world, radius: s.radiusM, named: s };
      nav.flyTo(() => ({ pos: s.world, radius: Math.max(s.radiusM * 40, 0.5 * AU) }));
    } else if (r.star !== undefined) {
      const e = starEntry(r.star);
      selection = { name: e.name, kind: 'star', world: e.world, radius: e.radiusM, starIdx: r.star };
      nav.flyTo(() => ({ pos: e.world, radius: Math.max(e.radiusM * 40, 0.5 * AU) }));
    } else if (r.exo) {
      const e = exoHostEntry(r.exo);
      selection = { name: e.name, kind: 'star', world: e.world, radius: e.radiusM, exo: r.exo };
      const aMax = r.exo.planets.reduce((m, p) => Math.max(m, p.aAU || 0), 0) || 0.5;
      nav.flyTo(() => ({ pos: e.world, radius: Math.max(aMax * AU / 2.2, e.radiusM * 40) }), 0.55);
    } else if (r.glow) {
      selection = { name: r.glow.name, kind: r.glow.kind, world: r.glow.world, radius: r.glow.radius };
      nav.flyTo(() => ({ pos: r.glow.world, radius: r.glow.radius }));
    } else {
      selection = { name: r.name, kind: r.kind, world: r.world, radius: r.radius, body: r };
      nav.flyTo(() => {
        const b = system.bodies().find(x => x.name === r.name) || r;
        return { pos: b.world, radius: b.radius };
      });
    }
    showSelectionCard();
  },
  setRate(r) { clock.rate = r; ui.setRateActive(r); },
  stepDays(d) { clock.jd += d; },
  goNow() { clock.jd = eph.jdTdb(eph.jdUtcNow()); },
  goHome,
});

const nav = new Nav(canvas, () => ui.isTyping && ui.isTyping());

function pushBack() {
  backStack.push({ pos: nav.camWorld.clone(), quat: nav.quat.clone() });
  if (backStack.length > 32) backStack.shift();
}
function goHome() {
  pushBack();
  nav.flyTo(() => {
    const e = system.bodies().find(b => b.name === 'Earth');
    return e ? { pos: e.world, radius: e.radius } : { pos: new THREE.Vector3(AU, 0, 0), radius: 6.371e6 };
  });
}
window.addEventListener('keydown', (e) => {
  if (ui.isTyping && ui.isTyping()) return;
  if (e.code === 'Backspace') {
    const b = backStack.pop();
    if (b) nav.goDirect(b.pos, b.quat);
  } else if (e.code === 'KeyH') goHome();
  else if (e.code === 'KeyT') {
    if (clock.rate !== 0) { clock.prevRate = clock.rate; clock.rate = 0; }
    else clock.rate = clock.prevRate || 60;
    ui.setRateActive(clock.rate);
  } else if (e.code === 'KeyG' && selection) {
    pushBack();
    const sel = selection;
    nav.flyTo(() => ({ pos: liveWorld(sel), radius: sel.radius }));
  }
});

function liveWorld(sel) {
  if (sel.body || ['planet', 'moon', 'star', 'craft'].includes(sel.kind)) {
    const b = system.bodies().find(x => x.name === sel.name);
    if (b) return b.world;
  }
  return sel.world;
}

// ---------- picking ----------
function pickAll(ndc) {
  const w2 = window.innerWidth / 2, h2 = window.innerHeight / 2;
  const v = new THREE.Vector3();
  let best = null, bestScore = 20;
  const consider = (name, kind, world, radius, extra) => {
    v.copy(world).sub(nav.camWorld);
    const dist = v.length();
    v.project(camera);
    if (v.z < -1 || v.z > 1) return;
    const dx = (v.x - ndc[0]) * w2, dy = (v.y - ndc[1]) * h2;
    const appPx = (radius / dist) * h2 / Math.tan(camera.fov * Math.PI / 360);
    const d = Math.max(0, Math.hypot(dx, dy) - Math.min(appPx, 200));
    if (d < bestScore) { bestScore = d; best = { name, kind, world: world.clone(), radius, ...extra }; }
  };
  for (const b of system.bodies()) consider(b.name, b.kind, b.world, b.radius, { body: b });
  for (const it of pocketItems) consider(it.name, it.kind, it.world, it.radius, { pocket: it });
  for (const it of farGlows.items) consider(it.name, it.kind, it.world, it.radius, { glow: it });
  for (const s of named) consider(s.name, 'star', s.world, s.radiusM, { named: s });
  if (!best && starsNear) {
    const hit = pickStar(starsNear, ndc, nav.camWorld, camera, 16);
    if (hit) {
      const e = starEntry(hit.idx);
      best = { name: e.name, kind: 'star', world: e.world, radius: e.radiusM, starIdx: hit.idx };
    }
  }
  return best;
}
nav.onSelect((nx, ny) => {
  const hit = pickAll([nx, ny]);
  if (hit) { selection = hit; showSelectionCard(); }
  else { selection = null; ui.hideCard(); }
});
nav.onTravel((nx, ny) => {
  const hit = pickAll([nx, ny]) || selection;
  if (!hit) return;
  selection = hit;
  pushBack();
  nav.flyTo(() => ({ pos: liveWorld(hit), radius: hit.radius }));
  showSelectionCard();
});

function showSelectionCard() {
  const s = selection;
  if (!s) return;
  if (s.body) { ui.showCard(system.bodyCard(s.body, nav.camWorld, clock.jd)); return; }
  const rows = [];
  if (s.named) {
    rows.push({ label: 'distance', value: `${s.named.distPc.toFixed(2)} pc`, ev: 'gaia-compiled' });
    rows.push({ label: 'V magnitude', value: s.named.vmag.toFixed(2), ev: 'gaia-compiled' });
    rows.push({ label: 'radius (est.)', value: `${(s.named.radiusM / RSUN).toFixed(2)} R☉` });
    ui.showCard({ title: s.name, kind: 'star · compiled catalog', rows, note: 'surface appearance illustrative' });
    return;
  }
  if (s.kind === 'star' && s.starIdx !== undefined && starsNear) {
    const m = starsNear.meta[s.starIdx];
    const phys = starPhysical(s.starIdx);
    rows.push({ label: 'distance', value: `${m.distPc.toFixed(2)} pc (±${(m.distErrPc ?? 0).toFixed(2)})`, ev: 'gaia-dr3' });
    rows.push({ label: 'G magnitude', value: m.gmag.toFixed(2), ev: 'gaia-dr3' });
    rows.push({ label: 'BP−RP', value: (m.bprp ?? 0).toFixed(2), ev: 'gaia-dr3' });
    rows.push({ label: 'radius (est.)', value: `${phys.R.toFixed(2)} R☉`, ev: 'gaia-dr3' });
    ui.showCard({ title: s.name, kind: 'star · Gaia DR3', rows, note: 'surface appearance illustrative' });
  } else if (s.kind === 'exoplanet' && s.pocket) {
    const p = s.pocket.pl;
    if (p.perD) rows.push({ label: 'period', value: `${p.perD.toFixed(2)} d`, ev: 'exo-archive' });
    if (p.aAU) rows.push({ label: 'semi-major axis', value: `${p.aAU.toFixed(3)} AU`, ev: 'exo-archive' });
    if (p.radE) rows.push({ label: 'radius', value: `${p.radE.toFixed(2)} R⊕`, ev: 'exo-archive' });
    if (p.eqt) rows.push({ label: 'eq. temperature', value: `${Math.round(p.eqt)} K`, ev: 'exo-archive' });
    rows.push({ label: 'discovered', value: `${p.discYear || '—'} · ${p.method || ''}`, ev: 'exo-archive' });
    ui.showCard({ title: s.name, kind: `exoplanet · ${s.pocket.host}`, rows, note: 'appearance illustrative; orbit circular approx.' });
  } else if (s.exo) {
    const x = s.exo;
    rows.push({ label: 'distance', value: `${x.distPc.toFixed(2)} pc`, ev: x.gaiaIdx != null ? 'gaia-dr3' : 'exo-archive' });
    if (x.teff) rows.push({ label: 'star T_eff', value: `${Math.round(x.teff)} K`, ev: 'exo-archive' });
    if (x.stRad) rows.push({ label: 'star radius', value: `${x.stRad.toFixed(2)} R☉`, ev: 'exo-archive' });
    rows.push({ label: 'planets', value: `${x.planets.length} confirmed`, ev: 'exo-archive' });
    for (const p of x.planets.slice(0, 7)) {
      rows.push({ label: p.name.replace(x.host, '').trim() || p.name, value: `${p.aAU ? p.aAU.toFixed(3) + ' AU' : '—'} · ${p.radE ? p.radE.toFixed(2) + ' R⊕' : '—'}`, ev: 'exo-archive' });
    }
    ui.showCard({ title: x.host, kind: 'planetary system', rows, note: 'orbits circular approx.; appearance illustrative' });
  } else if (s.kind === 'galaxy' || s.kind === 'node') {
    rows.push({ label: 'range', value: prettyDist(s.world.distanceTo(nav.camWorld)) });
    ui.showCard({ title: s.name, kind: s.kind === 'node' ? 'cosmic-web node' : 'galaxy', rows, note: s.kind === 'node' ? 'filaments procedural' : 'position measured; portrait illustrative', });
  } else {
    rows.push({ label: 'range', value: prettyDist(s.world.distanceTo(nav.camWorld)) });
    ui.showCard({ title: s.name, kind: s.kind, rows });
  }
}

// ---------- date formatting ----------
function fmtDate(jdTdb) {
  const jdUtc = jdTdb - 69.184 / 86400;
  const z = Math.floor(jdUtc + 0.5), f = jdUtc + 0.5 - z;
  let a = z;
  if (z >= 2299161) {
    const al = Math.floor((z - 1867216.25) / 36524.25);
    a = z + 1 + al - Math.floor(al / 4);
  }
  const b = a + 1524, c = Math.floor((b - 122.1) / 365.25), d = Math.floor(365.25 * c), e = Math.floor((b - d) / 30.6001);
  const day = b - d - Math.floor(30.6001 * e);
  const mon = e < 14 ? e - 1 : e - 13;
  const yr = mon > 2 ? c - 4716 : c - 4715;
  const hf = f * 24, hh = Math.floor(hf), mm = Math.floor((hf - hh) * 60);
  const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  return `${yr < 0 ? `${-yr} B.C.` : yr} ${MON[mon - 1]} ${day} ${String(hh).padStart(2, '0')}:${String(mm).padStart(2, '0')} UTC`;
}

// ---------- boot position: Earth overview ----------
{
  const jd = clock.jd;
  const earthIdx = eph.PLANETS.findIndex(p => p.name === 'Earth');
  const ep = eph.planetState(earthIdx >= 0 ? earthIdx : 2, jd).pos;
  const sunward = ep.clone().normalize().negate();   // day side faces the Sun
  const up = new THREE.Vector3(0, 0, 1);
  nav.camWorld.copy(ep).addScaledVector(sunward, 4.2 * 6.371e6).addScaledVector(up, 1.5 * 6.371e6);
  nav.lookAtWorld(ep);
  ui.setRateActive(clock.rate);
}

// ---------- main loop ----------
let pocketItems = [];
let last = performance.now();
let tSec = 0;
function schedule() {
  // rAF stalls in hidden tabs; keep simulation time and headless
  // verification alive with a slow timeout ticker
  if (document.hidden) setTimeout(() => frame(performance.now()), 66);
  else requestAnimationFrame(frame);
}
function frame(now) {
  schedule();
  const dt = Math.min((now - last) / 1000, 0.1);
  last = now;
  tSec += dt;
  clock.jd += clock.rate * dt / 86400;
  maybeFetchStates();

  // nearest surface -> auto flight speed; collision floor
  const near = system.nearestSurface(nav.camWorld);
  let nearDist = near.dist, nearBody = near.body;
  if (pocket.active) {
    const d = pocket.active.world.distanceTo(nav.camWorld) - pocket.active.radiusM;
    if (d < nearDist) { nearDist = d; nearBody = { name: pocket.active.name, world: pocket.active.world, radius: pocket.active.radiusM }; }
  }
  const speed = nav.update(dt, Math.max(nearDist, 0.5));
  if (nearBody) {
    const d = nav.camWorld.distanceTo(nearBody.world);
    const floor = nearBody.radius * 1.015;
    if (d < floor) {
      nav.camWorld.copy(nearBody.world).addScaledVector(nav.camWorld.clone().sub(nearBody.world).normalize(), floor);
    }
  }

  // world update, camera-relative parking
  const pxScale = (window.innerHeight / 2) / Math.tan(camera.fov * Math.PI / 360);
  for (const m of cloudMats) m.uniforms.uPxScale.value = pxScale;
  // cosmic web is a >Mpc-scale layer: gate it so clusters (real angular size
  // ~8 deg but invisible surface brightness) never photobomb nearby scales
  if (webMat) {
    const dMpc = nav.camWorld.length() / (1e3 * KPC);
    const t = Math.min(Math.max((dMpc - 0.8) / 12, 0), 1);
    webMat.uniforms.uB.value = 0.16 * t * t;
  }
  system.setCamQuat(nav.quat);
  system.update(clock.jd, nav.camWorld, tSec);
  camera.quaternion.copy(nav.quat);
  camera.position.set(0, 0, 0);
  camera.updateMatrixWorld();
  starGroup.position.set(-nav.camWorld.x, -nav.camWorld.y, -nav.camWorld.z);
  webGroup.position.copy(starGroup.position);
  farGlows.group.position.copy(starGroup.position);
  mwGroup.position.set(gcWorld.x - nav.camWorld.x, gcWorld.y - nav.camWorld.y, gcWorld.z - nav.camWorld.z);
  if (m31Group && m31World) m31Group.position.set(m31World.x - nav.camWorld.x, m31World.y - nav.camWorld.y, m31World.z - nav.camWorld.z);

  // star pocket activation by proximity
  if (starsNear && nav.camWorld.length() > 0.05 * PC) {
    const ns = nearestCatalogStar(nav.camWorld);
    if (ns && ns.dist < 1.0 * PC) {
      const cur = pocket.active && pocket.active.gaiaIdx;
      if (cur !== ns.idx) activatePocket(starEntry(ns.idx));
    } else if (pocket.active && (!ns || ns.dist > 1.2 * PC)) clearPocket();
  } else if (pocket.active) clearPocket();
  pocketItems = pocketUpdate(clock.jd, nav.camWorld, tSec);

  // labels
  const labels = [];
  const w2 = window.innerWidth / 2, h2 = window.innerHeight / 2;
  const v = new THREE.Vector3();
  const sunDist = nav.camWorld.length();
  const consider = (name, kind, world, radius, minPx, size) => {
    if (labels.length >= 48) return;
    v.copy(world).sub(nav.camWorld);
    const dist = v.length();
    v.project(camera);
    if (v.z < -1 || v.z > 1) return;
    const x = (v.x + 1) * w2, y = (1 - v.y) * h2;
    if (x < -50 || x > 2 * w2 + 50 || y < -50 || y > 2 * h2 + 50) return;
    const appPx = (radius / dist) * h2 / Math.tan(camera.fov * Math.PI / 360);
    labels.push({ text: name, x, y: y - Math.max(appPx * 0.9, 14) - 6, alpha: 0.85, size: size || 'm', kind });
  };
  for (const b of system.bodies()) {
    const d = b.world.distanceTo(nav.camWorld);
    if (b.kind === 'craft') { if (sunDist < 2e13) consider(b.name, 'craft', b.world, 1e9, 0, 's'); continue; }
    if (b.kind === 'moon' && d > 3e11) continue;
    if (d < (b.kind === 'star' ? 1e14 : 5e13)) consider(b.name, 'body', b.world, b.radius, 0, b.kind === 'planet' || b.kind === 'star' ? 'm' : 's');
  }
  for (const it of pocketItems) consider(it.name, it.kind === 'star' ? 'star' : 'body', it.world, it.radius, 0, it.kind === 'star' ? 'l' : 's');
  if (sunDist > 3e14) {
    for (const s of named) consider(s.name, 'star', s.world, 1e10, 0, 's');
  }
  if (sunDist > 30 * KPC) {
    for (const it of farGlows.items) {
      if (it.kind === 'galaxy' || sunDist > 3000 * KPC) consider(it.name, 'galaxy', it.world, it.radius * 0.2, 0, 'm');
    }
  }
  if (selection) {
    const w = liveWorld(selection);
    consider(selection.name, 'body', w, selection.radius, 0, 'l');
  }
  ui.renderLabels(labels);

  // chip
  const focusName = selection ? selection.name : (nearBody ? nearBody.name : '—');
  const focusWorld = selection ? liveWorld(selection) : (nearBody ? nearBody.world : null);
  ui.setChip({
    focus: focusName,
    dist: focusWorld ? prettyDist(Math.max(nav.camWorld.distanceTo(focusWorld) - (selection ? selection.radius : nearBody.radius), 0)) : '',
    speed: speed > 0 ? prettySpeed(speed) : '',
    date: fmtDate(clock.jd),
    src: system.stateSrc(),
  });

  composer.render();
}
schedule();
document.addEventListener('visibilitychange', () => { if (!document.hidden) last = performance.now(); });
// debug/verification handle
window.__pantheon = { renderer, composer, scene, camera, nav, clock, system, goTo: (n, k = 4) => {
  const b = system.bodies().find(x => x.name === n);
  if (!b) return;
  const sunward = b.world.clone().normalize().negate();          // day side
  const side = new THREE.Vector3(0, 0, 1).cross(sunward).normalize();
  nav.camWorld.copy(b.world).addScaledVector(sunward, b.radius * k)
    .addScaledVector(side, b.radius * k * 0.35).addScaledVector(new THREE.Vector3(0, 0, 1), b.radius * k * 0.3);
  nav.lookAtWorld(b.world);
} };
