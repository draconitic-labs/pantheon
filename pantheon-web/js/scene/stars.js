// Pantheon 3D: star clouds. Vertices are SUN-CENTERED (m, ecliptic); main.js parks
// each cloud's group at (sunWorld - camWorld) every frame. Brightness comes from
// real flux: lum from absolute G magnitude, apparent flux by inverse square. Faint
// stars fade out continuously instead of popping, and bright ones bloom.
import * as THREE from 'three';

export const starUniforms = { uPR: { value: 1 } };

const VSH = /* glsl */`
  attribute vec3 col;
  attribute float lum;
  #ifdef PER_VERTEX_VIS
  attribute float vis;
  #endif
  varying vec3 vCol;
  varying float vPx, vI;
  uniform float uPc, uGain, uPR, uMaxPx, uFloor;
  #include <common>
  #include <logdepthbuf_pars_vertex>
  void main() {
    vCol = col;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    // divide BEFORE length: squaring meter-scale components overflows f32
    float dp = length(mv.xyz / uPc);
    float F = lum / max(dp * dp * 0.01, 1e-9);     // flux relative to magnitude 0
    float b = pow(F, 0.34);
    float fade = smoothstep(uFloor, uFloor * 6.0, b);
    float px = min(1.9 + 3.0 * pow(b, 0.85), uMaxPx);
    vPx = px;
    vI = uGain * min(b, 3.5) * fade;
    #ifdef PER_VERTEX_VIS
    vI *= vis;
    #endif
    if (vI < 0.0008) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); gl_PointSize = 0.0; return; }
    gl_PointSize = px * uPR;
    gl_Position = projectionMatrix * mv;
    #include <logdepthbuf_vertex>
  }
`;
const FSH = /* glsl */`
  varying vec3 vCol;
  varying float vPx, vI;
  #include <common>
  #include <logdepthbuf_pars_fragment>
  void main() {
    #include <logdepthbuf_fragment>
    vec2 pc = gl_PointCoord * 2.0 - 1.0;
    float d2 = dot(pc, pc);
    if (d2 > 1.0) discard;
    float core = exp(-d2 * 9.0);
    float halo = 0.12 * exp(-d2 * 2.2) * (1.0 - d2);
    float s = smoothstep(8.0, 18.0, vPx);
    float sp = s * 0.5 * (1.0 - d2) * (exp(-abs(pc.x) * 26.0) * exp(-pc.y * pc.y * 4.0)
                                     + exp(-abs(pc.y) * 26.0) * exp(-pc.x * pc.x * 4.0));
    vec3 c = mix(vCol, vec3(1.0), core * core * 0.35);
    gl_FragColor = vec4(c * (core + halo + sp) * vI, 1.0);
  }
`;

export function makeStarPoints(starSet, { perVertexVis = false, gain = 1.0, maxPx = 26, floor = 0.012 } = {}) {
  const n = starSet.count;
  let lum = starSet.lum;
  if (!lum) {
    lum = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      const m = starSet.meta[i];
      const mg = m.gmag - 5 * Math.log10(Math.max(m.distPc, 0.1) / 10);
      lum[i] = Math.pow(10, -0.4 * mg);
    }
  }
  // chroma only: the catalog colors carry an apparent-magnitude factor we don't want
  const col = new Float32Array(n * 3);
  for (let i = 0; i < n; i++) {
    const r = starSet.colors[3 * i], g = starSet.colors[3 * i + 1], b = starSet.colors[3 * i + 2];
    const k = 1 / Math.max(r, g, b, 1e-4);
    col[3 * i] = Math.pow(r * k, 1.35); col[3 * i + 1] = Math.pow(g * k, 1.35); col[3 * i + 2] = Math.pow(b * k, 1.35);
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(starSet.positions, 3));
  g.setAttribute('col', new THREE.BufferAttribute(col, 3));
  g.setAttribute('lum', new THREE.BufferAttribute(lum, 1));
  g.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e26);
  const mat = new THREE.ShaderMaterial({
    vertexShader: VSH, fragmentShader: FSH,
    defines: perVertexVis ? { PER_VERTEX_VIS: 1 } : {},
    uniforms: {
      uPc: { value: 3.0856775814913673e16 }, uGain: { value: gain }, uMaxPx: { value: maxPx },
      uFloor: { value: floor }, uPR: starUniforms.uPR,
    },
    blending: THREE.AdditiveBlending, depthWrite: false, depthTest: true, transparent: true,
  });
  const pts = new THREE.Points(g, mat);
  pts.frustumCulled = false;
  pts.renderOrder = -5;
  return pts;
}

// Screen-space picking: nearest star to an NDC point within maxPx. Only stars bright
// enough to see count, so a click on empty sky stays empty.
export function pickStar(starSet, ndc, camWorld, camera, maxPx = 8) {
  const w2 = window.innerWidth / 2, h2 = window.innerHeight / 2;
  const v = new THREE.Vector3();
  const fwd = new THREE.Vector3(0, 0, -1).applyQuaternion(camera.quaternion);
  let best = null, bestD = maxPx;
  const p = starSet.positions;
  for (let i = 0; i < starSet.count; i++) {
    v.set(p[3 * i] - camWorld.x, p[3 * i + 1] - camWorld.y, p[3 * i + 2] - camWorld.z);
    const dist = v.length();
    const m = starSet.meta[i];
    const mg = m.gmag - 5 * Math.log10(Math.max(m.distPc, 0.1) / 10);
    const apparent = mg + 5 * Math.log10(dist / 3.0856775814913673e17);
    if (apparent > 11 || v.dot(fwd) <= 0) continue;
    v.project(camera);
    const d = Math.hypot((v.x - ndc[0]) * w2, (v.y - ndc[1]) * h2);
    if (d < bestD) {
      bestD = d;
      best = { idx: i, px: d, dist, world: new THREE.Vector3(p[3 * i], p[3 * i + 1], p[3 * i + 2]) };
    }
  }
  return best;
}
