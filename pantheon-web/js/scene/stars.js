// Pantheon 3D — Gaia star clouds. Vertices are SUN-CENTERED (m, ecliptic);
// main.js parks each cloud's group at (sunWorld − camWorld) every frame.
// Point size derives from real flux: lum from absolute G magnitude, px ∝ flux^¼.
import * as THREE from 'three';

const VSH = /* glsl */`
  attribute vec3 col;
  attribute float lum;
  varying vec3 vCol;
  varying float vPx;
  uniform float uPx;      // px at 10 pc for Mg = 0
  uniform float uPc;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vCol = col;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    // divide BEFORE length: squaring meter-scale components overflows f32
    float dp = length(mv.xyz / uPc);             // distance in parsec
    float flux = lum / max(dp * dp * 0.01, 1e-14); // rel. flux vs Mg0 @10pc
    float px = uPx * pow(flux, 0.25);
    vPx = px;
    gl_PointSize = clamp(px, 1.5, 15.0);
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const FSH = /* glsl */`
  varying vec3 vCol;
  varying float vPx;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    vec2 pc = gl_PointCoord * 2.0 - 1.0;
    float d2 = dot(pc, pc);
    if (d2 > 1.0) discard;
    float core = exp(-d2 * 5.0);
    float halo = 0.22 * exp(-d2 * 1.4);
    // sub-pixel energy conservation: fade rather than alias away
    float e = clamp(vPx / 1.5, 0.0, 1.0);
    e *= e;
    // overbright cores read as white-hot through bloom
    float boost = 1.0 + 0.8 * smoothstep(6.0, 14.0, vPx);
    gl_FragColor = vec4(vCol * (core + halo) * e * boost, 1.0);
  }
`;

export function makeStarPoints(starSet, pxScale = 8.0) {
  const n = starSet.count;
  const lum = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    const m = starSet.meta[i];
    const mg = m.gmag - 5 * Math.log10(Math.max(m.distPc, 0.1) / 10);
    lum[i] = Math.pow(10, -0.4 * mg);
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(starSet.positions, 3));
  g.setAttribute('col', new THREE.BufferAttribute(starSet.colors, 3));
  g.setAttribute('lum', new THREE.BufferAttribute(lum, 1));
  // bounding sphere in meters overflows nothing, but compute manually to
  // avoid an O(n) pass over huge coords with the default epsilon logic
  g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e21);
  const mat = new THREE.ShaderMaterial({
    vertexShader: VSH, fragmentShader: FSH,
    uniforms: { uPx: { value: pxScale }, uPc: { value: 3.0856775814913673e16 } },
    blending: THREE.AdditiveBlending, depthWrite: false, depthTest: true,
    transparent: true,
  });
  const pts = new THREE.Points(g, mat);
  pts.frustumCulled = false;
  return pts;
}

// Screen-space picking: nearest star to NDC point within maxPx.
// Returns {idx, px, world} or null. worldOffset = cloud world center (sun = 0,0,0).
export function pickStar(starSet, ndc, camWorld, camera, maxPx = 14) {
  const w2 = window.innerWidth / 2, h2 = window.innerHeight / 2;
  const v = new THREE.Vector3();
  let best = null, bestD = maxPx;
  const p = starSet.positions;
  for (let i = 0; i < starSet.count; i++) {
    v.set(p[3 * i] - camWorld.x, p[3 * i + 1] - camWorld.y, p[3 * i + 2] - camWorld.z);
    const dist = v.length();
    v.project(camera);
    if (v.z < -1 || v.z > 1) continue;
    const dx = (v.x - ndc[0]) * w2, dy = (v.y - ndc[1]) * h2;
    const d = Math.hypot(dx, dy);
    if (d < bestD) {
      bestD = d;
      best = { idx: i, px: d, dist, world: new THREE.Vector3(p[3 * i], p[3 * i + 1], p[3 * i + 2]) };
    }
  }
  return best;
}
