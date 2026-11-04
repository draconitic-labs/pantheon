// Pantheon 3D: quality tiers and the governor that keeps the frame rate.
// Steps are uniforms and target sizes, so changing tier never recompiles a shader.

export const TIERS = {
  ultra: { label: 'Ultra', scale: 1.0, msaa: 4, bloomLevels: 7, atmo: [16, 4], vol: [52, 2], tex: 8192, particles: 1.0 },
  high: { label: 'High', scale: 1.0, msaa: 4, bloomLevels: 6, atmo: [12, 3], vol: [36, 2], tex: 8192, particles: 1.0 },
  medium: { label: 'Medium', scale: 0.85, msaa: 2, bloomLevels: 5, atmo: [9, 2], vol: [24, 3], tex: 4096, particles: 0.6 },
  low: { label: 'Low', scale: 0.7, msaa: 0, bloomLevels: 4, atmo: [6, 2], vol: [14, 4], tex: 2048, particles: 0.3 },
};
const ORDER = ['low', 'medium', 'high', 'ultra'];

export function guessTier(rendererName, software) {
  if (software) return 'low';
  if (/nvidia|geforce|rtx|gtx|radeon rx|radeon pro|arc\b/i.test(rendererName)) return 'ultra';
  if (/apple m\d|radeon/i.test(rendererName)) return 'high';
  if (/intel|uhd|iris|mali|adreno/i.test(rendererName)) return 'medium';
  return 'high';
}

// Watches real frame times. Steps down fast when frames run long, climbs back
// slowly and only with headroom.
export class Governor {
  constructor(start, apply) {
    this.mode = 'auto';      // 'auto' or a tier key
    this.tier = start;
    this.ceiling = 'ultra';  // the name of the adapter may be masked: frame times decide
    this.apply = apply;
    this.samples = [];
    this.windowStart = performance.now();
    this.quietUntil = this.windowStart + 4000;
    this.goodSince = 0;
    this.slowWindows = 0;
  }

  set(mode) {
    this.mode = mode;
    this._setTier(mode === 'auto' ? 'high' : mode);   // automatic starts high and adapts
    this.pause(2500);
  }

  // texture uploads, resizes and tab switches make long frames that are not the
  // renderer's fault; do not learn from them
  pause(ms) {
    this.quietUntil = Math.max(this.quietUntil, performance.now() + ms);
    this.samples.length = 0;
    this.windowStart = performance.now();
    this.slowWindows = 0;
  }

  _setTier(t) {
    if (t === this.tier) return;
    this.tier = t;
    this.apply(TIERS[t], t);
    this.pause(2500);
  }

  // dtMs: frame interval; workMs: smoothed render cost through glFinish
  tick(dtMs, workMs) {
    const now = performance.now();
    if (now < this.quietUntil || document.hidden) return;
    this.samples.push(dtMs);
    if (now - this.windowStart < 2000) return;
    const s = this.samples.slice().sort((a, b) => a - b);
    const p80 = s[Math.floor(s.length * 0.8)] || 16;
    this.samples.length = 0;
    this.windowStart = now;
    this.lastP80 = p80;
    if (this.mode !== 'auto') return;
    const i = ORDER.indexOf(this.tier);
    // glFinish can under-report here, so the frame interval has the last word: costly
    // frames step down fast, plainly slow ones soon, mildly slow ones if they persist
    // (a brief run of those is usually the machine busy elsewhere).
    const costly = workMs > 17 || (p80 > 21 && workMs > 11);
    if (costly || p80 > 21) {
      this.slowWindows++;
      this.goodSince = 0;
      const need = costly ? 2 : p80 > 30 ? 4 : 8;
      if (this.slowWindows >= need && i > 0) { this.slowWindows = 0; this._setTier(ORDER[i - 1]); }
    } else {
      this.slowWindows = 0;
      const headroom = workMs > 0 && workMs < 7.5 && p80 < 17.5;
      if (!headroom) { this.goodSince = 0; return; }
      if (!this.goodSince) this.goodSince = now;
      if (now - this.goodSince > 25000 && i < ORDER.indexOf(this.ceiling)) {
        this.goodSince = 0;
        this._setTier(ORDER[i + 1]);
      }
    }
  }
}
