// Pantheon 3D: the HDR pipeline. The scene renders into a half-float MSAA target;
// bloom is a threshold-free mip chain (13-tap down, tent up, Karis on the first
// step so single bright stars cannot flicker); a 1x1 target tracks scene
// brightness for eye adaptation; the last pass tone-maps (ACES), converts to
// sRGB, and dithers so the long dark gradients of space do not band.
import * as THREE from 'three';

const FS_VSH = /* glsl */`
  varying vec2 vUv;
  void main() { vUv = uv; gl_Position = vec4(position.xy, 0.0, 1.0); }
`;

const DOWN_FSH = /* glsl */`
  varying vec2 vUv;
  uniform sampler2D tSrc;
  uniform vec2 uTexel;
  uniform float uKaris;
  float kw(vec3 c) { return 1.0 / (1.0 + dot(c, vec3(0.2126, 0.7152, 0.0722))); }
  vec3 s(float x, float y) { return texture2D(tSrc, vUv + uTexel * vec2(x, y)).rgb; }
  void main() {
    vec3 a = s(-2., 2.), b = s(0., 2.), c = s(2., 2.);
    vec3 d = s(-2., 0.), e = s(0., 0.), f = s(2., 0.);
    vec3 g = s(-2., -2.), h = s(0., -2.), i = s(2., -2.);
    vec3 j = s(-1., 1.), k = s(1., 1.), l = s(-1., -1.), m = s(1., -1.);
    vec3 col;
    if (uKaris > 0.5) {
      vec3 g0 = (a + b + d + e) * 0.25, g1 = (b + c + e + f) * 0.25;
      vec3 g2 = (d + e + g + h) * 0.25, g3 = (e + f + h + i) * 0.25, g4 = (j + k + l + m) * 0.25;
      float w0 = kw(g0) * 0.125, w1 = kw(g1) * 0.125, w2 = kw(g2) * 0.125, w3 = kw(g3) * 0.125, w4 = kw(g4) * 0.5;
      col = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / max(w0 + w1 + w2 + w3 + w4, 1e-4);
    } else {
      col = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    }
    gl_FragColor = vec4(min(col, vec3(4000.0)), 1.0);
  }
`;

const UP_FSH = /* glsl */`
  varying vec2 vUv;
  uniform sampler2D tSrc;
  uniform vec2 uTexel;
  uniform float uRadius;
  void main() {
    vec2 t = uTexel * uRadius;
    vec3 c = texture2D(tSrc, vUv).rgb * 4.0;
    c += (texture2D(tSrc, vUv + vec2(t.x, 0.)).rgb + texture2D(tSrc, vUv - vec2(t.x, 0.)).rgb
        + texture2D(tSrc, vUv + vec2(0., t.y)).rgb + texture2D(tSrc, vUv - vec2(0., t.y)).rgb) * 2.0;
    c += texture2D(tSrc, vUv + t).rgb + texture2D(tSrc, vUv - t).rgb
       + texture2D(tSrc, vUv + vec2(t.x, -t.y)).rgb + texture2D(tSrc, vUv + vec2(-t.x, t.y)).rgb;
    gl_FragColor = vec4(c / 16.0, 1.0);
  }
`;

// average linear luminance of the smallest mip, eased over time
const ADAPT_FSH = /* glsl */`
  varying vec2 vUv;
  uniform sampler2D tSrc, tPrev;
  uniform float uK;
  float L(vec2 p) { return dot(texture2D(tSrc, p).rgb, vec3(0.2126, 0.7152, 0.0722)); }
  void main() {
    float a = 0.0;
    for (int y = 0; y < 3; y++) for (int x = 0; x < 3; x++)
      a += L(vec2(0.2 + 0.3 * float(x), 0.2 + 0.3 * float(y)));
    a /= 9.0;
    if (!(a < 1e4)) a = 0.0;   // NaN or overflow: never poison the eye
    gl_FragColor = vec4(vec3(mix(texture2D(tPrev, vec2(0.5)).r, a, uK)), 1.0);
  }
`;

const COMPOSITE_FSH = /* glsl */`
  varying vec2 vUv;
  uniform sampler2D tHdr, tBloom, tAdapt;
  uniform float uBloom, uExposure, uWarp, uTime, uAdaptK;
  uniform vec2 uWarpC;
  float ign(vec2 p) { return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715)))); }
  // Stephen Hill's ACES fit: filmic rolloff without the raw-ACES hue skew
  vec3 aces(vec3 c) {
    const mat3 I = mat3(0.59719, 0.07600, 0.02840, 0.35458, 0.90834, 0.13383, 0.04823, 0.01566, 0.83777);
    const mat3 O = mat3(1.60475, -0.10208, -0.00327, -0.53108, 1.10813, -0.07276, -0.07367, -0.00605, 1.07602);
    c = I * c;
    vec3 a = c * (c + 0.0245786) - 0.000090537;
    vec3 b = c * (0.983729 * c + 0.4329510) + 0.238081;
    return clamp(O * (a / b), 0.0, 1.0);
  }
  vec3 srgb(vec3 c) {
    return mix(c * 12.92, 1.055 * pow(max(c, vec3(1e-5)), vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
  }
  void main() {
    vec3 c;
    float noise = ign(gl_FragCoord.xy + fract(uTime * 7.3) * 64.0);
    if (uWarp > 0.004) {
      // flight streaks: radial blur from the vanishing point, strongest at the rim
      vec2 d = vUv - uWarpC;
      float k = uWarp * 0.16 * smoothstep(0.05, 0.75, length(d));
      c = vec3(0.0);
      for (int i = 0; i < 9; i++) c += texture2D(tHdr, uWarpC + d * (1.0 - k * (float(i) + noise) / 9.0)).rgb;
      c /= 9.0;
    } else {
      c = texture2D(tHdr, vUv).rgb;
    }
    c = mix(c, texture2D(tBloom, vUv).rgb, uBloom);
    float avg = texture2D(tAdapt, vec2(0.5)).r;
    c *= uExposure / (1.0 + uAdaptK * avg);
    vec2 q = vUv - 0.5;
    c *= 1.0 - 0.28 * smoothstep(0.12, 0.62, dot(q, q));
    c = srgb(aces(c));
    c += (noise + ign(gl_FragCoord.yx) - 1.0) * (1.6 / 255.0);
    gl_FragColor = vec4(c, 1.0);
  }
`;

export class Post {
  constructor(renderer, scene, camera) {
    this.renderer = renderer;
    this.scene = scene;
    this.camera = camera;
    this.w = 1; this.h = 1;
    this.levels = 6;
    this.msaa = 4;
    this.exposure = 1.0;
    this.bloom = 0.075;
    this.warp = 0;
    this.warpC = new THREE.Vector2(0.5, 0.5);
    this.mips = [];
    this.time = 0;
    this.adaptFirst = true;

    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 3, -1, 0, -1, 3, 0]), 3));
    geo.setAttribute('uv', new THREE.BufferAttribute(new Float32Array([0, 0, 2, 0, 0, 2]), 2));
    this.fsCam = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
    this.fsTri = new THREE.Mesh(geo);
    this.fsTri.frustumCulled = false;
    this.fsScene = new THREE.Scene();
    this.fsScene.add(this.fsTri);

    const mat = (fsh, uniforms, extra) => new THREE.ShaderMaterial({
      vertexShader: FS_VSH, fragmentShader: fsh, uniforms,
      depthTest: false, depthWrite: false, ...extra,
    });
    this.downMat = mat(DOWN_FSH, { tSrc: { value: null }, uTexel: { value: new THREE.Vector2() }, uKaris: { value: 0 } });
    this.upMat = mat(UP_FSH, { tSrc: { value: null }, uTexel: { value: new THREE.Vector2() }, uRadius: { value: 1.0 } }, {
      blending: THREE.CustomBlending, blendEquation: THREE.AddEquation,
      blendSrc: THREE.OneFactor, blendDst: THREE.OneFactor, transparent: true,
    });
    this.adaptMat = mat(ADAPT_FSH, { tSrc: { value: null }, tPrev: { value: null }, uK: { value: 1 } });
    this.compMat = mat(COMPOSITE_FSH, {
      tHdr: { value: null }, tBloom: { value: null }, tAdapt: { value: null },
      uBloom: { value: this.bloom }, uExposure: { value: 1 }, uWarp: { value: 0 },
      uWarpC: { value: this.warpC }, uTime: { value: 0 }, uAdaptK: { value: 0.85 },
    });
    const adaptOpts = { type: THREE.HalfFloatType, format: THREE.RGBAFormat, depthBuffer: false, minFilter: THREE.NearestFilter, magFilter: THREE.NearestFilter };
    this.adapt = [new THREE.WebGLRenderTarget(1, 1, adaptOpts), new THREE.WebGLRenderTarget(1, 1, adaptOpts)];

    // GPU frame time, when the browser exposes timer queries
    const gl = renderer.getContext();
    this.gl = gl;
    this.timerExt = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    this.queries = [];
    this.activeQuery = null;
    this.gpuMs = 0;
    this.gpuSampleAt = 0;
  }

  // w,h in device pixels
  setSize(w, h) {
    this.w = Math.max(2, Math.round(w));
    this.h = Math.max(2, Math.round(h));
    this._rebuild();
  }

  setQuality({ msaa, bloomLevels }) {
    const rebuild = msaa !== this.msaa || bloomLevels !== this.levels;
    this.msaa = msaa;
    this.levels = bloomLevels;
    if (rebuild && this.hdr) this._rebuild();
  }

  _rebuild() {
    if (this.hdr) this.hdr.dispose();
    for (const m of this.mips) m.dispose();
    const maxS = this.renderer.capabilities.maxSamples;
    // half-float targets need this extension; without it stay LDR rather than fail
    const hdrType = this.gl.getExtension('EXT_color_buffer_float') || this.gl.getExtension('EXT_color_buffer_half_float')
      ? THREE.HalfFloatType : THREE.UnsignedByteType;
    this.hdr = new THREE.WebGLRenderTarget(this.w, this.h, {
      type: hdrType, format: THREE.RGBAFormat,
      samples: Math.min(this.msaa, maxS),
      depthBuffer: true, stencilBuffer: false,
      minFilter: THREE.LinearFilter, magFilter: THREE.LinearFilter,
    });
    this.mips = [];
    let mw = this.w, mh = this.h;
    for (let i = 0; i < this.levels; i++) {
      mw = Math.max(2, mw >> 1); mh = Math.max(2, mh >> 1);
      this.mips.push(new THREE.WebGLRenderTarget(mw, mh, {
        type: hdrType, format: THREE.RGBAFormat, depthBuffer: false,
        minFilter: THREE.LinearFilter, magFilter: THREE.LinearFilter,
      }));
    }
  }

  _fs(material, target) {
    this.fsTri.material = material;
    this.renderer.setRenderTarget(target);
    this.renderer.render(this.fsScene, this.fsCam);
  }

  _beginTimer() {
    if (!this.timerExt || this.activeQuery) return;
    const gl = this.gl;
    const q = gl.createQuery();
    gl.beginQuery(this.timerExt.TIME_ELAPSED_EXT, q);
    this.activeQuery = q;
  }

  _endTimer() {
    if (!this.activeQuery) return;
    const gl = this.gl;
    gl.endQuery(this.timerExt.TIME_ELAPSED_EXT);
    this.queries.push(this.activeQuery);
    this.activeQuery = null;
    while (this.queries.length) {
      const q = this.queries[0];
      if (!gl.getQueryParameter(q, gl.QUERY_RESULT_AVAILABLE)) break;
      const disjoint = gl.getParameter(this.timerExt.GPU_DISJOINT_EXT);
      if (!disjoint) {
        const ms = gl.getQueryParameter(q, gl.QUERY_RESULT) / 1e6;
        this.gpuMs = this.gpuMs ? this.gpuMs + (ms - this.gpuMs) * 0.1 : ms;
        this.gpuSampleAt = performance.now();
      }
      gl.deleteQuery(q);
      this.queries.shift();
    }
  }

  render(dt) {
    const r = this.renderer;
    this.time += dt;
    this._beginTimer();
    r.autoClear = false;
    r.setRenderTarget(this.hdr);
    r.clear(true, true, false);
    r.render(this.scene, this.camera);

    // bloom down
    let src = this.hdr.texture;
    const dm = this.downMat.uniforms;
    for (let i = 0; i < this.mips.length; i++) {
      const t = this.mips[i];
      const sw = i === 0 ? this.w : this.mips[i - 1].width, sh = i === 0 ? this.h : this.mips[i - 1].height;
      dm.tSrc.value = src;
      dm.uTexel.value.set(1 / sw, 1 / sh);
      dm.uKaris.value = i === 0 ? 1 : 0;
      this._fs(this.downMat, t);
      src = t.texture;
    }
    // eye adaptation reads the smallest mip before it collects the chain
    const au = this.adaptMat.uniforms;
    au.tSrc.value = this.mips[this.mips.length - 1].texture;
    au.tPrev.value = this.adapt[0].texture;
    au.uK.value = this.adaptFirst ? 1 : 1 - Math.exp(-dt / 0.9);
    this.adaptFirst = false;
    this._fs(this.adaptMat, this.adapt[1]);
    this.adapt.reverse();
    // bloom up, adding each coarser level into the next finer one
    const um = this.upMat.uniforms;
    for (let i = this.mips.length - 1; i > 0; i--) {
      um.tSrc.value = this.mips[i].texture;
      um.uTexel.value.set(1 / this.mips[i].width, 1 / this.mips[i].height);
      this._fs(this.upMat, this.mips[i - 1]);
    }
    const cu = this.compMat.uniforms;
    cu.tHdr.value = this.hdr.texture;
    cu.tBloom.value = this.mips[0].texture;
    cu.tAdapt.value = this.adapt[0].texture;
    cu.uExposure.value = this.exposure;
    cu.uBloom.value = this.bloom * (6 / this.levels);
    cu.uWarp.value = this.warp;
    cu.uTime.value = this.time;
    this._fs(this.compMat, null);
    this._endTimer();
  }

  // brightness the eye is adapted to (for tests and the stats readout)
  adaptedLevel() {
    const buf = new Float32Array(4);
    try {
      this.renderer.readRenderTargetPixels(this.adapt[0], 0, 0, 1, 1, buf);
    } catch { return NaN; }
    return buf[0];
  }
}
