// Pantheon 3D: navigation. camWorld is the f64 world position (meters, heliocentric
// ecliptic); the three.js camera itself never leaves the scene origin.
//
// Default is orbit mode: drag swings around the focus, scroll moves along one
// continuous log scale from the surface to the cosmic web. As the camera pulls away
// the pivot slides up a chain of anchors (moon, planet, Sun, galactic center, Local
// Group, the web), so one scroll wheel crosses every scale and scrolling back in
// returns to where you started. Flights arc out to where both ends fit, then settle.
// Free flight (WASD) is an option, never a trap.
import * as THREE from 'three';

export const FOV = 50;
const MAX_DIST = 2.6e25;

const clamp = (x, a, b) => Math.min(b, Math.max(a, x));
export const sst = (x, a, b) => { const t = clamp((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t); };
const sm5 = (t) => t * t * t * (t * (t * 6 - 15) + 10);

const _a = new THREE.Vector3(), _b = new THREE.Vector3(), _m = new THREE.Matrix4(), _q = new THREE.Quaternion();
const ZERO = new THREE.Vector3();

function lookQuat(dirFromTarget, up, out) {
  _m.lookAt(dirFromTarget, ZERO, up);
  return out.setFromRotationMatrix(_m);
}

export class Nav {
  // hooks: blocked(), onClick(nx,ny), onDouble(nx,ny), onInput(kind), surface(cam)->{dist},
  //        upAt(d, out), onFlyKey(), onLock(locked), onLockFail(), reducedMotion
  constructor(canvas, hooks) {
    this.canvas = canvas;
    this.h = hooks;
    this.mode = 'orbit';
    this.camWorld = new THREE.Vector3();
    this.quat = new THREE.Quaternion();
    this.focus = null;
    this.dir = new THREE.Vector3(0, 0, 1);
    this.lnAlt = Math.log(1e7);
    this.lnGoal = this.lnAlt;
    this.zoomRate = 10;
    this.yawVel = 0; this.pitchVel = 0;
    this.pivot = new THREE.Vector3();
    this.up = new THREE.Vector3(0, 0, 1);
    this.viewOff = new THREE.Quaternion();
    this.lookedAway = false;
    this.lookBlend = 1;
    this.blendFrom = new THREE.Quaternion();
    this.flight = null;
    this.warp = 0;
    this.keys = new Set();
    this.speedMult = 1;
    this.freeVel = new THREE.Vector3();
    this.speedNow = 0;
    this.locked = false;
    this.dragging = false;
    this.wheelHeat = 0;
    this.ridePrev = null;
    this._bind();
  }

  // ---- input ---------------------------------------------------------------
  _bind() {
    const c = this.canvas;
    this.pts = new Map();
    c.style.touchAction = 'none';
    c.addEventListener('pointerdown', (e) => {
      if (this.locked) { if (e.button === 0) this.h.onClick(0, 0); return; }
      c.setPointerCapture?.(e.pointerId);
      this.pts.set(e.pointerId, { x: e.clientX, y: e.clientY, sx: e.clientX, sy: e.clientY, btn: e.button, shift: e.shiftKey, t: performance.now(), moved: false });
      if (this.pts.size === 2) this.pinch = this._pinchDist();
      this.yawVel = this.pitchVel = 0;
    });
    c.addEventListener('pointermove', (e) => {
      if (this.locked) { this._look(e.movementX || 0, e.movementY || 0, 0.0022); return; }
      const p = this.pts.get(e.pointerId);
      if (!p) return;
      const dx = e.clientX - p.x, dy = e.clientY - p.y;
      p.x = e.clientX; p.y = e.clientY;
      if (!p.moved && Math.hypot(e.clientX - p.sx, e.clientY - p.sy) < 5) return;
      if (!p.moved) { p.moved = true; this.dragging = true; this.h.onInput('drag'); if (this.flight) this.cancelFlight(); }
      if (this.pts.size === 2) {
        const d = this._pinchDist();
        if (this.pinch > 0 && d > 0) this.zoomBy(-Math.log(d / this.pinch) * 1.6);
        this.pinch = d;
        return;
      }
      if (p.btn === 2 || p.shift) this._lookOffset(dx, dy);
      else if (this.mode === 'free') this._look(dx, dy, 0.0032);
      else this._orbit(dx, dy);
    });
    const end = (e) => {
      const p = this.pts.get(e.pointerId);
      if (!p) return;
      this.pts.delete(e.pointerId);
      if (this.pts.size === 0) this.dragging = false;
      if (e.type === 'pointerup' && !p.moved && p.btn === 0 && performance.now() - p.t < 600) {
        const [nx, ny] = this._ndc(e);
        this.h.onClick(nx, ny);
      }
    };
    c.addEventListener('pointerup', end);
    c.addEventListener('pointercancel', end);
    c.addEventListener('dblclick', (e) => { const [nx, ny] = this._ndc(e); this.h.onDouble(nx, ny); });
    // WebKitGTK's own context menu would otherwise pop up over the scene
    window.addEventListener('contextmenu', (e) => e.preventDefault());
    c.addEventListener('wheel', (e) => {
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 33 : e.deltaMode === 2 ? 400 : 1;
      const dy = e.deltaY * unit * (e.ctrlKey ? 6 : 1);
      this.h.onInput('zoom');
      if (this.mode === 'free') {
        this.speedMult = clamp(this.speedMult * Math.exp(-dy * 0.0022), 0.02, 200);
        this.h.onSpeed?.(this.speedMult);
        return;
      }
      if (this.flight) this.cancelFlight();
      // a fast spin crosses whole scales; a single notch stays gentle
      this.wheelHeat = Math.min(this.wheelHeat + Math.min(Math.abs(dy), 300) / 100, 6);
      this.zoomBy((dy / 100) * 0.26 * (1 + this.wheelHeat * 0.6));
    }, { passive: false });
    document.addEventListener('pointerlockchange', () => {
      this.locked = document.pointerLockElement === c;
      this.h.onLock?.(this.locked);
    });
    document.addEventListener('pointerlockerror', () => this.h.onLockFail?.());
    window.addEventListener('keydown', (e) => {
      if (this.h.blocked() || e.ctrlKey || e.metaKey || e.altKey) return;
      const code = e.code;
      this.keys.add(code);
      if (/^Arrow/.test(code)) e.preventDefault();
      if (['KeyW', 'KeyA', 'KeyS', 'KeyD', 'KeyQ', 'KeyE'].includes(code)) {
        this.h.onInput('fly');
        if (this.mode !== 'free') this.h.onFlyKey?.();
      } else if (/^Arrow/.test(code) || code === 'Equal' || code === 'Minus') {
        this.h.onInput('key');
        if (this.flight) this.cancelFlight();
      }
    });
    window.addEventListener('keyup', (e) => this.keys.delete(e.code));
    const clear = () => { this.keys.clear(); this.dragging = false; this.pts.clear(); };
    window.addEventListener('blur', clear);
    document.addEventListener('visibilitychange', clear);
  }

  _pinchDist() {
    const v = [...this.pts.values()];
    return v.length >= 2 ? Math.hypot(v[0].x - v[1].x, v[0].y - v[1].y) : 0;
  }

  _ndc(e) {
    const r = this.canvas.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * 2 - 1, -(((e.clientY - r.top) / r.height) * 2 - 1)];
  }

  // ---- orbit -----------------------------------------------------------------
  _minAlt() {
    const f = this.focus;
    return f ? (f.minAlt ?? Math.max(f.radius * 0.012, 50)) : 50;
  }

  _dist() { return (this.focus ? this.focus.radius : 0) + Math.exp(this.lnAlt); }

  // near the ground the same drag covers the same ground, not the same angle
  _orbitK() {
    const d = this._dist();
    return clamp(Math.exp(this.lnAlt) / d * 1.2, 0.025, 1) * 0.0048;
  }

  _orbit(dx, dy) {
    const k = this._orbitK();
    this._turn(-dx * k, -dy * k);
    this.yawVel = -dx * k * 30; this.pitchVel = -dy * k * 30;
  }

  _turn(yaw, pitch) {
    const up = this.up;
    this.dir.applyQuaternion(_q.setFromAxisAngle(up, yaw));
    const elev = Math.asin(clamp(this.dir.dot(up), -1, 1));
    const lim = 1.53;
    const next = clamp(elev + pitch, -lim, lim);
    const right = _a.crossVectors(up, this.dir);
    if (right.lengthSq() > 1e-10) {
      right.normalize();
      this.dir.applyQuaternion(_q.setFromAxisAngle(right, -(next - elev)));
    }
    this.dir.normalize();
  }

  _lookOffset(dx, dy) {
    const s = 0.0024;
    const qy = new THREE.Quaternion().setFromAxisAngle(_a.set(0, 1, 0), -dx * s);
    const qx = new THREE.Quaternion().setFromAxisAngle(_a.set(1, 0, 0), -dy * s);
    if (this.mode === 'free') { this.quat.multiply(qy).multiply(qx).normalize(); return; }
    this.viewOff.multiply(qy).multiply(qx).normalize();
    this.lookedAway = true;
  }

  recenter() {
    if (!this.lookedAway) return;
    this.blendFrom.copy(this.quat);
    this.lookBlend = 0;
    this.viewOff.identity();
    this.lookedAway = false;
  }

  _look(mx, my, s) {
    const qy = new THREE.Quaternion().setFromAxisAngle(_a.set(0, 1, 0), -mx * s);
    const qx = new THREE.Quaternion().setFromAxisAngle(_a.set(1, 0, 0), -my * s);
    this.quat.multiply(qy).multiply(qx).normalize();
  }

  // positive = out. Zooming out also tilts the view toward each scale's natural
  // vantage (above the ecliptic, then above the galaxy), so flat systems never
  // arrive edge-on.
  zoomBy(dLn) {
    if (!this.focus) return;
    const lo = Math.log(this._minAlt());
    const hi = Math.log(MAX_DIST);
    const prev = this.lnGoal;
    this.lnGoal = clamp(this.lnGoal + dLn, lo, hi);
    const applied = this.lnGoal - prev;
    if (applied > 0) {
      const d = Math.exp(this.lnGoal);
      const band = sst(Math.log10(d), 10.3, 11.3) * (1 - sst(Math.log10(d), 14, 15))
        + sst(Math.log10(d), 19.5, 20.5) * (1 - sst(Math.log10(d), 22.5, 23.5));
      if (band > 0.001) {
        const target = (d < 1e16 ? 26 : 40) * Math.PI / 180;
        const elev = Math.asin(clamp(this.dir.dot(this.up), -1, 1));
        if (Math.abs(elev) < target) {
          const sign = elev >= 0 ? 1 : -1;
          const step = (target * sign - elev) * Math.min(applied * 0.22 * band, 1);
          this._turn(0, step);
        }
      }
    }
  }

  _pivotAt(d, out) {
    const f = this.focus;
    const start = f.pos();
    out.copy(start);
    let prev = start;
    for (const anchor of f.chain || []) {
      const P = anchor.pos();
      const S = prev.distanceTo(P);
      if (S > 0) out.lerp(P, sst(Math.log(d / S), Math.log(0.25), Math.log(5)));
      prev = P;
    }
    return out;
  }

  // pixels-free description of where the view is centered, for tests and the HUD
  dist() { return this.camWorld.distanceTo(this.pivot); }

  _placeOrbit(dt) {
    const d = this._dist();
    this._pivotAt(d, this.pivot);
    this.h.upAt(d, this.up);
    this.camWorld.copy(this.pivot).addScaledVector(this.dir, d);
    const look = lookQuat(this.dir, this.up, new THREE.Quaternion());
    // low over a surface, lift the gaze toward the horizon: straight down is a flat map
    const R = this.focus.radius || 0;
    if (R > 0) {
      const alpha = Math.asin(Math.min(R / d, 1));
      const tilt = sst(alpha, 0.49, 1.05) * (alpha - 0.21);
      if (tilt > 0) look.multiply(_q.setFromAxisAngle(_a.set(1, 0, 0), tilt));
    }
    if (this.lookBlend < 1) {
      this.lookBlend = Math.min(1, this.lookBlend + dt / 0.7);
      this.quat.slerpQuaternions(this.blendFrom, look, sm5(this.lookBlend));
    } else {
      this.quat.copy(look);
    }
    if (this.lookedAway) this.quat.multiply(this.viewOff);
  }

  // Make t the focus without moving the camera: the view eases over to it.
  retarget(t) {
    this.focus = t;
    const R = t.radius || 0;
    let d = Math.max(this.camWorld.distanceTo(t.pos()), 1);
    for (let i = 0; i < 4; i++) { this._pivotAt(d, this.pivot); d = Math.max(this.camWorld.distanceTo(this.pivot), 1); }
    this.dir.copy(this.camWorld).sub(this.pivot).divideScalar(d);
    this.lnAlt = this.lnGoal = Math.log(Math.max(d - R, this._minAlt()));
    this.blendFrom.copy(this.quat);
    this.lookBlend = 0;
    this.viewOff.identity();
    this.lookedAway = false;
    this.yawVel = this.pitchVel = 0;
  }

  // Place the camera on target at once, facing it (boot and tests).
  place(t, alt, dir) {
    this.focus = t;
    this.dir.copy(dir).normalize();
    this.lnAlt = this.lnGoal = Math.log(Math.max(alt, this._minAlt()));
    this.lookBlend = 1;
    this.viewOff.identity();
    this.lookedAway = false;
    this.flight = null;
    this.mode = 'orbit';
    this._placeOrbit(0);
  }

  setMode(m) {
    if (m === this.mode) return;
    if (this.flight) this.cancelFlight();
    this.mode = m;
    this.freeVel.set(0, 0, 0);
    this.ridePrev = null;
    this.yawVel = this.pitchVel = 0;
    if (m === 'orbit' && this.locked) document.exitPointerLock?.();
    if (m === 'free') { this.viewOff.identity(); this.lookedAway = false; }
  }

  lockMouse() {
    try {
      const r = this.canvas.requestPointerLock?.();
      if (r && r.catch) r.catch(() => this.h.onLockFail?.());
    } catch { this.h.onLockFail?.(); }
  }

  // ---- flights -----------------------------------------------------------------
  arriveDistance(t, aspect) {
    const half = (FOV / 2) * Math.PI / 180;
    const fill = (t.fill ?? 0.62) * Math.min(1, aspect);
    return t.arrive ?? ((t.frame ?? t.radius) / Math.tan(half * fill));
  }

  flyTo(t, { speed = 1, aspect = 1.6 } = {}) {
    const cam = this.camWorld;
    let p0;
    if (this.mode === 'orbit' && this.focus) {
      p0 = this.pivot.clone();
    } else {
      const ahead = new THREE.Vector3(0, 0, -1).applyQuaternion(this.quat);
      const reach = Math.max(this.h.surface(cam).dist, 1e3);
      p0 = cam.clone().addScaledVector(ahead, reach);
    }
    if (this.mode !== 'orbit') {
      this.mode = 'orbit';
      this.freeVel.set(0, 0, 0);
      if (this.locked) document.exitPointerLock?.();
    }
    const d0 = Math.max(cam.distanceTo(p0), 1);
    const u0 = cam.clone().sub(p0).divideScalar(d0);
    const T = t.pos();
    const d1 = this.arriveDistance(t, aspect);
    const u1 = t.arrivalDir ? t.arrivalDir(u0) : u0.clone();
    const sep = T.distanceTo(p0);
    const peak = Math.max(sep * 0.8, d0, d1);
    const L0 = Math.log(d0), L1 = Math.log(d1), LP = Math.log(peak);
    const Lc = 2 * LP - (L0 + L1) / 2;
    const decades = (2 * LP - L0 - L1) / Math.LN10;
    const reduced = this.h.reducedMotion?.();
    const dur = clamp(2.6 + 0.4 * decades, 2.6, 10) * speed * (reduced ? 0.5 : 1);
    const qRot = new THREE.Quaternion().setFromUnitVectors(u0, u1);
    this.flight = {
      t, x: 0, dur, p0, u0, qRot, L0, Lc, L1, q0: this.quat.clone(), prevL: L0,
      lift: peak > 3 * Math.max(d0, d1) ? 0.35 : 0, decades,
    };
    this.yawVel = this.pitchVel = 0;
    this.viewOff.identity();
    this.lookedAway = false;
  }

  cancelFlight() {
    const f = this.flight;
    if (!f) return;
    this.flight = null;
    this.warp = 0;
    this.retarget(f.t);    // keep the camera, keep the destination as the thing you orbit
  }

  _stepFlight(dt) {
    const f = this.flight;
    f.x = Math.min(1, f.x + dt / f.dur);
    const s = sm5(f.x);
    const T = f.t.pos();
    const g = sst(s, 0.04, 0.96);
    const p = _a.copy(f.p0).lerp(T, g);
    const lnD = (1 - s) * (1 - s) * f.L0 + 2 * s * (1 - s) * f.Lc + s * s * f.L1;
    const d = Math.exp(lnD);
    this.h.upAt(d, this.up);
    const u = _b.copy(f.u0).applyQuaternion(_q.identity().slerp(f.qRot, sst(s, 0.12, 1)));
    if (f.lift) u.addScaledVector(this.up, f.lift * 4 * s * (1 - s)).normalize();
    this.dir.copy(u);
    this.pivot.copy(p);
    this.camWorld.copy(p).addScaledVector(u, d);
    const look = lookQuat(u, this.up, new THREE.Quaternion());
    this.quat.slerpQuaternions(f.q0, look, sst(s, 0, 0.2));
    // speed across the log scale drives the streaks
    const rate = Math.abs(lnD - f.prevL) / Math.max(dt, 1e-3);
    f.prevL = lnD;
    const want = this.h.reducedMotion?.() ? 0 : clamp((rate - 1.6) / 3.4, 0, 1);
    this.warp += (want - this.warp) * (1 - Math.exp(-dt * 5));
    if (f.x >= 1) {
      this.flight = null;
      this.warp = 0;
      this.focus = f.t;
      const R = f.t.radius || 0;
      this.lnAlt = this.lnGoal = Math.log(Math.max(d - R, this._minAlt()));
      this.lookBlend = 1;
      this.h.onArrive?.(f.t);
    }
  }

  // ---- free flight ---------------------------------------------------------------
  _stepFree(dt) {
    if (this.focus) {
      const fp = this.focus.pos();
      if (this.ridePrev) {
        const R = this.focus.radius || 0;
        if (R > 0 && this.camWorld.distanceTo(fp) < R * 400) this.camWorld.add(_a.copy(fp).sub(this.ridePrev));
      }
      this.ridePrev = fp.clone();
    }
    const near = this.h.surface(this.camWorld);
    let v = Math.max(1, 0.55 * near.dist) * this.speedMult;
    if (this.keys.has('ShiftLeft') || this.keys.has('ShiftRight')) v *= 6;
    const mv = _a.set(0, 0, 0);
    if (this.keys.has('KeyW') || this.keys.has('ArrowUp')) mv.z -= 1;
    if (this.keys.has('KeyS') || this.keys.has('ArrowDown')) mv.z += 1;
    if (this.keys.has('KeyA') || this.keys.has('ArrowLeft')) mv.x -= 1;
    if (this.keys.has('KeyD') || this.keys.has('ArrowRight')) mv.x += 1;
    if (this.keys.has('KeyE')) mv.y += 1;
    if (this.keys.has('KeyQ')) mv.y -= 1;
    const roll = (this.keys.has('KeyZ') ? 1 : 0) - (this.keys.has('KeyC') ? 1 : 0);
    if (roll) this.quat.multiply(_q.setFromAxisAngle(_b.set(0, 0, 1), roll * 1.2 * dt)).normalize();
    const want = mv.lengthSq() > 0 ? mv.normalize().applyQuaternion(this.quat).multiplyScalar(v) : mv;
    this.freeVel.lerp(want, 1 - Math.exp(-dt * 5));
    const sp = this.freeVel.length();
    if (sp > v * 1.2) this.freeVel.multiplyScalar(v * 1.2 / sp);
    this.camWorld.addScaledVector(this.freeVel, dt);
    this.speedNow = this.freeVel.length();
  }

  update(dt) {
    this.wheelHeat *= Math.exp(-dt * 3);
    if (this.flight) { this._stepFlight(dt); return; }
    if (this.mode === 'free') { this._stepFree(dt); return; }
    // keyboard orbit and zoom
    const k = 1.2 * dt * Math.min(1, this._orbitK() / 0.0048);
    if (this.keys.has('ArrowLeft')) this._turn(-k, 0);
    if (this.keys.has('ArrowRight')) this._turn(k, 0);
    if (this.keys.has('ArrowUp')) this._turn(0, k);
    if (this.keys.has('ArrowDown')) this._turn(0, -k);
    if (this.keys.has('Equal')) this.zoomBy(-1.6 * dt);
    if (this.keys.has('Minus')) this.zoomBy(1.6 * dt);
    // inertia after a drag
    if (!this.dragging && (this.yawVel || this.pitchVel)) {
      this._turn(this.yawVel * dt, this.pitchVel * dt);
      const decay = Math.exp(-dt * 4.5);
      this.yawVel *= decay; this.pitchVel *= decay;
      if (Math.abs(this.yawVel) + Math.abs(this.pitchVel) < 1e-3) this.yawVel = this.pitchVel = 0;
    }
    if (this.focus && this.autoSpin) this._turn(this.autoSpin * dt, 0);
    this.lnAlt += (this.lnGoal - this.lnAlt) * (1 - Math.exp(-dt * this.zoomRate));
    this.speedNow = 0;
    if (this.focus) this._placeOrbit(dt);
  }
}

// ---- plain-language distances -----------------------------------------------------
const AU = 1.495978707e11, LY = 9.4607304725808e15;
const grp = (n, d = 0) => n.toLocaleString('en-US', { maximumFractionDigits: d, minimumFractionDigits: d });
const sig = (n) => (n >= 100 ? grp(n) : n >= 10 ? grp(n, 1) : grp(n, 2));

export function prettyDist(m) {
  if (m < 1e3) return `${grp(m)} m`;
  if (m < 1e9) return `${grp(m / 1e3)} km`;
  if (m < 0.1 * AU) return `${sig(m / 1e9)} million km`;
  if (m < 0.2 * LY) return `${sig(m / AU)} AU`;
  const ly = m / LY;
  if (ly < 1e4) return `${ly < 100 ? sig(ly) : grp(ly)} light-years`;
  if (ly < 1e6) return `${grp(Math.round(ly / 100) * 100)} light-years`;
  if (ly < 1e9) return `${sig(ly / 1e6)} million light-years`;
  return `${sig(ly / 1e9)} billion light-years`;
}

export function prettySpeed(mps) {
  const C = 299792458;
  if (mps < 3e3) return `${grp(mps)} m/s`;
  if (mps < 0.5 * C) return `${grp(mps / 1e3)} km/s`;
  if (mps < 0.2 * AU) return `${sig(mps / C)} times light speed`;
  if (mps < 0.2 * LY) return `${sig(mps / AU)} AU per second`;
  return `${prettyDist(mps)} per second`;
}

// A round length that fits maxM, with its label: 1, 2 or 5 of the natural unit.
export function niceBar(maxM) {
  let unit, name;
  if (maxM < 2e9) { unit = 1e3; name = (v) => `${grp(v)} km`; }
  else if (maxM < 0.1 * AU) { unit = 1e9; name = (v) => `${grp(v)} million km`; }
  else if (maxM < 0.2 * LY) { unit = AU; name = (v) => `${grp(v)} AU`; }
  else if (maxM < 2e6 * LY) { unit = LY; name = (v) => `${grp(v)} light-year${v === 1 ? '' : 's'}`; }
  else { unit = 1e6 * LY; name = (v) => `${grp(v)} million light-years`; }
  const v = maxM / unit;
  const p = Math.pow(10, Math.floor(Math.log10(Math.max(v, 1e-9))));
  const f = v / p;
  const n = (f >= 5 ? 5 : f >= 2 ? 2 : 1) * p;
  return { meters: n * unit, label: name(n) };
}
