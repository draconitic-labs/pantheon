// Pantheon 3D: the solar system. Spheres with IAU orientations, single-scattering
// atmospheres, Earth's cloud shell and night side, rings with mutual shadows,
// eclipses from real geometry, the Sun's surface and corona, orbit engravings,
// spacecraft tracks, and flux-faithful points for everything too small to resolve.
//
// All world math is f64 (JS numbers / THREE.Vector3); every group is parked
// camera-relative each frame. Mesh object space IS the body-fixed frame: poles at
// object +Y, prime meridian at object +X, east toward -Z.
import * as THREE from 'three';
import * as eph from '../eph.js';
import { makeStarPoints } from './stars.js';
import {
  BODY_VSH, BODY_FSH, ATMO_FSH, CLOUD_FSH, RING_VSH, RING_FSH, STAR_FSH,
  CORONA_VSH, CORONA_FSH, GLARE_FSH, ORBIT_VSH, ORBIT_FSH,
} from './shaders.js';

const AU = 1.495978707e11;
const PC = 3.0856775814913673e16;
const RSUN = 6.957e8;
const DEG = Math.PI / 180;

// Surface styling. light: 0 lambert, 1 regolith, 2 cloud tops. flat: polar
// flattening. Optical depths (tau, zenith) and scale heights are in body radii;
// heights above Earth are exaggerated about 1.5x so the limb reads at planet scale.
const STYLE = {
  Mercury: { kind: 1, a: '#8a8078', b: '#565049', p: [0, 1.7, 0, 0], light: 1, bump: 0.35 },
  Venus: {
    kind: 1, a: '#e8d5a8', b: '#c9ab74', p: [0, 0.8, 0, 0], light: 2,
    atmo: { Hr: 0.0028, Hm: 0.0028, top: 1.022, tauR: [0.18, 0.28, 0.45], tauM: [0.28, 0.24, 0.15], g: 0.72, e: 14 },
  },
  Earth: {
    kind: 4, a: '#0a1d33', b: '#123252', p: [0, 2, 0, 0], light: 0, bump: 0.22, amb: 0.0012, sunset: [1.0, 0.72, 0.5],
    atmo: { Hr: 0.0019, Hm: 0.00035, top: 1.018, tauR: [0.0464, 0.108, 0.265], tauO: [0.0156, 0.045, 0.002], tauM: [0.03, 0.03, 0.03], g: 0.78, e: 15 },
  },
  Mars: {
    kind: 1, a: '#b06a43', b: '#7a4530', p: [0.9, 1.3, 0, 0], light: 1, bump: 0.3, amb: 0.001, sunset: [1.0, 0.7, 0.55],
    atmo: { Hr: 0.0048, Hm: 0.0048, top: 1.035, tauR: [0.002, 0.004, 0.01], tauM: [0.3, 0.18, 0.09], g: 0.65, e: 14 },
  },
  Jupiter: {
    kind: 2, a: '#d8c0a0', b: '#a07858', p: [0, 1, 9.0, 0.9], light: 2, flat: 0.0649,
    atmo: { Hr: 0.0024, Hm: 0.0024, top: 1.02, tauR: [0.05, 0.08, 0.13], tauM: [0.06, 0.05, 0.035], g: 0.6, e: 10 },
  },
  Saturn: {
    kind: 2, a: '#e0d0a8', b: '#b8a070', p: [0, 1, 7.0, 0], light: 2, flat: 0.0980,
    atmo: { Hr: 0.0024, Hm: 0.0024, top: 1.02, tauR: [0.05, 0.07, 0.1], tauM: [0.06, 0.05, 0.035], g: 0.6, e: 10 },
  },
  Uranus: {
    kind: 3, a: '#9fd4d8', b: '#7fb8c8', p: [0, 1, 6.0, 0], light: 2, flat: 0.0229,
    atmo: { Hr: 0.004, Hm: 0.004, top: 1.03, tauR: [0.06, 0.14, 0.2], tauM: [0.01, 0.015, 0.02], g: 0.6, e: 16 },
  },
  Neptune: {
    kind: 3, a: '#4f78c8', b: '#35529a', p: [0, 1, 6.0, 0], light: 2, flat: 0.0171,
    atmo: { Hr: 0.004, Hm: 0.004, top: 1.03, tauR: [0.05, 0.1, 0.28], tauM: [0.01, 0.015, 0.03], g: 0.6, e: 16 },
  },
  Pluto: { kind: 1, a: '#c8b49a', b: '#8a7460', p: [0.85, 1.2, 0, 0], light: 1 },
  Moon: { kind: 1, a: '#b8b4ac', b: '#6a675f', p: [0, 2.2, 0, 0], light: 1, bump: 0.45, amb: 0.0015 },
  Io: { kind: 1, a: '#d8c060', b: '#a08030', p: [0, 1.8, 0, 0], light: 1 },
  Europa: { kind: 1, a: '#d8cfc0', b: '#a89880', p: [0, 1.1, 0, 0], light: 1 },
  Titan: {
    kind: 1, a: '#c89850', b: '#906830', p: [0, 0.7, 0, 0], light: 2,
    atmo: { Hr: 0.016, Hm: 0.02, top: 1.16, tauR: [0.03, 0.07, 0.17], tauM: [0.75, 0.38, 0.12], g: 0.7, e: 16 },
  },
  Triton: { kind: 1, a: '#d0c8c0', b: '#98908a', p: [0.8, 1.3, 0, 0], light: 1 },
};
const styleFor = (name) => STYLE[name] || { kind: 1, a: '#a89c8c', b: '#6a6055', p: [0, 1.5, 0, 0], light: 1 };

// Absolute magnitude H (1 AU from Sun and observer), for a true naked-eye look.
const ABS_MAG = {
  Mercury: -0.6, Venus: -4.47, Earth: -3.86, Mars: -1.52, Jupiter: -9.4, Saturn: -8.88,
  Uranus: -7.19, Neptune: -6.87, Pluto: -0.44, Moon: 0.21, Titan: -1.2, Io: -1.7, Europa: -1.4,
  Ganymede: -2.1, Callisto: -1.0,
};
const POINT_COLOR = {
  Mercury: [0.8, 0.76, 0.7], Venus: [1.0, 0.95, 0.8], Earth: [0.55, 0.72, 1.0], Mars: [1.0, 0.6, 0.4],
  Jupiter: [1.0, 0.9, 0.75], Saturn: [1.0, 0.94, 0.78], Uranus: [0.7, 0.95, 0.95], Neptune: [0.55, 0.65, 1.0],
  Pluto: [0.85, 0.8, 0.72],
};

const MAPS = {
  Sun: '8k_sun.jpg', Mercury: '8k_mercury.jpg', Venus: '4k_venus_atmosphere.jpg',
  Earth: '8k_earth_daymap.jpg', Moon: '8k_moon.jpg', Mars: '8k_mars.jpg',
  Jupiter: '8k_jupiter.jpg', Saturn: '8k_saturn.jpg',
  Uranus: '2k_uranus.jpg', Neptune: '2k_neptune.jpg',
};

// Domain of the Saturn ring strip, fitted to the known gaps: the Cassini Division
// at u = 0.70 and the C ring's inner edge at u = 0.08 give these radii.
const SATURN_STRIP = { inner: 68824e3, outer: 141754e3 };

// object +Y -> pole, +X -> prime meridian; east ends up on object -Z
function basisToQuatMesh(b) {
  return new THREE.Quaternion().setFromRotationMatrix(new THREE.Matrix4().makeBasis(b.x, b.z, b.x.clone().cross(b.z)));
}
// object +Z -> pole (ring plane in XY)
function basisToQuatRing(b) {
  return new THREE.Quaternion().setFromRotationMatrix(new THREE.Matrix4().makeBasis(b.x, b.z.clone().cross(b.x), b.z));
}

const PLACEHOLDER_TEX = new THREE.DataTexture(new Uint8Array([128, 128, 128, 255]), 1, 1);
PLACEHOLDER_TEX.needsUpdate = true;

export function bodyMaterial(style, seed, quality) {
  const s = style;
  const uniforms = {
    uKind: { value: s.kind }, uLight: { value: s.light || 0 }, uNOcc: { value: 0 },
    uColA: { value: new THREE.Color(s.a) }, uColB: { value: new THREE.Color(s.b) },
    uParam: { value: new THREE.Vector4(...s.p) },
    uSunObj: { value: new THREE.Vector3(0, 0, 1) }, uCamObj: { value: new THREE.Vector3(0, 0, 3) },
    uSunTint: { value: new THREE.Vector3(...(s.sunset || [1, 1, 1])) },
    uTime: { value: 0 }, uSeed: { value: seed }, uClose: { value: 0 },
    uSunI: { value: 2.6 }, uAmb: { value: s.amb ?? 0.0008 }, uFlat: { value: 1 - (s.flat || 0) },
    uBump: { value: s.bump || 0 }, uCloudLon: { value: 0 }, uSunAng: { value: 0.0047 },
    uMap: { value: PLACEHOLDER_TEX }, uNight: { value: PLACEHOLDER_TEX },
    uClouds: { value: PLACEHOLDER_TEX }, uRingMap: { value: PLACEHOLDER_TEX },
    uHasMap: { value: 0 }, uHasNight: { value: 0 }, uHasClouds: { value: 0 }, uHasRing: { value: 0 },
    uRingIn: { value: 1 }, uRingOut: { value: 2 }, uMapTexel: { value: new THREE.Vector2(0.001, 0.001) },
    uOcc: { value: [new THREE.Vector4(), new THREE.Vector4(), new THREE.Vector4(), new THREE.Vector4()] },
  };
  return new THREE.ShaderMaterial({ vertexShader: BODY_VSH, fragmentShader: BODY_FSH, uniforms });
}

const OVER_BLEND = {
  blending: THREE.CustomBlending, blendEquation: THREE.AddEquation,
  blendSrc: THREE.OneFactor, blendDst: THREE.OneMinusSrcAlphaFactor,
  blendSrcAlpha: THREE.OneFactor, blendDstAlpha: THREE.OneMinusSrcAlphaFactor,
  transparent: true, depthWrite: false,
};

export function atmoMaterial(a) {
  const beta = (t, H) => new THREE.Vector3(t[0] / H, t[1] / H, t[2] / H);
  return new THREE.ShaderMaterial({
    vertexShader: BODY_VSH, fragmentShader: ATMO_FSH,
    uniforms: {
      uCamObj: { value: new THREE.Vector3(0, 0, 3) }, uSunObj: { value: new THREE.Vector3(0, 0, 1) },
      uBetaR: { value: beta(a.tauR, a.Hr) }, uBetaO: { value: beta(a.tauO || [0, 0, 0], a.Hr) },
      uBetaM: { value: beta(a.tauM, a.Hm) },
      uHr: { value: a.Hr }, uHm: { value: a.Hm }, uG: { value: a.g }, uTop: { value: a.top },
      uSunE: { value: a.e }, uN: { value: 12 }, uNL: { value: 3 },
    },
    ...OVER_BLEND,
  });
}

// A star as a solid object: surface, corona, and screen-sized glare. Used for the
// Sun and for any catalog star you fly to.
export function makeStarObject(sphereGeo, { tint = [1, 1, 1], intensity = 7.0, span = 9, glare = true } = {}) {
  const group = new THREE.Group();
  const tintV = new THREE.Vector3(...tint);
  const mat = new THREE.ShaderMaterial({
    vertexShader: BODY_VSH, fragmentShader: STAR_FSH,
    uniforms: {
      uCamObj: { value: new THREE.Vector3(0, 0, 3) }, uTint: { value: tintV },
      uMap: { value: PLACEHOLDER_TEX }, uHasMap: { value: 0 },
      uTime: { value: 0 }, uClose: { value: 0 }, uI: { value: intensity },
    },
  });
  const mesh = new THREE.Mesh(sphereGeo, mat);
  group.add(mesh);
  const corona = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), new THREE.ShaderMaterial({
    vertexShader: CORONA_VSH, fragmentShader: CORONA_FSH,
    uniforms: {
      uRight: { value: new THREE.Vector3(1, 0, 0) }, uUp: { value: new THREE.Vector3(0, 1, 0) }, uTint: { value: tintV },
      uSpan: { value: span }, uTime: { value: 0 }, uI: { value: 1 }, uDisc: { value: 1 },
    },
    blending: THREE.AdditiveBlending, transparent: true, depthWrite: false,
  }));
  corona.frustumCulled = false;
  corona.renderOrder = 3;
  group.add(corona);
  let glareMesh = null;
  if (glare) {
    glareMesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), new THREE.ShaderMaterial({
      vertexShader: CORONA_VSH, fragmentShader: GLARE_FSH,
      uniforms: { uTint: { value: tintV }, uI: { value: 0 }, uSpan: { value: 1 } },
      blending: THREE.AdditiveBlending, transparent: true, depthWrite: false, depthTest: true,
    }));
    glareMesh.frustumCulled = false;
    glareMesh.renderOrder = 4;
    group.add(glareMesh);
  }
  return { group, mesh, mat, corona, glare: glareMesh, span, radius: 1, vis: 1, flare: 1 };
}

const _v = new THREE.Vector3();
const _q = new THREE.Quaternion();
const _r = new THREE.Vector3();
const _u = new THREE.Vector3();

// radius and camera drive what a star object shows; called once per frame
export function updateStarObject(s, world, radius, camWorld, camQuat, tSec, pxScale, occlusion = 1) {
  s.group.position.set(world.x - camWorld.x, world.y - camWorld.y, world.z - camWorld.z);
  s.mesh.scale.setScalar(radius);
  const d = world.distanceTo(camWorld);
  const inv = s.mesh.quaternion.clone().invert();
  s.mat.uniforms.uCamObj.value.copy(camWorld).sub(world).divideScalar(radius).applyQuaternion(inv);
  const discPx = radius / Math.max(d, radius) * pxScale;
  s.mat.uniforms.uTime.value = tSec;
  s.mat.uniforms.uClose.value = THREE.MathUtils.smoothstep(discPx, 700, 3200);
  s.corona.quaternion.copy(camQuat);
  s.corona.scale.setScalar(radius * s.span);
  const cu = s.corona.material.uniforms;
  cu.uRight.value.set(1, 0, 0).applyQuaternion(camQuat);
  cu.uUp.value.set(0, 1, 0).applyQuaternion(camQuat);
  cu.uTime.value = tSec;
  s.corona.visible = discPx > 0.7;
  if (s.glare) {
    const dAU = d / AU;
    const px = 340 / Math.pow(1 + dAU * 0.12, 0.6);
    const fadeNear = 1 - THREE.MathUtils.smoothstep(discPx, 26, 130);
    const fadeFar = 1 - THREE.MathUtils.smoothstep(d, 3e15, 3e16);
    s.glare.visible = fadeNear * fadeFar > 0.002;
    s.glare.quaternion.copy(camQuat);
    s.glare.scale.setScalar(px * d / pxScale);
    s.flare += (occlusion - s.flare) * 0.25;
    s.glare.material.uniforms.uI.value = 1.15 * fadeNear * fadeFar * s.flare;
  }
}

function orbitMaterial(color, gain) {
  return new THREE.ShaderMaterial({
    vertexShader: ORBIT_VSH, fragmentShader: ORBIT_FSH,
    uniforms: { uCol: { value: new THREE.Color(color) }, uHead: { value: 0 }, uGain: { value: gain }, uNear: { value: 1e9 } },
    blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
  });
}

function orbitLine(samples, color, gain = 1) {
  const n = samples.length;
  const pos = new Float32Array(n * 3), tt = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    pos[3 * i] = samples[i].x; pos[3 * i + 1] = samples[i].y; pos[3 * i + 2] = samples[i].z;
    tt[i] = i / (n - 1);
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('t', new THREE.BufferAttribute(tt, 1));
  const line = new THREE.Line(g, orbitMaterial(color, gain));
  line.frustumCulled = false;
  line.renderOrder = -4;
  return line;
}

function pointInRings(lon, lat, rings) {
  let inside = false;
  for (const ring of rings) {
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      const [xi, yi] = ring[i], [xj, yj] = ring[j];
      if (((yi > lat) !== (yj > lat)) && (lon < (xj - xi) * (lat - yi) / (yj - yi) + xi)) inside = !inside;
    }
  }
  return inside;
}

// object space: +Y pole, +X prime meridian, east toward -Z (right-handed)
function lonLatToUnit(lonDeg, latDeg) {
  const lon = lonDeg * DEG, lat = latDeg * DEG;
  return new THREE.Vector3(Math.cos(lat) * Math.cos(lon), Math.sin(lat), -Math.cos(lat) * Math.sin(lon));
}

// Ring strip when there is no imagery: painted from the published zones.
function zoneStrip(zones, inner, outer) {
  const W = 1024;
  const d = new Uint8Array(W * 4);
  for (let i = 0; i < W; i++) {
    const r = inner + (outer - inner) * (i + 0.5) / W;
    let a = 0;
    for (const z of zones) {
      if (r >= z.rIn && r <= z.rOut) {
        const w = (z.rOut - z.rIn) * 0.1;
        a = Math.max(a, z.density * 0.85 * Math.min(1, (r - z.rIn) / w, (z.rOut - r) / w));
      }
    }
    d[4 * i] = 160; d[4 * i + 1] = 148; d[4 * i + 2] = 128;
    d[4 * i + 3] = Math.round(255 * Math.min(a * (0.82 + 0.18 * Math.sin(i * 0.37) * Math.sin(i * 0.11)), 1));
  }
  const t = new THREE.DataTexture(d, W, 1);
  t.magFilter = THREE.LinearFilter;
  t.minFilter = THREE.LinearFilter;
  t.needsUpdate = true;
  return t;
}

export class SolarSystem {
  constructor(scene, textures) {
    this.scene = scene;
    this.textures = textures;
    this.helio = new THREE.Group();            // heliocentric static ink (orbits, trails)
    scene.add(this.helio);
    this.sphereHi = new THREE.SphereGeometry(1, 256, 160);
    this.sphereLo = new THREE.SphereGeometry(1, 96, 64);
    this.bodiesList = [];
    this.groups = new Map();                   // name -> record
    this.moonOrbitGroups = new Map();          // planet name -> {group, a}
    this.orbitLines = new Map();               // name -> {line, jd0, period}
    this.tracks = [];
    this.stateCache = null;
    this.fetchedOk = false;
    this.quality = { atmo: [12, 3] };
    this._buildSun();
    for (let i = 0; i < eph.PLANETS.length; i++) this._buildPlanet(i);
    this._buildMoons();
    this._buildPoints();
    this.updatePositions(eph.jdTdb(eph.jdUtcNow()));
  }

  setQuality(q) {
    this.quality = q;
    for (const rec of this.groups.values()) {
      if (rec.atmoMat) { rec.atmoMat.uniforms.uN.value = q.atmo[0]; rec.atmoMat.uniforms.uNL.value = q.atmo[1]; }
    }
  }

  _mkBody(name, radius, seed, geo) {
    const group = new THREE.Group();
    const style = styleFor(name);
    const mat = bodyMaterial(style, seed);
    const mesh = new THREE.Mesh(geo, mat);
    const flat = style.flat || 0;
    mesh.scale.set(radius, radius * (1 - flat), radius);
    group.add(mesh);
    const rec = { name, group, mesh, mat, atmoMat: null, radius, world: new THREE.Vector3(), style, flat, textured: false, wantTex: false };
    if (style.atmo) {
      rec.atmoMat = atmoMaterial(style.atmo);
      const shell = new THREE.Mesh(this.sphereLo, rec.atmoMat);
      shell.scale.set(radius * style.atmo.top, radius * style.atmo.top * (1 - flat), radius * style.atmo.top);
      shell.renderOrder = 2;
      rec.shell = shell;
      group.add(shell);
    }
    this.scene.add(group);
    this.groups.set(name, rec);
    return rec;
  }

  _buildSun() {
    const star = makeStarObject(this.sphereHi, { tint: [1.0, 0.9, 0.72], intensity: 2.6 });
    this.scene.add(star.group);
    this.sun = star;
    const rec = {
      name: 'Sun', group: star.group, mesh: star.mesh, mat: star.mat, radius: RSUN,
      world: new THREE.Vector3(), style: { kind: 0 }, flat: 0, textured: false, wantTex: false, isStar: true,
    };
    this.groups.set('Sun', rec);
  }

  _buildPlanet(i) {
    const p = eph.PLANETS[i];
    const rec = this._mkBody(p.name, p.radius, i * 3.7 + 2, this.sphereHi);
    rec.planetIdx = i;
    const per = p.period || 365.25 * Math.pow(p.a || 1, 1.5);
    rec.period = per;
    const jd0 = eph.jdTdb(eph.jdUtcNow());
    const N = 720, samples = [];
    for (let k = 0; k < N; k++) samples.push(eph.planetState(i, jd0 + per * k / N).pos);
    samples.push(samples[0].clone());
    rec.orbitA = samples.reduce((m, s) => Math.max(m, s.length()), 0);
    const line = orbitLine(samples, '#d8b36a', 0.9);
    this.helio.add(line);
    this.orbitLines.set(p.name, { line, jd0, period: per });
    this._buildRings(rec, i);
  }

  _buildRings(rec, planetIdx) {
    const entry = (eph.RINGS || []).find(r => r.planet === planetIdx);
    if (!entry || !entry.zones.length) return;
    const R = rec.radius;
    const isSaturn = rec.name === 'Saturn';
    const inner = isSaturn ? SATURN_STRIP.inner : Math.min(...entry.zones.map(z => z.rIn));
    const outer = isSaturn ? SATURN_STRIP.outer : Math.max(...entry.zones.map(z => z.rOut));
    const strip = zoneStrip(entry.zones, inner, outer);
    const geo = new THREE.RingGeometry(inner / R, outer / R, 512, 1);
    const mat = new THREE.ShaderMaterial({
      vertexShader: RING_VSH, fragmentShader: RING_FSH,
      uniforms: {
        uSunObj: { value: new THREE.Vector3(0, 0, 1) }, uCamObj: { value: new THREE.Vector3(0, 0, 3) },
        uMap: { value: strip }, uInner: { value: inner / R }, uOuter: { value: outer / R },
        uSunI: { value: 2 }, uFlat: { value: 1 - rec.flat }, uClose: { value: 0 },
      },
      ...OVER_BLEND, side: THREE.DoubleSide,
    });
    rec.ring = new THREE.Mesh(geo, mat);
    rec.ring.scale.setScalar(R);
    rec.ring.renderOrder = 1;
    rec.ringOuter = outer;
    rec.group.add(rec.ring);
    if (isSaturn) {
      const mu = rec.mat.uniforms;
      mu.uRingMap.value = strip;
      mu.uHasRing.value = 1;
      mu.uRingIn.value = inner / R;
      mu.uRingOut.value = outer / R;
    }
  }

  _buildMoons() {
    const jd0 = eph.jdTdb(eph.jdUtcNow());
    for (let mi = 0; mi < eph.MOONS.length; mi++) {
      const m = eph.MOONS[mi];
      const rec = this._mkBody(m.name, m.radius, 40 + mi * 1.31, this.sphereLo);
      rec.moonIdx = mi;
      rec.parent = eph.PLANETS[m.parent].name;
      let og = this.moonOrbitGroups.get(rec.parent);
      if (!og) {
        og = { group: new THREE.Group(), a: 0 };
        this.scene.add(og.group);
        this.moonOrbitGroups.set(rec.parent, og);
      }
      const N = 256, samples = [];
      for (let k = 0; k < N; k++) samples.push(eph.moonRel(mi, jd0 + m.period * k / N));
      samples.push(samples[0].clone());
      const a = samples[0].length();
      rec.orbitA = a;
      og.a = Math.max(og.a, a);
      const line = orbitLine(samples, '#9ab8d8', 0.8);
      og.group.add(line);
      this.orbitLines.set(m.name, { line, jd0, period: m.period, a });
    }
  }

  // Everything too small to resolve still shows, at its true apparent magnitude.
  _buildPoints() {
    const names = ['Sun', ...eph.PLANETS.map(p => p.name), ...eph.MOONS.map(m => m.name)];
    const n = names.length;
    const pos = new Float32Array(n * 3), col = new Float32Array(n * 3);
    const meta = [];
    for (let i = 0; i < n; i++) {
      col.set(POINT_COLOR[names[i]] || (names[i] === 'Sun' ? [1, 0.95, 0.85] : [0.9, 0.88, 0.8]), 3 * i);
      meta.push({ gmag: 0, distPc: 10 });
    }
    this.pointNames = names;
    const pts = makeStarPoints({ count: n, positions: pos, colors: col, meta }, { perVertexVis: true });
    this.points = pts;
    this.pointVis = new Float32Array(n);
    pts.geometry.setAttribute('vis', new THREE.BufferAttribute(this.pointVis, 1));
    pts.renderOrder = -5;
    this.pointGroup = new THREE.Group();
    this.pointGroup.add(pts);
    this.scene.add(this.pointGroup);
  }

  // ---- imagery -------------------------------------------------------------
  // Each body asks for its maps as you approach; the place you fly to asks first.
  wantTextures(name, urgent = false) {
    const rec = this.groups.get(name);
    const bank = this.textures;
    if (!rec || !bank) return;
    if (rec.wantTex) {
      if (urgent && MAPS[name]) bank.prioritize(MAPS[name]);
      return;
    }
    rec.wantTex = true;
    const file = MAPS[name];
    if (file) {
      bank.request(file, (t) => {
        rec.mat.uniforms.uMap.value = t;
        rec.mat.uniforms.uHasMap.value = 1;
        if (rec.mat.uniforms.uMapTexel) rec.mat.uniforms.uMapTexel.value.set(1 / t.userData.size[0], 1 / t.userData.size[1]);
        rec.textured = true;
      }, { urgent });
    }
    if (name === 'Earth') {
      bank.request('8k_earth_nightmap.jpg', (t) => {
        rec.mat.uniforms.uNight.value = t;
        rec.mat.uniforms.uHasNight.value = 1;
        rec.nightReal = true;
        if (rec.lightsObj) rec.lightsObj.visible = false;
      }, { urgent });
      bank.request('2k_earth_clouds.jpg', (t) => {
        rec.mat.uniforms.uClouds.value = t;
        rec.mat.uniforms.uHasClouds.value = 1;
        rec.cloudsReal = true;
        this._addClouds(rec, t);
      }, { urgent, srgb: false });
    }
    if (name === 'Saturn' && rec.ring && bank.has('8k_saturn_ring_alpha.png')) {
      bank.request('8k_saturn_ring_alpha.png', (t) => {
        rec.ring.material.uniforms.uMap.value = t;
        rec.mat.uniforms.uRingMap.value = t;
        rec.ringReal = true;
      }, { urgent, wrap: false });
    }
  }

  _addClouds(earth, tex) {
    const mat = new THREE.ShaderMaterial({
      vertexShader: BODY_VSH, fragmentShader: CLOUD_FSH,
      uniforms: {
        uSunObj: { value: new THREE.Vector3(0, 0, 1) }, uClouds: { value: tex },
        uCloudLon: { value: 0 }, uSunI: { value: 2.6 }, uClose: { value: 0 },
      },
      ...OVER_BLEND,
    });
    const shell = new THREE.Mesh(this.sphereHi, mat);
    shell.scale.setScalar(earth.radius * 1.0045);
    shell.renderOrder = 1;
    earth.cloudMat = mat;
    earth.cloudShell = shell;
    earth.group.add(shell);
  }

  // Earth's fresh-clone fallback: Natural Earth land on a painted globe, plus
  // vector coasts and a synthetic settlement prior for the night side.
  addEarthExtras(coastRings, landRings, rand) {
    const earth = this.groups.get('Earth');
    if (!earth || earth.coastAdded) return;
    earth.coastAdded = true;
    if (!earth.textured && !this.textures.has(MAPS.Earth) && landRings && landRings.length) {
      const c = document.createElement('canvas');
      c.width = 2048; c.height = 1024;
      const g = c.getContext('2d');
      g.fillStyle = '#10335c';
      g.fillRect(0, 0, 2048, 1024);
      const grad = g.createLinearGradient(0, 0, 0, 1024);
      grad.addColorStop(0.0, '#dfe6ea'); grad.addColorStop(0.14, '#c7cec9'); grad.addColorStop(0.24, '#5a6f47');
      grad.addColorStop(0.38, '#7b7a4c'); grad.addColorStop(0.46, '#a59463'); grad.addColorStop(0.54, '#4d6a38');
      grad.addColorStop(0.68, '#7e7a50'); grad.addColorStop(0.82, '#9aa79c'); grad.addColorStop(0.9, '#e4e9ec'); grad.addColorStop(1, '#eef1f3');
      g.fillStyle = grad;
      g.beginPath();
      for (const ring of landRings) {
        ring.forEach(([lon, lat], i) => {
          const x = (lon + 180) / 360 * 2048, y = (90 - lat) / 180 * 1024;
          if (i) g.lineTo(x, y); else g.moveTo(x, y);
        });
        g.closePath();
      }
      g.fill('evenodd');
      const t = new THREE.CanvasTexture(c);
      t.colorSpace = THREE.SRGBColorSpace;
      t.wrapS = THREE.RepeatWrapping;
      t.anisotropy = 8;
      earth.mat.uniforms.uMap.value = t;
      earth.mat.uniforms.uHasMap.value = 1;
      earth.mat.uniforms.uMapTexel.value.set(1 / 2048, 1 / 1024);
      earth.mat.uniforms.uBump.value = 0;
      earth.paintedLand = true;
    }
    const verts = [];
    for (const ring of coastRings) {
      for (let i = 0; i + 1 < ring.length; i++) {
        const a = lonLatToUnit(ring[i][0], ring[i][1]).multiplyScalar(1.0015);
        const b = lonLatToUnit(ring[i + 1][0], ring[i + 1][1]).multiplyScalar(1.0015);
        verts.push(a.x, a.y, a.z, b.x, b.y, b.z);
      }
    }
    const cg = new THREE.BufferGeometry();
    cg.setAttribute('position', new THREE.BufferAttribute(new Float32Array(verts), 3));
    const cmat = new THREE.ShaderMaterial({
      vertexShader: /* glsl */`
        #include <common>
        #include <logdepthbuf_pars_vertex>
        void main() {
          gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
          #include <logdepthbuf_vertex>
        }`,
      fragmentShader: /* glsl */`
        #include <common>
        #include <logdepthbuf_pars_fragment>
        void main() {
          #include <logdepthbuf_fragment>
          gl_FragColor = vec4(vec3(0.85, 0.72, 0.45) * 0.4, 1.0);
        }`,
      blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
    });
    const coast = new THREE.LineSegments(cg, cmat);
    coast.frustumCulled = false;
    coast.visible = !!earth.paintedLand;
    earth.mesh.add(coast);
    if (earth.paintedLand && landRings && landRings.length) {
      const pts = [];
      let tries = 0;
      while (pts.length < 1600 * 3 && tries++ < 60000) {
        const lon = rand() * 360 - 180;
        const lat = (Math.asin(rand() * 2 - 1) * 180 / Math.PI) * 0.72;
        if (pointInRings(lon, lat, landRings)) {
          const v = lonLatToUnit(lon, lat).multiplyScalar(1.002);
          pts.push(v.x, v.y, v.z);
        }
      }
      const lg = new THREE.BufferGeometry();
      lg.setAttribute('position', new THREE.BufferAttribute(new Float32Array(pts), 3));
      earth.lightsMat = new THREE.ShaderMaterial({
        vertexShader: /* glsl */`
          varying float vGate;
          uniform vec3 uSunObj;
          #include <common>
          #include <logdepthbuf_pars_vertex>
          void main() {
            vGate = smoothstep(0.02, -0.12, dot(normalize(position), uSunObj));
            gl_PointSize = 1.8;
            gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
            #include <logdepthbuf_vertex>
          }`,
        fragmentShader: /* glsl */`
          varying float vGate;
          #include <common>
          #include <logdepthbuf_pars_fragment>
          void main() {
            #include <logdepthbuf_fragment>
            vec2 pc = gl_PointCoord * 2.0 - 1.0;
            float d2 = dot(pc, pc);
            if (d2 > 1.0) discard;
            gl_FragColor = vec4(vec3(1.0, 0.82, 0.58) * exp(-d2 * 3.0) * vGate * 1.6, 1.0);
          }`,
        uniforms: { uSunObj: { value: new THREE.Vector3(0, 0, 1) } },
        blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
      });
      const lights = new THREE.Points(lg, earth.lightsMat);
      lights.frustumCulled = false;
      earth.lightsObj = lights;
      earth.mesh.add(lights);
    }
  }

  setTracks(tracks) {
    this.tracks = [];
    for (const tr of tracks || []) {
      const n = tr.samples.length;
      if (n < 2) continue;
      const pos = new Float32Array(n * 3), tt = new Float32Array(n);
      for (let i = 0; i < n; i++) {
        pos[3 * i] = tr.samples[i][1]; pos[3 * i + 1] = tr.samples[i][2]; pos[3 * i + 2] = tr.samples[i][3];
        tt[i] = i / (n - 1);
      }
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
      g.setAttribute('t', new THREE.BufferAttribute(tt, 1));
      const line = new THREE.Line(g, orbitMaterial('#7ea8d8', 0.8));
      line.material.uniforms.uHead.value = 1.0;
      line.frustumCulled = false;
      line.renderOrder = -4;
      this.helio.add(line);
      this.tracks.push({ name: tr.name, note: tr.note, samples: tr.samples, line, world: new THREE.Vector3() });
    }
    this.updatePositions(this.lastJd ?? eph.jdTdb(eph.jdUtcNow()));
  }

  // position along a Horizons arc: linear in the cached samples, clamped at the ends
  _trackAt(tr, jd) {
    const s = tr.samples;
    let lo = 0, hi = s.length - 1;
    if (jd <= s[0][0]) { hi = 0; } else if (jd >= s[hi][0]) { lo = hi; } else {
      while (hi - lo > 1) { const m = (lo + hi) >> 1; if (s[m][0] <= jd) lo = m; else hi = m; }
    }
    if (lo === hi) { tr.world.set(s[lo][1], s[lo][2], s[lo][3]); tr.outside = jd < s[0][0] || jd > s[s.length - 1][0]; return; }
    const f = (jd - s[lo][0]) / (s[hi][0] - s[lo][0]);
    tr.world.set(s[lo][1] + (s[hi][1] - s[lo][1]) * f, s[lo][2] + (s[hi][2] - s[lo][2]) * f, s[lo][3] + (s[hi][3] - s[lo][3]) * f);
    tr.outside = false;
  }

  setStates(states) {
    this.stateCache = states;
    if (states) {
      this._stateIdx = new Map();
      for (const b of states.bodies || []) this._stateIdx.set('p:' + b.name, b);
      for (const m of states.moons || []) this._stateIdx.set('m:' + m.name, m);
    }
  }

  // SPICE state carried forward along its instantaneous circular arc: stays on the
  // orbit at any time rate, where straight-line extrapolation made moons jump.
  _statePos(kind, name, idx, jd) {
    const c = this.stateCache;
    if (c && this._stateIdx) {
      const e = this._stateIdx.get((kind === 'planet' ? 'p:' : 'm:') + name);
      const dj = jd - c.jd;
      if (e && Math.abs(dj) < 12) {
        _r.set(e.pos[0], e.pos[1], e.pos[2]);
        _v.set(e.vel[0], e.vel[1], e.vel[2]);
        const r2 = _r.lengthSq();
        if (r2 > 0) {
          _u.crossVectors(_r, _v);
          const h = _u.length();
          if (h > 0) {
            _u.divideScalar(h);
            _q.setFromAxisAngle(_u, (h / r2) * dj);
            return _r.clone().applyQuaternion(_q);
          }
        }
        return _r.clone().addScaledVector(_v, dj);
      }
    }
    return kind === 'planet' ? eph.planetState(idx, jd).pos : eph.moonRel(idx, jd);
  }

  // true while the drawn positions rest on kernel states rather than mean elements
  usingKernels(jd) {
    const c = this.stateCache;
    return !!(c && c.src === 'spice' && Math.abs(jd - c.jd) < 12);
  }

  // the same question for one moon: no kernel is downloaded for the moons of Uranus
  moonUsesKernel(name, jd) {
    const c = this.stateCache;
    const e = this._stateIdx && this._stateIdx.get('m:' + name);
    return !!(c && e && e.src === 'spice' && Math.abs(jd - c.jd) < 12);
  }

  stateSrc(jd) {
    return this.usingKernels(jd ?? this.lastJd) ? 'DE440s, and satellite kernels where loaded' : 'mean elements';
  }

  // Moves every body to jd. Runs before the camera so frame-riding never lags.
  updatePositions(jd) {
    this.lastJd = jd;
    this.bodiesList.length = 0;
    const sun = this.groups.get('Sun');
    sun.world.set(0, 0, 0);
    this.bodiesList.push({ name: 'Sun', kind: 'star', world: sun.world, radius: sun.radius });
    for (let i = 0; i < eph.PLANETS.length; i++) {
      const p = eph.PLANETS[i];
      const rec = this.groups.get(p.name);
      rec.world.copy(this._statePos('planet', p.name, i, jd));
      this.bodiesList.push({ name: p.name, kind: 'planet', world: rec.world, radius: rec.radius, idx: i, frame: rec.ringOuter || rec.radius });
    }
    for (let mi = 0; mi < eph.MOONS.length; mi++) {
      const m = eph.MOONS[mi];
      const rec = this.groups.get(m.name);
      const parent = this.groups.get(rec.parent);
      rec.world.copy(parent.world).add(this._statePos('moon', m.name, mi, jd));
      this.bodiesList.push({ name: m.name, kind: 'moon', world: rec.world, radius: rec.radius, moonIdx: mi, parent: rec.parent });
    }
    for (const tr of this.tracks) {
      this._trackAt(tr, jd);
      this.bodiesList.push({ name: tr.name, kind: 'craft', world: tr.world, radius: 10, frame: 2e9, note: tr.note, outside: tr.outside });
    }
  }

  // Everything that depends on the camera. pxScale: pixels per unit tangent.
  updateRender(jd, camWorld, camQuat, tSec, pxScale, exposureHint) {
    const cloudLon = (jd * 0.012) % 1;
    const park = (g, w) => g.position.set(w.x - camWorld.x, w.y - camWorld.y, w.z - camWorld.z);
    const sunRec = this.groups.get('Sun');
    this.helio.position.set(-camWorld.x, -camWorld.y, -camWorld.z);
    this.pointGroup.position.copy(this.helio.position);

    // how zoomed out are we? orbit ink belongs to the system view, not to close-ups
    let nearAlt = Infinity, nearR = 1;
    for (const b of this.bodiesList) {
      if (b.kind === 'craft') continue;
      const alt = b.world.distanceTo(camWorld) - b.radius;
      if (alt / b.radius < nearAlt / nearR) { nearAlt = alt; nearR = b.radius; }
    }
    const systemView = THREE.MathUtils.smoothstep(Math.log10(Math.max(nearAlt / nearR, 1e-3)), 1.5, 2.5);
    const dSun = camWorld.length();

    // Sun occlusion: any resolved body in front of it hides the glare
    let sunVis = 1;
    {
      const toSun = _u.copy(camWorld).negate();
      const dS = toSun.length() || 1;
      toSun.divideScalar(dS);
      const sunAng = sunRec.radius / dS;
      for (const b of this.bodiesList) {
        if (b.kind === 'craft' || b.kind === 'star') continue;
        _v.copy(b.world).sub(camWorld);
        const db = _v.length();
        if (db >= dS) continue;
        const ca = _v.dot(toSun) / db;
        if (ca < 0.5) continue;
        const sep = Math.acos(Math.min(ca, 1));
        const ang = Math.asin(Math.min(b.radius / db, 1));
        sunVis = Math.min(sunVis, THREE.MathUtils.smoothstep(sep, ang - sunAng * 0.3, ang + sunAng * 1.3));
      }
    }
    sunRec.occlusion = sunVis;
    updateStarObject(this.sun, sunRec.world, sunRec.radius, camWorld, camQuat, tSec, pxScale, sunVis);

    // planets and moons
    const pvis = this.pointVis;
    const pgeo = this.points.geometry;
    const ppos = pgeo.getAttribute('position');
    const plum = pgeo.getAttribute('lum');
    const pc = PC;
    for (let i = 0; i < this.pointNames.length; i++) {
      const name = this.pointNames[i];
      const rec = this.groups.get(name);
      ppos.setXYZ(i, rec.world.x, rec.world.y, rec.world.z);
      const d = rec.world.distanceTo(camWorld);
      const discPx = rec.radius / Math.max(d, 1) * pxScale;
      if (name === 'Sun') {
        plum.setX(i, Math.pow(10, -0.4 * 4.67) );
        pvis[i] = THREE.MathUtils.smoothstep(d, 3e14, 3e15);
        continue;
      }
      const rAU = Math.max(rec.world.length() / AU, 0.05);
      const H = ABS_MAG[name] ?? 2.5 - 5 * Math.log10(Math.max(rec.radius, 1e3) / 1e6);
      plum.setX(i, Math.pow(10, -0.4 * H) * 2.35e-9 / (rAU * rAU));
      pvis[i] = 1 - THREE.MathUtils.smoothstep(discPx, 1.2, 4.0);
      const vis = discPx > 0.4;
      rec.group.visible = vis;
      if (discPx > 0.25) this.wantTexturesWhenClose(rec, discPx);
      if (vis) this._lightUniforms(rec, camWorld, dSun, tSec, cloudLon);
    }
    ppos.needsUpdate = true; plum.needsUpdate = true;
    pgeo.getAttribute('vis').needsUpdate = true;

    // orbit ink: fades in as you zoom out, fades out again far beyond the system
    for (let i = 0; i < eph.PLANETS.length; i++) {
      const p = eph.PLANETS[i];
      const rec = this.groups.get(p.name);
      const ol = this.orbitLines.get(p.name);
      const far = 1 - THREE.MathUtils.smoothstep(Math.log10(Math.max(dSun, 1) / rec.orbitA), 1.6, 2.6);
      // an orbit you are sitting on is a bundle of near-parallel lines, not a shape
      const onIt = THREE.MathUtils.smoothstep(Math.abs(dSun - rec.orbitA) / rec.orbitA, 0.1, 0.6);
      const gain = systemView * far * Math.max(onIt, THREE.MathUtils.smoothstep(Math.abs(camWorld.z) / rec.orbitA, 0.15, 0.6));
      ol.line.visible = gain > 0.003;
      if (ol.line.visible) {
        const u = ol.line.material.uniforms;
        u.uGain.value = 0.9 * gain;
        u.uHead.value = (((jd - ol.jd0) / ol.period) % 1 + 1) % 1;
        u.uNear.value = rec.orbitA * 0.05;
      }
      const og = this.moonOrbitGroups.get(p.name);
      if (og) {
        park(og.group, rec.world);
        const d = rec.world.distanceTo(camWorld);
        const show = THREE.MathUtils.smoothstep(d / og.a, 0.05, 0.4) * (1 - THREE.MathUtils.smoothstep(d / og.a, 8, 40));
        og.group.visible = show > 0.003;
        if (og.group.visible) {
          for (const line of og.group.children) {
            line.material.uniforms.uGain.value = 0.55 * show;
            line.material.uniforms.uNear.value = og.a * 0.02;
          }
        }
      }
    }
    for (const m of eph.MOONS) {
      const ol = this.orbitLines.get(m.name);
      ol.line.material.uniforms.uHead.value = (((jd - ol.jd0) / ol.period) % 1 + 1) % 1;
    }
    for (const tr of this.tracks) {
      tr.line.visible = systemView > 0.02;
      const u = tr.line.material.uniforms;
      u.uGain.value = 0.32 * systemView;
      u.uNear.value = 1e10;
    }
    return { systemView };
  }

  wantTexturesWhenClose(rec, discPx) {
    if (!rec.wantTex && discPx > 6) this.wantTextures(rec.name);
  }

  _lightUniforms(rec, camWorld, dSun, tSec, cloudLon) {
    const u = rec.mat.uniforms;
    if (rec.moonIdx !== undefined) rec.mesh.quaternion.copy(basisToQuatMesh(eph.moonBasis(rec.moonIdx, this.lastJd)));
    else rec.mesh.quaternion.copy(basisToQuatMesh(eph.bodyBasis(rec.planetIdx, this.lastJd)));
    rec.group.position.set(rec.world.x - camWorld.x, rec.world.y - camWorld.y, rec.world.z - camWorld.z);
    if (rec.shell) rec.shell.quaternion.copy(rec.mesh.quaternion);
    const inv = _q.copy(rec.mesh.quaternion).invert();
    const rSun = Math.max(rec.world.length(), 1);
    const sunDir = _u.copy(rec.world).divideScalar(-rSun);
    const so = _v.copy(sunDir).applyQuaternion(inv);
    u.uSunObj.value.copy(so);
    const camObj = _r.copy(camWorld).sub(rec.world).divideScalar(rec.radius).applyQuaternion(inv);
    u.uCamObj.value.copy(camObj);
    u.uTime.value = tSec;
    const dist = rec.world.distanceTo(camWorld);
    const alt = dist / rec.radius - 1;
    u.uClose.value = 1 - THREE.MathUtils.smoothstep(alt, 0.02, 0.45);
    // sunlight falls with distance, compressed as an eye adapting to the dark would
    const sunI = 2.7 * Math.pow(AU / rSun, 0.42);
    u.uSunI.value = sunI;
    u.uSunAng.value = RSUN / rSun;
    u.uCloudLon.value = cloudLon;
    if (rec.atmoMat) {
      const au = rec.atmoMat.uniforms;
      au.uSunObj.value.copy(so);
      au.uCamObj.value.copy(camObj);
      au.uSunE.value = rec.style.atmo.e * (sunI / 2.7);
      // outside the shell draw the near face, depth tested; inside, the far face
      const inside = camObj.length() < rec.style.atmo.top * 1.002;
      rec.atmoMat.side = inside ? THREE.BackSide : THREE.FrontSide;
      rec.atmoMat.depthTest = !inside;
      if (rec.atmoMat.userData.inside !== inside) { rec.atmoMat.userData.inside = inside; rec.atmoMat.needsUpdate = true; }
    }
    if (rec.cloudMat) {
      const cu = rec.cloudMat.uniforms;
      cu.uSunObj.value.copy(so);
      cu.uSunI.value = sunI;
      cu.uCloudLon.value = cloudLon;
      cu.uClose.value = u.uClose.value;
      rec.cloudShell.quaternion.copy(rec.mesh.quaternion);
      // from below the deck the shell would draw its back face: never needed here
    }
    if (rec.lightsMat) rec.lightsMat.uniforms.uSunObj.value.copy(so);
    if (rec.ring) {
      rec.ring.quaternion.copy(basisToQuatRing(eph.bodyBasis(rec.planetIdx, this.lastJd)));
      const invR = _q.copy(rec.ring.quaternion).invert();
      const ru = rec.ring.material.uniforms;
      ru.uSunObj.value.copy(sunDir).applyQuaternion(invR);
      ru.uCamObj.value.copy(camWorld).sub(rec.world).divideScalar(rec.radius).applyQuaternion(invR);
      ru.uSunI.value = sunI;
      ru.uClose.value = 1 - THREE.MathUtils.smoothstep(dist / rec.radius, 2.0, 8.0);
    }
    this._eclipsers(rec, inv);
  }

  // up to four bodies that could shadow this one: its moons, or its parent + siblings
  _eclipsers(rec, inv) {
    const u = rec.mat.uniforms;
    const out = u.uOcc.value;
    let n = 0;
    const consider = (other) => {
      if (n >= 4 || !other || other === rec) return;
      const v = _r.copy(other.world).sub(rec.world);
      const toSun = v.dot(_u.copy(rec.world).negate().normalize());
      if (toSun <= 0) return;   // only something between us and the Sun casts a shadow here
      v.divideScalar(rec.radius).applyQuaternion(inv);
      out[n++].set(v.x, v.y, v.z, other.radius / rec.radius);
    };
    if (rec.moonIdx !== undefined) {
      const parent = this.groups.get(rec.parent);
      consider(parent);
      for (const r of this.groups.values()) if (r.parent === rec.parent) consider(r);
    } else {
      for (const r of this.groups.values()) if (r.parent === rec.name) consider(r);
    }
    u.uNOcc.value = n;
  }

  bodies() { return this.bodiesList; }
  body(name) { return this.bodiesList.find(b => b.name === name) || null; }

  nearestSurface(camWorld) {
    let best = Infinity, bestB = null;
    for (const b of this.bodiesList) {
      if (b.kind === 'craft') continue;
      const d = b.world.distanceTo(camWorld) - b.radius;
      if (d < best) { best = d; bestB = b; }
    }
    return { dist: Math.max(best, 0.1), body: bestB };
  }

  // the facts and their provenance; every row carries an evidence key
  bodyCard(b, camWorld, jd) {
    if (b.kind === 'craft') {
      return {
        title: b.name, kind: 'Spacecraft, not to scale',
        rows: [
          { label: 'From the Sun', value: `${(b.world.length() / AU).toFixed(2)} AU`, ev: 'horizons-track' },
          { label: 'Distance', value: prettyRange(b.world.distanceTo(camWorld)), live: 'range' },
        ],
        note: b.outside ? 'The cached arc does not cover this date, so the marker holds at the end of its track.' : 'Position interpolated between cached Horizons samples.',
      };
    }
    const rows = [];
    const isLuna = b.name === 'Moon';
    const moonKernel = b.kind === 'moon' && this.moonUsesKernel(b.name, jd);
    const evState = b.kind === 'moon'
      ? (moonKernel ? (isLuna ? 'de440' : 'satellite-spk') : (isLuna ? 'luna-model' : 'moon-mean-elements'))
      : (this.usingKernels(jd) ? 'de440' : 'approx-elements');
    rows.push({ label: 'Radius', value: `${Math.round(b.radius / 1e3).toLocaleString()} km`, ev: 'physical-constants' });
    if (b.kind !== 'star') rows.push({ label: 'From the Sun', value: `${(b.world.length() / AU).toFixed(3)} AU`, ev: evState });
    const ol = this.orbitLines.get(b.name);
    if (ol) rows.push({ label: 'Orbit', value: ol.period > 800 ? `${(ol.period / 365.25).toFixed(1)} years` : `${ol.period.toFixed(1)} days`, ev: evState });
    if (b.kind === 'moon' && !moonKernel && !isLuna) rows.push({ label: 'Place on its orbit', value: 'Illustrative, no kernel loaded', ev: 'moon-phase' });
    rows.push({ label: 'Distance', value: prettyRange(b.world.distanceTo(camWorld)), live: 'range' });
    const rec = this.groups.get(b.name);
    if (rec) {
      if (b.name === 'Earth') {
        if (rec.textured) rows.push({ label: 'Surface', value: 'Blue Marble mosaic', ev: 'surface-imagery' });
        else rows.push({ label: 'Land', value: 'Natural Earth outlines, painted', ev: 'coastlines' });
        rows.push(rec.nightReal
          ? { label: 'Night lights', value: 'Black Marble mosaic', ev: 'surface-imagery' }
          : { label: 'Night lights', value: 'Synthetic placement', ev: 'land-lights' });
        if (rec.cloudsReal) rows.push({ label: 'Clouds', value: 'One saved snapshot, not today', ev: 'cloud-layer' });
      } else if (rec.textured) {
        rows.push({ label: 'Surface', value: b.name === 'Sun' ? 'Solar mosaic, animated' : 'Mission mosaic', ev: 'surface-imagery' });
      } else {
        rows.push({ label: 'Surface', value: 'Procedural, no imagery', ev: 'procedural-surface' });
      }
      if (rec.ring) rows.push(rec.ringReal
        ? { label: 'Rings', value: 'Radial brightness strip', ev: 'ring-imagery' }
        : { label: 'Rings', value: 'From published zone radii', ev: 'ring-zones' });
      if (rec.atmoMat) rows.push({ label: 'Atmosphere', value: 'Scattering model', ev: 'atmosphere-model' });
      if (rec.isStar) rows.push({ label: 'Corona', value: 'Illustrative', ev: 'corona-illustrative' });
      if (rec.planetIdx !== undefined || rec.moonIdx !== undefined) {
        if (rec.moonIdx === undefined) rows.push({ label: 'Rotation', value: 'IAU pole and meridian', ev: 'iau-rotation' });
        else rows.push({ label: 'Rotation', value: 'Tidally locked approximation', ev: 'moon-mean-elements' });
      }
    }
    return {
      title: b.name,
      kind: b.kind === 'moon' ? `Moon of ${b.parent}` : b.kind === 'star' ? 'Our star' : 'Planet',
      rows,
    };
  }
}

export function prettyRange(m) {
  if (m < 1e7) return `${(m / 1e3).toFixed(0)} km`;
  if (m < 0.1 * AU) return `${Math.round(m / 1e3).toLocaleString()} km`;
  return `${(m / AU).toFixed(3)} AU`;
}
