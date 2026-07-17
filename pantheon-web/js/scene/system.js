// Pantheon 3D — the solar system: procedurally-shaded spheres with IAU
// orientations, motion-graded orbit engravings, rings, Earth vector
// coastlines + night lights, spacecraft trails, and flux-faithful far glows.
//
// All world math is f64 (JS numbers / THREE.Vector3); every group is parked
// camera-relative each frame. Mesh geometry is a unit sphere scaled by the
// body radius; object space IS the body-fixed frame (poles at object +Y,
// prime meridian at object +X), so surface noise stays pinned to the ground
// while the IAU quaternion spins the mesh.
import * as THREE from 'three';
import * as eph from '../eph.js';

const NOISE = /* glsl */`
  float hash1(vec3 p) {
    p = fract(p * 0.3183099 + vec3(0.1, 0.17, 0.13));
    p *= 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
  }
  float vnoise(vec3 p) {
    vec3 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
      mix(mix(hash1(i), hash1(i + vec3(1,0,0)), f.x),
          mix(hash1(i + vec3(0,1,0)), hash1(i + vec3(1,1,0)), f.x), f.y),
      mix(mix(hash1(i + vec3(0,0,1)), hash1(i + vec3(1,0,1)), f.x),
          mix(hash1(i + vec3(0,1,1)), hash1(i + vec3(1,1,1)), f.x), f.y), f.z);
  }
  float fbm(vec3 p) {
    float a = 0.5, s = 0.0;
    for (int i = 0; i < 5; i++) { s += a * vnoise(p); p *= 2.03; a *= 0.5; }
    return s;
  }
`;

const BODY_VSH = /* glsl */`
  varying vec3 vObj;      // object-space position on unit sphere (= normal)
  varying vec3 vViewDir;  // view-space direction to fragment
  varying vec3 vNormalV;  // view-space normal
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vObj = position;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    vViewDir = normalize(mv.xyz);
    vNormalV = normalize(normalMatrix * normal);
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;

const BODY_FSH = /* glsl */`
  varying vec3 vObj;
  varying vec3 vViewDir;
  varying vec3 vNormalV;
  uniform int uKind;        // 0 sun, 1 rocky, 2 gas, 3 ice, 4 earth
  uniform vec3 uColA, uColB;
  uniform vec4 uParam;      // x: polar cap, y: detail, z: band freq, w: spot
  uniform vec3 uSunObj;     // sun direction, object space
  uniform float uTime, uSeed;
  uniform float uClose;     // 0 far .. 1 at the surface (extra octaves + relief)
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  void main() {
    #include <logdepthbuf_fragment>
    vec3 n = normalize(vObj);
    vec3 np = n;              // shading normal (relief-perturbed up close)
    float lat = asin(clamp(n.y, -1.0, 1.0));
    float lon = atan(n.z, n.x);
    vec3 p = n * uParam.y + vec3(uSeed);
    vec3 col;
    if (uKind == 0) {
      // Sun: granulation + limb darkening, HDR emissive
      float g = fbm(n * 14.0 + vec3(0.0, uTime * 0.02, 0.0));
      float g2 = fbm(n * 40.0 - vec3(uTime * 0.03));
      float limb = pow(max(dot(-vViewDir, vNormalV), 0.0), 0.55);
      col = mix(uColB, uColA, g * 0.7 + g2 * 0.3) * (0.55 + 1.1 * limb) * 2.5;
      gl_FragColor = vec4(col, 1.0);
      return;
    } else if (uKind == 2) {
      // banded gas giant: latitudinal bands warped by turbulence
      float turb = fbm(vec3(lon * 1.5, lat * 3.0, uSeed)) - 0.5;
      float band = sin(lat * uParam.z + turb * 3.4) * 0.5 + 0.5;
      float fine = fbm(vec3(lon * 4.0, lat * uParam.z * 1.7, uSeed + 4.0));
      col = mix(uColA, uColB, band) * (0.82 + 0.36 * fine);
      if (uParam.w > 0.0) {
        // great oval storm
        vec2 d = vec2((lon - 0.6) * cos(lat) / 0.26, (lat + 0.38) / 0.11);
        float spot = exp(-dot(d, d));
        col = mix(col, vec3(0.78, 0.42, 0.28), spot * uParam.w);
      }
    } else if (uKind == 3) {
      // ice giant: serene gradient, whisper bands
      float band = sin(lat * uParam.z) * 0.5 + 0.5;
      float haze = fbm(p * 0.8);
      col = mix(uColA, uColB, lat * 0.5 + 0.5) * (0.9 + 0.12 * band + 0.08 * haze);
    } else if (uKind == 4) {
      // Earth: deep ink ocean; coasts/lights are vector children
      float sea = fbm(p * 2.0);
      col = mix(uColA, uColB, sea * 0.55);
      col += vec3(0.10, 0.13, 0.16) * pow(1.0 - abs(dot(vViewDir, vNormalV)), 2.0);
    } else {
      // rocky: albedo fbm + crater-ish sharpening + optional polar caps
      float a1 = fbm(p * 3.0);
      float a2 = fbm(p * 9.0 + 7.0);
      col = mix(uColA, uColB, a1) * (0.72 + 0.55 * a2);
      if (uParam.x > 0.0) {
        float cap = smoothstep(uParam.x, uParam.x + 0.07, abs(n.y) + 0.05 * (a1 - 0.5));
        col = mix(col, vec3(0.93, 0.94, 0.96), cap);
      }
    }
    // close-range detail — uClose is a uniform, so the branch is free when far
    if (uClose > 0.002) {
      float f1 = fbm(p * 7.0 + 11.0);
      float f2 = fbm(p * 26.0 + 23.0);
      if (uKind == 2 || uKind == 3) {
        // storm filaments sheared along the bands
        float fl = fbm(vec3(lon * 10.0, lat * 40.0, uSeed + 7.0));
        col *= 1.0 + uClose * (0.22 * (fl - 0.5) + 0.12 * (f2 - 0.5));
      } else if (uKind == 4) {
        col *= 1.0 + uClose * 0.15 * (f2 - 0.5);   // ocean mottling
      } else {
        float ridge = 1.0 - abs(f1 * 2.0 - 1.0);
        col *= 1.0 + uClose * (0.30 * (f1 - 0.5) + 0.26 * (f2 - 0.5) - 0.16 * ridge * ridge);
        // relief: perturb the shading normal from the same height field
        vec3 t1 = normalize(cross(n, abs(n.y) > 0.93 ? vec3(1, 0, 0) : vec3(0, 1, 0)));
        vec3 t2 = cross(n, t1);
        float e = 0.012;
        float hC = fbm((n * uParam.y + vec3(uSeed)) * 9.0 + 31.0);
        float hX = fbm(((n + t1 * e) * uParam.y + vec3(uSeed)) * 9.0 + 31.0);
        float hY = fbm(((n + t2 * e) * uParam.y + vec3(uSeed)) * 9.0 + 31.0);
        np = normalize(n - (t1 * (hX - hC) + t2 * (hY - hC)) * (0.05 / e) * uClose);
      }
    }
    float diff = smoothstep(-0.06, 0.14, dot(np, uSunObj));
    // engraved graticule, 15 deg hairlines (recedes near the ground)
    float gLat = abs(fract(lat * 57.29578 / 15.0 + 0.5) - 0.5);
    float gLon = abs(fract(lon * 57.29578 / 15.0 + 0.5) - 0.5) * cos(lat);
    float grat = (1.0 - smoothstep(0.0, 0.03, min(gLat, gLon))) * 0.05;
    col = col * (0.045 + diff) + vec3(0.85, 0.70, 0.42) * grat * (0.25 + diff) * (1.0 - 0.6 * uClose);
    gl_FragColor = vec4(col, 1.0);
  }
`;

const ATMO_VSH = /* glsl */`
  varying vec3 vN; varying vec3 vV; varying vec3 vObj;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vObj = position;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    vN = normalize(normalMatrix * normal);
    vV = normalize(mv.xyz);
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const ATMO_FSH = /* glsl */`
  varying vec3 vN; varying vec3 vV; varying vec3 vObj;
  uniform vec3 uCol; uniform float uStrength; uniform vec3 uSunObj;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    float rim = pow(1.0 - abs(dot(vN, vV)), 3.0);
    float day = smoothstep(-0.35, 0.3, dot(normalize(vObj), uSunObj));
    gl_FragColor = vec4(uCol * rim * uStrength * (0.12 + day), 1.0);
  }
`;

const ORBIT_VSH = /* glsl */`
  attribute float t;
  varying float vT;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vT = t;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    #include <logdepthbuf_vertex>
  }
`;
const ORBIT_FSH = /* glsl */`
  varying float vT;
  uniform vec3 uCol; uniform float uHead; uniform float uGain;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    float d = fract(uHead - vT);          // 0 at head, grows behind
    float a = 0.06 + 0.5 * pow(1.0 - d, 3.0);
    gl_FragColor = vec4(uCol * a * uGain, 1.0);
  }
`;

const LIGHTS_VSH = /* glsl */`
  varying float vGate;
  uniform vec3 uSunObj;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vec3 n = normalize(position);
    vGate = smoothstep(0.02, -0.12, dot(n, uSunObj));   // night side only
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_PointSize = 1.8;
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const LIGHTS_FSH = /* glsl */`
  varying float vGate;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    vec2 pc = gl_PointCoord * 2.0 - 1.0;
    float d2 = dot(pc, pc);
    if (d2 > 1.0) discard;
    gl_FragColor = vec4(vec3(1.0, 0.82, 0.58) * exp(-d2 * 3.0) * vGate * 0.85, 1.0);
  }
`;

const RING_VSH = /* glsl */`
  varying vec3 vObj;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vObj = position;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
    #include <logdepthbuf_vertex>
  }
`;
const RING_FSH = /* glsl */`
  varying vec3 vObj;
  uniform vec4 uZones[12];   // r0, r1 (m), alpha, tone
  uniform int uNZones;
  uniform vec3 uSunObj;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  ${NOISE}
  void main() {
    #include <logdepthbuf_fragment>
    float r = length(vObj.xy);
    float a = 0.0, tone = 1.0;
    for (int i = 0; i < 12; i++) {
      if (i >= uNZones) break;
      vec4 z = uZones[i];
      if (r >= z.x && r <= z.y) {
        float e = min(smoothstep(z.x, z.x + (z.y - z.x) * 0.12, r),
                      1.0 - smoothstep(z.y - (z.y - z.x) * 0.12, z.y, r));
        a = max(a, z.z * e); tone = z.w;
      }
    }
    if (a <= 0.001) discard;
    float grain = 0.75 + 0.5 * vnoise(vec3(r * 3.0e-7, 0.0, 0.0));
    float lit = 0.35 + 0.65 * abs(uSunObj.z);
    vec3 col = mix(vec3(0.86, 0.78, 0.62), vec3(0.72, 0.68, 0.60), tone);
    gl_FragColor = vec4(col * a * grain * lit * 0.85, 1.0);
  }
`;

const GLOW_VSH = /* glsl */`
  attribute vec3 col;
  attribute float lum;
  varying vec3 vCol; varying float vPx;
  uniform float uPx; uniform float uPc;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vCol = col;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    float dp = length(mv.xyz / uPc);   // divide first: meters² overflows f32
    float flux = lum / max(dp * dp * 0.01, 1e-16);
    float px = uPx * pow(flux, 0.25);
    vPx = px;
    gl_PointSize = clamp(px, 1.0, 26.0);
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const GLOW_FSH = /* glsl */`
  varying vec3 vCol; varying float vPx;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    vec2 pc = gl_PointCoord * 2.0 - 1.0;
    float d2 = dot(pc, pc);
    if (d2 > 1.0) discard;
    float e = clamp(vPx / 1.4, 0.0, 1.0); e *= e;
    float rays = 1.0 + 0.25 * pow(abs(pc.x * pc.y) * 4.0, 0.5) * smoothstep(8.0, 20.0, vPx);
    gl_FragColor = vec4(vCol * (exp(-d2 * 4.5) + 0.2 * exp(-d2 * 1.2)) * e / rays, 1.0);
  }
`;

// Per-body surface styling, keyed by name (decoupled from eph kind numbering).
const STYLE = {
  Sun:     { kind: 0, a: '#fff6e0', b: '#ffb44d', p: [0, 1, 0, 0] },
  Mercury: { kind: 1, a: '#8a8078', b: '#565049', p: [0, 1.7, 0, 0] },
  Venus:   { kind: 1, a: '#e8d5a8', b: '#c9ab74', p: [0, 0.8, 0, 0], atmo: ['#e8d8b0', 1.1] },
  Earth:   { kind: 4, a: '#0a1d33', b: '#123252', p: [0, 2, 0, 0], atmo: ['#6fa8dc', 1.5] },
  Mars:    { kind: 1, a: '#b06a43', b: '#7a4530', p: [0.9, 1.3, 0, 0], atmo: ['#c88a5a', 0.35] },
  Jupiter: { kind: 2, a: '#d8c0a0', b: '#a07858', p: [0, 1, 9.0, 0.9] },
  Saturn:  { kind: 2, a: '#e0d0a8', b: '#b8a070', p: [0, 1, 7.0, 0] },
  Uranus:  { kind: 3, a: '#9fd4d8', b: '#7fb8c8', p: [0, 1, 6.0, 0] },
  Neptune: { kind: 3, a: '#4f78c8', b: '#35529a', p: [0, 1, 6.0, 0], atmo: ['#6f90d8', 0.4] },
  Pluto:   { kind: 1, a: '#c8b49a', b: '#8a7460', p: [0.85, 1.2, 0, 0] },
  Moon:    { kind: 1, a: '#b8b4ac', b: '#6a675f', p: [0, 2.2, 0, 0] },
  Io:      { kind: 1, a: '#d8c060', b: '#a08030', p: [0, 1.8, 0, 0] },
  Europa:  { kind: 1, a: '#d8cfc0', b: '#a89880', p: [0, 1.1, 0, 0] },
  Titan:   { kind: 1, a: '#c89850', b: '#906830', p: [0, 0.7, 0, 0], atmo: ['#d8a860', 0.9] },
  Triton:  { kind: 1, a: '#d0c8c0', b: '#98908a', p: [0.8, 1.3, 0, 0] },
};
function styleFor(name) {
  return STYLE[name] || { kind: 1, a: '#a89c8c', b: '#6a6055', p: [0, 1.5, 0, 0] };
}

function basisToQuatMesh(b) {   // object +Y -> pole, +X -> prime meridian
  const zc = b.x.clone().cross(b.z);
  return new THREE.Quaternion().setFromRotationMatrix(new THREE.Matrix4().makeBasis(b.x, b.z, zc));
}
function basisToQuatRing(b) {   // object +Z -> pole (ring/orbit plane in XY)
  return new THREE.Quaternion().setFromRotationMatrix(new THREE.Matrix4().makeBasis(b.x, b.z.clone().cross(b.x), b.z));
}

function bodyMaterial(name, seed) {
  const s = styleFor(name);
  return new THREE.ShaderMaterial({
    vertexShader: BODY_VSH, fragmentShader: BODY_FSH,
    uniforms: {
      uKind: { value: s.kind },
      uColA: { value: new THREE.Color(s.a) },
      uColB: { value: new THREE.Color(s.b) },
      uParam: { value: new THREE.Vector4(...s.p) },
      uSunObj: { value: new THREE.Vector3(0, 0, 1) },
      uTime: { value: 0 }, uSeed: { value: seed },
      uClose: { value: 0 },
    },
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
  g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e16);
  const mat = new THREE.ShaderMaterial({
    vertexShader: ORBIT_VSH, fragmentShader: ORBIT_FSH,
    uniforms: { uCol: { value: new THREE.Color(color) }, uHead: { value: 0 }, uGain: { value: gain } },
    blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
  });
  const line = new THREE.Line(g, mat);
  line.frustumCulled = false;
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

function lonLatToUnit(lonDeg, latDeg) {
  // object space: +Y pole, +X prime meridian, right-handed (lon east +)
  const lon = lonDeg * Math.PI / 180, lat = latDeg * Math.PI / 180;
  return new THREE.Vector3(Math.cos(lat) * Math.cos(lon), Math.sin(lat), Math.cos(lat) * Math.sin(lon));
}

export class SolarSystem {
  constructor(scene) {
    this.scene = scene;
    this.helio = new THREE.Group();            // heliocentric static ink (orbits, trails)
    scene.add(this.helio);
    this.sphereGeo = new THREE.SphereGeometry(1, 96, 64);
    this.bodiesList = [];
    this.groups = new Map();                   // name -> {group, mesh, mat, radius, ...}
    this.moonOrbitGroups = new Map();          // planet name -> group (parent-centered)
    this.orbitLines = new Map();               // name -> {line, jd0, period}
    this.trackHeads = [];
    this.stateCache = null;                    // set via setStates
    this._buildSun();
    for (let i = 0; i < eph.PLANETS.length; i++) this._buildPlanet(i);
    this._buildMoons();
    this._buildGlowPoints();
  }

  _mkBodyGroup(name, radius, seed, withAtmo) {
    const group = new THREE.Group();
    const mat = bodyMaterial(name, seed);
    const mesh = new THREE.Mesh(this.sphereGeo, mat);
    mesh.scale.setScalar(radius);
    group.add(mesh);
    let atmoMat = null;
    const st = styleFor(name);
    if (st.atmo) {
      atmoMat = new THREE.ShaderMaterial({
        vertexShader: ATMO_VSH, fragmentShader: ATMO_FSH,
        uniforms: {
          uCol: { value: new THREE.Color(st.atmo[0]) },
          uStrength: { value: st.atmo[1] },
          uSunObj: { value: new THREE.Vector3(0, 0, 1) },
        },
        blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
        side: THREE.FrontSide,
      });
      const shell = new THREE.Mesh(this.sphereGeo, atmoMat);
      shell.scale.setScalar(radius * 1.022);
      group.add(shell);
    }
    this.scene.add(group);
    const rec = { name, group, mesh, mat, atmoMat, radius, world: new THREE.Vector3() };
    this.groups.set(name, rec);
    return rec;
  }

  _buildSun() {
    const r = this._mkBodyGroup('Sun', 6.957e8, 1.0);
    // additive glow billboard, fixed angular size
    const gmat = new THREE.ShaderMaterial({
      vertexShader: /* glsl */`
        varying vec2 vUv;
        #include <common>
        #include <logdepthbuf_pars_vertex>
        void main() {
          vUv = uv * 2.0 - 1.0;
          gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
          #include <logdepthbuf_vertex>
        }`,
      fragmentShader: /* glsl */`
        varying vec2 vUv;
        #include <common>
        #include <logdepthbuf_pars_fragment>
        void main() {
          #include <logdepthbuf_fragment>
          float d = length(vUv);
          if (d > 1.0) discard;
          float g = exp(-d * d * 7.0) * 1.4 + exp(-d * 2.6) * 0.3;
          float rays = 1.0 + 0.35 * pow(abs(vUv.x * vUv.y) * 4.0, 0.6);
          g *= smoothstep(1.0, 0.72, d);   // reach zero before the quad edge
          gl_FragColor = vec4(vec3(1.0, 0.88, 0.66) * g / rays, 1.0);
        }`,
      blending: THREE.AdditiveBlending, depthWrite: false, depthTest: true, transparent: true,
    });
    this.sunGlow = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), gmat);
    this.sunGlow.frustumCulled = false;
    r.group.add(this.sunGlow);
  }

  _buildPlanet(i) {
    const p = eph.PLANETS[i];
    const rec = this._mkBodyGroup(p.name, p.radius, i * 3.7 + 2);
    rec.planetIdx = i;
    // orbit engraving, sampled uniformly in time over one period
    const per = p.period || this._estimatePeriod(i);
    rec.period = per;
    const jd0 = eph.jdTdb(eph.jdUtcNow());
    const N = 512, samples = [];
    for (let k = 0; k < N; k++) samples.push(eph.planetState(i, jd0 + per * k / N).pos);
    samples.push(samples[0].clone());
    const line = orbitLine(samples, '#d8b36a', 0.9);
    this.helio.add(line);
    this.orbitLines.set(p.name, { line, jd0, period: per });
    this._buildRings(rec, i);
    if (p.name === 'Earth') rec.needsEarthExtras = true;
  }

  _estimatePeriod(i) {
    const a = eph.PLANETS[i].a || 1;
    return 365.25 * Math.pow(a, 1.5);
  }

  _buildRings(rec, planetIdx) {
    const entry = (eph.RINGS || []).find(r => r.planet === planetIdx);
    if (!entry || !entry.zones.length) return;
    const zones = entry.zones;
    const inner = Math.min(...zones.map(z => z.rIn));
    const outer = Math.max(...zones.map(z => z.rOut));
    const geo = new THREE.RingGeometry(inner, outer, 256, 4);
    const zArr = [];
    for (let i = 0; i < 12; i++) {
      const z = zones[i];
      zArr.push(z ? new THREE.Vector4(z.rIn, z.rOut, Math.min(z.density * 0.7, 0.85), 0.5)
                  : new THREE.Vector4(0, 0, 0, 0));
    }
    const mat = new THREE.ShaderMaterial({
      vertexShader: RING_VSH, fragmentShader: RING_FSH,
      uniforms: {
        uZones: { value: zArr }, uNZones: { value: Math.min(zones.length, 12) },
        uSunObj: { value: new THREE.Vector3(0, 0, 1) },
      },
      blending: THREE.AdditiveBlending, depthWrite: false, transparent: true, side: THREE.DoubleSide,
    });
    rec.ring = new THREE.Mesh(geo, mat);
    rec.group.add(rec.ring);
  }

  _buildMoons() {
    for (let mi = 0; mi < eph.MOONS.length; mi++) {
      const m = eph.MOONS[mi];
      const rec = this._mkBodyGroup(m.name, m.radius, 40 + mi * 1.31);
      rec.moonIdx = mi;
      rec.parent = eph.PLANETS[m.parent].name;
      let og = this.moonOrbitGroups.get(rec.parent);
      if (!og) {
        og = new THREE.Group();
        this.scene.add(og);
        this.moonOrbitGroups.set(rec.parent, og);
      }
      const jd0 = eph.jdTdb(eph.jdUtcNow());
      const N = 192, samples = [];
      for (let k = 0; k < N; k++) samples.push(eph.moonRel(mi, jd0 + m.period * k / N));
      samples.push(samples[0].clone());
      const line = orbitLine(samples, '#9ab8d8', 0.7);
      og.add(line);
      this.orbitLines.set(m.name, { line, jd0, period: m.period });
    }
  }

  _buildGlowPoints() {
    // planets (and the Sun) as physically-fading glow points for far viewing
    const names = ['Sun', ...eph.PLANETS.map(p => p.name)];
    // rel. luminosity vs Mg=0 at 10 pc; Sun real (Mg 4.67), planets tuned to
    // read like the bright wanderers they are from ~1 AU
    const LUM = { Sun: 0.0136, Mercury: 3e-15, Venus: 3.2e-14, Earth: 1.5e-14, Mars: 6e-15,
      Jupiter: 5e-14, Saturn: 2.6e-14, Uranus: 4e-15, Neptune: 3e-15, Pluto: 3e-16 };
    const COL = { Sun: [1.0, 0.93, 0.78], Mercury: [0.8, 0.76, 0.7], Venus: [1.0, 0.95, 0.8],
      Earth: [0.55, 0.72, 1.0], Mars: [1.0, 0.6, 0.4], Jupiter: [1.0, 0.9, 0.75],
      Saturn: [1.0, 0.94, 0.78], Uranus: [0.7, 0.95, 0.95], Neptune: [0.55, 0.65, 1.0],
      Pluto: [0.85, 0.8, 0.72] };
    const n = names.length;
    const pos = new Float32Array(n * 3), col = new Float32Array(n * 3), lum = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      const c = COL[names[i]] || [0.9, 0.88, 0.8];
      col.set(c, 3 * i);
      lum[i] = LUM[names[i]] || 1e-15;
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
    g.setAttribute('col', new THREE.BufferAttribute(col, 3));
    g.setAttribute('lum', new THREE.BufferAttribute(lum, 1));
    g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e14);
    const mat = new THREE.ShaderMaterial({
      vertexShader: GLOW_VSH, fragmentShader: GLOW_FSH,
      uniforms: { uPx: { value: 8.0 }, uPc: { value: 3.0856775814913673e16 } },
      blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
    });
    this.glowNames = names;
    this.glowPts = new THREE.Points(g, mat);
    this.glowPts.frustumCulled = false;
    this.glowGroup = new THREE.Group();
    this.glowGroup.add(this.glowPts);
    this.scene.add(this.glowGroup);
  }

  // Earth vector extras — call once coast (and land) GeoJSON rings are loaded.
  addEarthExtras(coastRings, landRings, rand) {
    const earth = this.groups.get('Earth');
    if (!earth || earth.coastAdded) return;
    earth.coastAdded = true;
    // coastlines as line segments on the unit sphere (children of the mesh:
    // they inherit radius scale and the IAU spin quaternion)
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
          gl_FragColor = vec4(vec3(0.85, 0.72, 0.45) * 0.55, 1.0);
        }`,
      blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
    });
    const coast = new THREE.LineSegments(cg, cmat);
    coast.frustumCulled = false;
    earth.mesh.add(coast);
    // night lights: deterministic settlement points on land, night side only
    if (landRings && landRings.length) {
      const pts = [];
      let tries = 0;
      while (pts.length < 1200 * 3 && tries++ < 60000) {
        const lon = rand() * 360 - 180;
        const lat = (Math.asin(rand() * 2 - 1) * 180 / Math.PI) * 0.72; // mid-lat prior
        if (pointInRings(lon, lat, landRings)) {
          const v = lonLatToUnit(lon, lat).multiplyScalar(1.002);
          pts.push(v.x, v.y, v.z);
        }
      }
      const lg = new THREE.BufferGeometry();
      lg.setAttribute('position', new THREE.BufferAttribute(new Float32Array(pts), 3));
      earth.lightsMat = new THREE.ShaderMaterial({
        vertexShader: LIGHTS_VSH, fragmentShader: LIGHTS_FSH,
        uniforms: { uSunObj: { value: new THREE.Vector3(0, 0, 1) } },
        blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
      });
      const lights = new THREE.Points(lg, earth.lightsMat);
      lights.frustumCulled = false;
      earth.mesh.add(lights);
    }
  }

  setTracks(tracks) {
    this.tracks = tracks || [];
    for (const tr of this.tracks) {
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
      g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e14);
      const mat = new THREE.ShaderMaterial({
        vertexShader: ORBIT_VSH, fragmentShader: ORBIT_FSH,
        uniforms: { uCol: { value: new THREE.Color('#7ea8d8') }, uHead: { value: 1.0 }, uGain: { value: 0.8 } },
        blending: THREE.AdditiveBlending, depthWrite: false, transparent: true,
      });
      const line = new THREE.Line(g, mat);
      line.frustumCulled = false;
      this.helio.add(line);
      const last = tr.samples[n - 1];
      this.trackHeads.push({ name: tr.name, world: new THREE.Vector3(last[1], last[2], last[3]) });
    }
  }

  setStates(states) { this.stateCache = states; }

  _statePos(kind, name, idx, jd) {
    // API cache (SPICE truth) extrapolated by vel; fallback: analytic elements
    const c = this.stateCache;
    if (c) {
      const list = kind === 'planet' ? c.bodies : c.moons;
      const e = list && list.find(b => b.name === name);
      if (e) {
        const dj = jd - c.jd;
        if (Math.abs(dj) < 1.5) {
          return new THREE.Vector3(
            e.pos[0] + e.vel[0] * dj, e.pos[1] + e.vel[1] * dj, e.pos[2] + e.vel[2] * dj);
        }
      }
    }
    return kind === 'planet' ? eph.planetState(idx, jd).pos : eph.moonRel(idx, jd);
  }

  stateSrc() {
    return this.stateCache && this.stateCache.src === 'spice' ? 'DE440s + SPKs' : 'mean elements';
  }

  update(jd, camWorld, tSec) {
    this.bodiesList.length = 0;
    const off = (g, world) => {
      g.position.set(world.x - camWorld.x, world.y - camWorld.y, world.z - camWorld.z);
    };
    // Sun
    const sun = this.groups.get('Sun');
    sun.world.set(0, 0, 0);
    off(sun.group, sun.world);
    sun.mat.uniforms.uTime.value = tSec;
    // corona billboard stays body-scaled; the flux glow point carries the
    // sun's appearance at range (a fixed-angular quad reads as an artifact)
    this.sunGlow.scale.setScalar(sun.radius * 4.5);
    this.sunGlow.quaternion.copy(this._camQuat || new THREE.Quaternion());
    this.bodiesList.push({ name: 'Sun', kind: 'star', world: sun.world, radius: sun.radius });
    // helio ink + glow cloud ride the sun frame
    this.helio.position.copy(sun.group.position);
    this.glowGroup.position.copy(sun.group.position);
    // planets
    for (let i = 0; i < eph.PLANETS.length; i++) {
      const p = eph.PLANETS[i];
      const rec = this.groups.get(p.name);
      rec.world.copy(this._statePos('planet', p.name, i, jd));
      off(rec.group, rec.world);
      const basis = eph.bodyBasis(i, jd);
      rec.mesh.quaternion.copy(basisToQuatMesh(basis));
      if (rec.ring) rec.ring.quaternion.copy(basisToQuatRing(basis));
      const d = rec.world.distanceTo(camWorld);
      rec.group.visible = d < rec.radius * 250000;
      if (rec.group.visible) this._lightUniforms(rec, camWorld);
      const ol = this.orbitLines.get(p.name);
      ol.line.material.uniforms.uHead.value = ((jd - ol.jd0) / ol.period) % 1;
      this.bodiesList.push({ name: p.name, kind: 'planet', world: rec.world, radius: rec.radius, idx: i });
      // moon orbit group rides the planet
      const og = this.moonOrbitGroups.get(p.name);
      if (og) {
        og.position.copy(rec.group.position);
        og.visible = d < 3e12;
      }
    }
    // moons
    for (let mi = 0; mi < eph.MOONS.length; mi++) {
      const m = eph.MOONS[mi];
      const rec = this.groups.get(m.name);
      const parent = this.groups.get(rec.parent);
      const rel = this._statePos('moon', m.name, mi, jd);
      rec.world.copy(parent.world).add(rel);
      const dParent = parent.world.distanceTo(camWorld);
      const vis = dParent < 3e12;
      rec.group.visible = vis;
      if (vis) {
        off(rec.group, rec.world);
        rec.mesh.quaternion.copy(basisToQuatRing(eph.moonBasis(mi, jd)));
        this._lightUniforms(rec, camWorld);
        const ol = this.orbitLines.get(m.name);
        ol.line.material.uniforms.uHead.value = ((jd - ol.jd0) / ol.period) % 1;
        this.bodiesList.push({ name: m.name, kind: 'moon', world: rec.world, radius: rec.radius, moonIdx: mi, parent: rec.parent });
      }
    }
    // glow point positions (heliocentric)
    const gp = this.glowPts.geometry.getAttribute('position');
    for (let i = 0; i < this.glowNames.length; i++) {
      const rec = this.groups.get(this.glowNames[i]);
      gp.setXYZ(i, rec.world.x, rec.world.y, rec.world.z);
    }
    gp.needsUpdate = true;
    for (const th of this.trackHeads) {
      this.bodiesList.push({ name: th.name, kind: 'craft', world: th.world, radius: 10 });
    }
  }

  _lightUniforms(rec, camWorld) {
    // sun direction in the mesh's object space
    const sunDirWorld = rec.world.clone().negate().normalize();
    const inv = rec.mesh.quaternion.clone().invert();
    const so = sunDirWorld.clone().applyQuaternion(inv);
    rec.mat.uniforms.uSunObj.value.copy(so);
    if (rec.mat.uniforms.uClose) {
      const alt = rec.world.distanceTo(camWorld) / rec.radius - 1;
      rec.mat.uniforms.uClose.value = Math.min(Math.max(1 - alt / 9, 0), 1);
    }
    if (rec.atmoMat) rec.atmoMat.uniforms.uSunObj.value.copy(so);
    if (rec.lightsMat) rec.lightsMat.uniforms.uSunObj.value.copy(so);
    if (rec.ring) {
      const invR = rec.ring.quaternion.clone().invert();
      rec.ring.material.uniforms.uSunObj.value.copy(sunDirWorld.clone().applyQuaternion(invR));
    }
  }

  setCamQuat(q) { this._camQuat = q; }

  bodies() { return this.bodiesList; }

  // world position of a solar-system body at jd WITHOUT a full update — used
  // by the frame-riding camera, so it must match what update() will compute
  predictWorld(name, jd) {
    if (name === 'Sun') return new THREE.Vector3();
    const pi = eph.PLANETS.findIndex(p => p.name === name);
    if (pi >= 0) return this._statePos('planet', name, pi, jd);
    const mi = eph.MOONS.findIndex(m => m.name === name);
    if (mi >= 0) {
      const parentName = eph.PLANETS[eph.MOONS[mi].parent].name;
      return this.predictWorld(parentName, jd).add(this._statePos('moon', name, mi, jd));
    }
    return null;
  }

  nearestSurface(camWorld) {
    let best = Infinity, bestB = null;
    for (const b of this.bodiesList) {
      if (b.kind === 'craft') continue;
      const d = b.world.distanceTo(camWorld) - b.radius;
      if (d < best) { best = d; bestB = b; }
    }
    return { dist: Math.max(best, 0.1), body: bestB };
  }

  bodyCard(b, camWorld, jd) {
    const rows = [];
    const evState = this.stateCache && this.stateCache.src === 'spice'
      ? (b.kind === 'moon' ? 'satellite-spk' : 'de440') : 'approx-elements';
    rows.push({ label: 'radius', value: `${Math.round(b.radius / 1e3).toLocaleString()} km`, ev: 'iau-rotation' });
    const dSun = b.world.length();
    if (b.kind !== 'star') {
      rows.push({ label: 'sun distance', value: `${(dSun / 1.495978707e11).toFixed(3)} AU`, ev: evState });
    }
    const ol = this.orbitLines.get(b.name);
    if (ol) rows.push({ label: 'orbital period', value: ol.period > 800 ? `${(ol.period / 365.25).toFixed(2)} yr` : `${ol.period.toFixed(2)} d`, ev: evState });
    rows.push({ label: 'camera range', value: prettyRange(b.world.distanceTo(camWorld)) });
    if (b.name === 'Earth') {
      rows.push({ label: 'coastlines', value: 'Natural Earth 110m', ev: 'coastlines' });
      rows.push({ label: 'night lights', value: 'illustrative placement', ev: 'land-lights' });
    }
    return {
      title: b.name,
      kind: b.kind === 'moon' ? `moon of ${b.parent}` : b.kind,
      rows,
      note: `states: ${this.stateSrc()}`,
    };
  }
}

function prettyRange(m) {
  const AU = 1.495978707e11;
  if (m < 1e7) return `${(m / 1e3).toFixed(0)} km`;
  if (m < 0.1 * AU) return `${Math.round(m / 1e3).toLocaleString()} km`;
  return `${(m / AU).toFixed(3)} AU`;
}
