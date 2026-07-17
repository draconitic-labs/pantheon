// Pantheon 3D — free-flight navigation.
// camWorld is the f64 world-space camera position (meters, heliocentric
// ecliptic); the three.js camera itself never leaves the scene origin.
import * as THREE from 'three';

export class Nav {
  constructor(canvas, isTyping) {
    this.canvas = canvas;
    this.isTyping = isTyping || (() => false);
    this.camWorld = new THREE.Vector3(0, 0, 0);
    this.quat = new THREE.Quaternion();
    this.keys = new Set();
    this.speedMult = 1.0;
    this.locked = false;
    this.autopilot = null;      // {targetFn, arriveDist, k, t}
    this.speedNow = 0;
    this.selectCb = null; this.travelCb = null; this.moveCb = null;

    canvas.addEventListener('click', (e) => {
      if (!this.locked) { canvas.requestPointerLock(); return; }
      if (this.selectCb) this.selectCb(0, 0);
    });
    canvas.addEventListener('dblclick', (e) => {
      const [nx, ny] = this.locked ? [0, 0] : this._ndc(e);
      if (this.travelCb) this.travelCb(nx, ny);
    });
    document.addEventListener('pointerlockchange', () => {
      this.locked = document.pointerLockElement === canvas;
    });
    document.addEventListener('mousemove', (e) => {
      if (!this.locked) return;
      const s = 0.0022;
      // Local-frame yaw/pitch (true free-fly; slight roll accumulation is
      // inherent and expected — Q/E trims it).
      const qy = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 1, 0), -e.movementX * s);
      const qx = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1, 0, 0), -e.movementY * s);
      this.quat.multiply(qy).multiply(qx).normalize();
    });
    window.addEventListener('keydown', (e) => {
      if (this.isTyping()) return;
      this.keys.add(e.code);
      if (['KeyW','KeyA','KeyS','KeyD','KeyR','KeyF'].includes(e.code)) this.autopilot = null;
    });
    window.addEventListener('keyup', (e) => this.keys.delete(e.code));
    window.addEventListener('blur', () => this.keys.clear());
    canvas.addEventListener('wheel', (e) => {
      e.preventDefault();
      const f = e.deltaY < 0 ? 1.25 : 1 / 1.25;
      this.speedMult = Math.min(1000, Math.max(0.01, this.speedMult * f));
    }, { passive: false });
  }

  _ndc(e) {
    const r = this.canvas.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * 2 - 1, -(((e.clientY - r.top) / r.height) * 2 - 1)];
  }

  onSelect(cb) { this.selectCb = cb; }
  onTravel(cb) { this.travelCb = cb; }

  // targetFn() -> {pos: Vector3 world (fresh each call — bodies move), radius}
  // lift: bends the approach out of the ecliptic so flat systems aren't
  // arrived at edge-on (0 = straight line)
  flyTo(targetFn, lift = 0) {
    const t = targetFn();
    const arrive = Math.max(t.radius * 3.5, 1);
    const dist = this.camWorld.distanceTo(t.pos);
    if (dist <= arrive * 1.05) return;
    const decades = Math.max(0.5, Math.log10(dist / arrive));
    this.autopilot = { targetFn, arriveDist: arrive, k: decades * Math.LN10 / 4.0, lift };
  }

  goDirect(pos, quat) {
    this.camWorld.copy(pos);
    if (quat) this.quat.copy(quat);
    this.autopilot = null;
  }

  lookAtWorld(target) {
    // Matrix4.lookAt(eye, target, up): camera -Z ends up pointing eye->target
    const dir = target.clone().sub(this.camWorld).normalize();
    const m = new THREE.Matrix4().lookAt(new THREE.Vector3(0, 0, 0), dir, this._up());
    this.quat.setFromRotationMatrix(m);
  }

  _up() {
    return new THREE.Vector3(0, 1, 0).applyQuaternion(this.quat);
  }

  // distSurface: distance to nearest body surface (m); returns speed (m/s)
  update(dt, distSurface) {
    if (this.autopilot) {
      const ap = this.autopilot;
      const t = ap.targetFn();
      const dir = t.pos.clone().sub(this.camWorld);
      const dist = dir.length();
      dir.divideScalar(dist || 1);
      const nd = Math.max(ap.arriveDist, dist * Math.exp(-ap.k * dt));
      const approach = ap.lift ? dir.clone().setZ(dir.z + ap.lift).normalize() : dir;
      this.camWorld.copy(t.pos).addScaledVector(approach, -nd);
      // ease gaze toward the (post-move) target direction
      const gaze = t.pos.clone().sub(this.camWorld).normalize();
      const m = new THREE.Matrix4().lookAt(new THREE.Vector3(0, 0, 0), gaze, this._up());
      const q = new THREE.Quaternion().setFromRotationMatrix(m);
      this.quat.slerp(q, Math.min(1, 3.0 * dt)).normalize();
      this.speedNow = (dist - nd) / Math.max(dt, 1e-6);
      if (nd <= ap.arriveDist * 1.01) this.autopilot = null;
      return this.speedNow;
    }
    let v = Math.max(1.0, 0.35 * distSurface) * this.speedMult;
    if (this.keys.has('ShiftLeft') || this.keys.has('ShiftRight')) v *= 8;
    const mv = new THREE.Vector3();
    if (this.keys.has('KeyW')) mv.z -= 1;
    if (this.keys.has('KeyS')) mv.z += 1;
    if (this.keys.has('KeyA')) mv.x -= 1;
    if (this.keys.has('KeyD')) mv.x += 1;
    if (this.keys.has('KeyR')) mv.y += 1;
    if (this.keys.has('KeyF')) mv.y -= 1;
    if (this.keys.has('KeyQ') || this.keys.has('KeyE')) {
      const roll = (this.keys.has('KeyQ') ? 1 : 0) - (this.keys.has('KeyE') ? 1 : 0);
      const qr = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 0, 1), roll * 1.2 * dt);
      this.quat.multiply(qr).normalize();
    }
    if (mv.lengthSq() > 0) {
      mv.normalize().applyQuaternion(this.quat);
      this.camWorld.addScaledVector(mv, v * dt);
      this.speedNow = v;
      if (this.moveCb) this.moveCb();
    } else {
      this.speedNow = 0;
    }
    return this.speedNow;
  }
}

export function prettyDist(m) {
  const AU = 1.495978707e11, PC = 3.0856775814913673e16, LY = 9.4607e15;
  if (m < 1e3) return `${m.toFixed(1)} m`;
  if (m < 1e7) return `${(m / 1e3).toFixed(1)} km`;
  if (m < 0.05 * AU) return `${Math.round(m / 1e3).toLocaleString()} km`;
  if (m < 0.1 * LY) return `${(m / AU).toFixed(m < AU ? 3 : 2)} AU`;
  if (m < 0.5 * PC) return `${(m / LY).toFixed(2)} ly`;
  if (m < 1e3 * PC) return `${(m / PC).toFixed(2)} pc`;
  if (m < 1e6 * PC) return `${(m / (1e3 * PC)).toFixed(2)} kpc`;
  return `${(m / (1e6 * PC)).toFixed(2)} Mpc`;
}

export function prettySpeed(mps) {
  const AU = 1.495978707e11, LY = 9.4607e15, C = 299792458;
  if (mps < 3e3) return `${mps.toFixed(0)} m/s`;
  if (mps < 0.02 * C) return `${(mps / 1e3).toFixed(1)} km/s`;
  if (mps < 3 * C) return `${(mps / C).toFixed(2)} c`;
  if (mps < 0.5 * AU) return `${(mps / C).toFixed(0)} c`;
  if (mps < 0.3 * LY) return `${(mps / AU).toFixed(2)} AU/s`;
  return `${(mps / LY).toFixed(2)} ly/s`;
}
