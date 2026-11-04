// Pantheon 3D: the background layer. A full-screen triangle at the very back of the
// scene: the ESO all-sky photograph while parallax is negligible, plus whatever the
// galaxy volume pass rendered this frame. The photograph is a real view from here,
// so it can only be drawn near the Sun; the volume takes over as it fades.
import * as THREE from 'three';

const VSH = /* glsl */`
  varying vec2 vUv;
  void main() { vUv = uv; gl_Position = vec4(position.xy, 1.0, 1.0); }
`;
const FSH = /* glsl */`
  varying vec2 vUv;
  uniform sampler2D uPano, uVol;
  uniform float uA, uVolOn;
  uniform mat3 uRot, uToGal;
  uniform vec2 uTan, uVolTexel;
  void main() {
    vec3 c = vec3(0.0);
    if (uA > 0.002) {
      vec3 rd = normalize(uRot * vec3((vUv * 2.0 - 1.0) * uTan, -1.0));
      vec3 g = uToGal * rd;
      // galactic longitude increases to the left, with the center mid-frame
      float x = atan(g.y, g.x) * 0.15915494;
      float uA0 = fract(0.5 - x), uB0 = fract(1.0 - x) - 0.5;
      float u = fwidth(uA0) <= fwidth(uB0) + 1e-5 ? uA0 : uB0;
      vec3 t = texture2D(uPano, vec2(u, asin(clamp(g.z, -1.0, 1.0)) * 0.31830989 + 0.5)).rgb;
      c += max(t - vec3(0.0035), vec3(0.0)) * 0.34 * uA;
    }
    if (uVolOn > 0.5) {
      // the volume is low-resolution and jittered: a small tent hides both
      vec2 e = uVolTexel;
      c += (texture2D(uVol, vUv).rgb * 4.0
        + texture2D(uVol, vUv + vec2(e.x, 0.0)).rgb * 2.0 + texture2D(uVol, vUv - vec2(e.x, 0.0)).rgb * 2.0
        + texture2D(uVol, vUv + vec2(0.0, e.y)).rgb * 2.0 + texture2D(uVol, vUv - vec2(0.0, e.y)).rgb * 2.0
        + texture2D(uVol, vUv + e).rgb + texture2D(uVol, vUv - e).rgb
        + texture2D(uVol, vUv + vec2(e.x, -e.y)).rgb + texture2D(uVol, vUv + vec2(-e.x, e.y)).rgb) / 16.0;
    }
    gl_FragColor = vec4(c, 1.0);
  }
`;

export class Sky {
  constructor(scene, galBasis) {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 3, -1, 0, -1, 3, 0]), 3));
    geo.setAttribute('uv', new THREE.BufferAttribute(new Float32Array([0, 0, 2, 0, 0, 2]), 2));
    const toGal = new THREE.Matrix3().set(
      galBasis.x.x, galBasis.x.y, galBasis.x.z,
      galBasis.y.x, galBasis.y.y, galBasis.y.z,
      galBasis.z.x, galBasis.z.y, galBasis.z.z,
    );
    this.mat = new THREE.ShaderMaterial({
      vertexShader: VSH, fragmentShader: FSH,
      uniforms: {
        uPano: { value: null }, uVol: { value: null }, uA: { value: 0 }, uVolOn: { value: 0 },
        uRot: { value: new THREE.Matrix3() }, uToGal: { value: toGal }, uTan: { value: new THREE.Vector2(1, 1) },
        uVolTexel: { value: new THREE.Vector2(0.001, 0.001) },
      },
      depthTest: false, depthWrite: false,
    });
    this.mesh = new THREE.Mesh(geo, this.mat);
    this.mesh.frustumCulled = false;
    this.mesh.renderOrder = -100;
    scene.add(this.mesh);
    this.hasPano = false;
  }

  setPano(t) { this.mat.uniforms.uPano.value = t; this.hasPano = true; }

  update(camera, panoAlpha, volRT) {
    const volTex = volRT ? volRT.texture : null;
    if (volRT) this.mat.uniforms.uVolTexel.value.set(1 / volRT.width, 1 / volRT.height);
    const u = this.mat.uniforms;
    const e = camera.matrixWorld.elements;
    u.uRot.value.set(e[0], e[4], e[8], e[1], e[5], e[9], e[2], e[6], e[10]);
    const ty = Math.tan(camera.fov * Math.PI / 360);
    u.uTan.value.set(ty * camera.aspect, ty);
    u.uA.value = this.hasPano ? panoAlpha : 0;
    u.uVolOn.value = volTex ? 1 : 0;
    u.uVol.value = volTex;
    this.mesh.visible = u.uA.value > 0.002 || !!volTex;
  }
}
