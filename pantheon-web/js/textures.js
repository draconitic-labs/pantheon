// Pantheon 3D: real imagery, decoded off the main thread and uploaded a tile per frame,
// so flying to a new world never freezes the picture. The server lists what exists,
// so a fresh clone without textures never requests a missing file and the renderer
// stays on its procedural fallbacks.
import * as THREE from 'three';

export class TextureBank {
  constructor(renderer) {
    this.renderer = renderer;
    this.gl = renderer.getContext();
    this.available = new Set();
    this.maxSize = 8192;
    this.aniso = Math.min(renderer.capabilities.getMaxAnisotropy(), 8);
    this.state = new Map();   // file -> 'loading' | 'done' | 'failed'
    this.queue = [];
    this.busy = false;
    this.active = null;       // the texture currently uploading
    this.onUpload = null;     // (file): a mipmap build is about to stall the thread
    this.onUploaded = null;   // (file, ms in total on the main thread)
    this.jobs = new Map();
    this.nextId = 1;
    try {
      this.worker = new Worker('js/decode-worker.js');
      this.worker.onmessage = (e) => {
        const job = this.jobs.get(e.data.id);
        this.jobs.delete(e.data.id);
        if (job) { if (e.data.tiles) job.resolve(e.data); else job.reject(new Error(e.data.error)); }
      };
      this.worker.onerror = () => {
        this.worker = null;
        for (const job of this.jobs.values()) job.reject(new Error('decode worker stopped'));
        this.jobs.clear();
      };
    } catch { this.worker = null; }
  }

  setAvailable(files) { this.available = new Set(files); }
  has(file) { return this.available.has(file); }
  pending() { return this.busy || this.queue.length > 0 || !!this.active; }

  // Queue a file once. urgent files jump ahead (the place you are flying to).
  request(file, onReady, { urgent = false, srgb = true, wrap = true } = {}) {
    if (!this.has(file) || this.state.has(file)) return false;
    this.state.set(file, 'loading');
    const job = { file, onReady, srgb, wrap };
    if (urgent) this.queue.unshift(job); else this.queue.push(job);
    this._pump();
    return true;
  }

  // Hurry a file that is already waiting in the queue.
  prioritize(file) {
    const i = this.queue.findIndex(j => j.file === file);
    if (i > 0) this.queue.unshift(this.queue.splice(i, 1)[0]);
  }

  _decode(file) {
    const url = new URL(`/data/textures/${file}`, location.href).href;
    if (this.worker) {
      return new Promise((resolve, reject) => {
        const id = this.nextId++;
        this.jobs.set(id, { resolve, reject });
        this.worker.postMessage({ id, url, maxSize: this.maxSize });
      }).catch(() => this._decodeHere(url));   // a worker that fails is not fatal
    }
    return this._decodeHere(url);
  }

  async _decodeHere(url) {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const raw = { premultiplyAlpha: 'none', colorSpaceConversion: 'none' };
    let bmp = await createImageBitmap(await res.blob(), { imageOrientation: 'flipY', ...raw });
    if (bmp.width > this.maxSize) {
      const small = await createImageBitmap(bmp, {
        resizeWidth: this.maxSize, resizeHeight: Math.round(bmp.height * this.maxSize / bmp.width), resizeQuality: 'high', ...raw,
      });
      bmp.close();
      bmp = small;
    }
    return { w: bmp.width, h: bmp.height, tiles: [{ x: 0, y: 0, w: bmp.width, h: bmp.height, bmp }] };
  }

  async _pump() {
    if (this.busy || this.active) return;
    const job = this.queue.shift();
    if (!job) return;
    this.busy = true;
    try {
      const d = await this._decode(job.file);
      this.active = { job, w: d.w, h: d.h, tiles: d.tiles, tex: this._allocate(d.w, d.h, job), ms: 0 };
    } catch (e) {
      console.warn(`texture ${job.file}: ${e.message}`);
      this.state.set(job.file, 'failed');
    }
    this.busy = false;
    if (!this.active) this._pump();
  }

  _allocate(w, h, job) {
    const gl = this.gl;
    const st = this.renderer.state;
    const tex = gl.createTexture();
    st.bindTexture(gl.TEXTURE_2D, tex);
    gl.texStorage2D(gl.TEXTURE_2D, Math.floor(Math.log2(Math.max(w, h))) + 1, job.srgb ? gl.SRGB8_ALPHA8 : gl.RGBA8, w, h);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, job.wrap ? gl.REPEAT : gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    const ext = gl.getExtension('EXT_texture_filter_anisotropic');
    if (ext) gl.texParameterf(gl.TEXTURE_2D, ext.TEXTURE_MAX_ANISOTROPY_EXT, this.aniso);
    st.unbindTexture();
    return tex;
  }

  // One tile per frame, then the mipmaps. Called from the frame loop.
  tick() {
    const a = this.active;
    if (!a) return;
    const gl = this.gl;
    const st = this.renderer.state;
    const t0 = performance.now();
    st.bindTexture(gl.TEXTURE_2D, a.tex);
    const t = a.tiles.shift();
    if (t) {
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
      gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
      gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, t.x, t.y, t.w, t.h, gl.RGBA, gl.UNSIGNED_BYTE, t.bmp);
      t.bmp.close();
    }
    if (a.tiles.length === 0) {
      if (this.onUpload) this.onUpload(a.job.file);
      gl.generateMipmap(gl.TEXTURE_2D);
    }
    st.unbindTexture();
    a.ms += performance.now() - t0;
    if (a.tiles.length === 0) {
      const tex = new THREE.ExternalTexture(a.tex);
      tex.userData.size = [a.w, a.h];
      this.active = null;
      this.state.set(a.job.file, 'done');
      if (this.onUploaded) this.onUploaded(a.job.file, a.ms);
      a.job.onReady(tex);
      this._pump();
    }
  }
}
