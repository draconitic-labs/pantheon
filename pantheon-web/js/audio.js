// Pantheon web — generative ambience (WebAudio port of src/audio.rs).
// Four slow pad chords cross-fade on a ~96 s cycle, sparse pentatonic chimes
// ring over them, and a faint lowpassed noise wash breathes underneath — the
// piece cycles forever with no seam. All levels are a whisper by design.
// No DOM access; the AudioContext is created lazily inside start() so the
// first call can come from a user gesture (autoplay policy).

const midiHz = (n) => 440 * Math.pow(2, (n - 69) / 12);

// Four dreamy chords (midi notes); each holds a ~24 s slot on the cycle.
const CHORDS = [
  [45, 52, 57, 59, 64], // Asus2
  [41, 48, 53, 57, 60], // Fmaj
  [48, 55, 60, 62, 67], // Csus2
  [43, 50, 55, 59, 62], // G
];
const CHORD_S = 24;   // seconds per chord slot
const PAD_TRIM = 0.4; // keeps total pad level ~0.07 (per-note amps mirror the Rust)

class Ambience {
  constructor() {
    this.ctx = null;
    this.g = 0.8;           // user gain 0..1
    this._running = false;
    this._padT = this._chimeT = this._washT = this._suspendT = 0;
  }

  get running() { return this._running; }

  _build() {
    const ctx = this.ctx = new AudioContext();
    this.master = ctx.createGain();
    this.master.gain.value = 0;
    this.master.connect(ctx.destination);

    // Pads: all 40 oscillators run permanently (free when suspended); the
    // scheduler crossfades the per-chord gains, so start/stop never respawns
    // nodes and double-start cannot duplicate voices.
    const padBus = ctx.createGain();
    padBus.gain.value = PAD_TRIM;
    padBus.connect(this.master);
    this.chordGains = [];
    this.noteGains = []; // [ci][ni], tremolo target on each
    for (let ci = 0; ci < 4; ci++) {
      const cg = ctx.createGain();
      cg.gain.value = ci === 0 ? 1 : 0;
      cg.connect(padBus);
      this.chordGains.push(cg);
      const row = [];
      for (let ni = 0; ni < 5; ni++) {
        const ng = ctx.createGain();
        ng.gain.value = 0.05 / (1 + ni * 0.22);
        ng.connect(cg);
        row.push(ng);
        // Detuned sine pair, ~2-4 cents each way (Rust det = 1+0.0011*(1+0.3ni)),
        // panned slightly apart like the Rust 0.65/0.35 L-R mix.
        const cents = 1200 * Math.log2(1 + 0.0011 * (1 + ni * 0.3));
        for (const s of [1, -1]) {
          const o = ctx.createOscillator();
          o.frequency.value = midiHz(CHORDS[ci][ni]);
          o.detune.value = s * cents;
          const p = ctx.createStereoPanner();
          p.pan.value = s * -0.3;
          o.connect(p).connect(ng);
          o.start();
        }
      }
      this.noteGains.push(row);
    }

    // Wash: looping white noise -> gentle lowpass, gain drifted by scheduler.
    const buf = ctx.createBuffer(1, ctx.sampleRate * 2, ctx.sampleRate);
    const d = buf.getChannelData(0);
    for (let i = 0; i < d.length; i++) d[i] = Math.random() * 2 - 1;
    const noise = ctx.createBufferSource();
    noise.buffer = buf;
    noise.loop = true;
    const lp = ctx.createBiquadFilter();
    lp.type = 'lowpass';
    lp.frequency.value = 220;
    lp.Q.value = 0.6;
    this.washGain = ctx.createGain();
    this.washGain.gain.value = 0.2;
    noise.connect(lp).connect(this.washGain).connect(this.master);
    noise.start();
  }

  // Chord weights: circular distance on the 4-slot cycle, smoothstepped over a
  // ~7 s window (same shape as the Rust fill loop). Reapplied every 0.5 s with
  // setTargetAtTime, so ramps are click-free and survive tab-timer jitter.
  _padTick() {
    const now = this.ctx.currentTime;
    const x = (now / CHORD_S) % 4;
    for (let ci = 0; ci < 4; ci++) {
      let dd = Math.abs(x - ci);
      if (dd > 2) dd = 4 - dd;
      let w = Math.min(1, Math.max(0, (1 - dd) / 0.30));
      w = w * w * (3 - 2 * w);
      this.chordGains[ci].gain.setTargetAtTime(w, now, 0.35);
      for (let ni = 0; ni < 5; ni++) {
        const amp = 0.05 / (1 + ni * 0.22)
          * (1 + 0.10 * Math.sin(now * 0.11 + ni * 1.7 + ci));
        this.noteGains[ci][ni].gain.setTargetAtTime(amp, now, 0.4);
      }
    }
  }

  // Sparse chime: a note of the active chord, one octave up (two 30% of the
  // time), exponential decay ~4 s, random pan.
  _chime() {
    const ctx = this.ctx;
    const now = ctx.currentTime;
    const ci = Math.round(now / CHORD_S) % 4;
    const ni = Math.floor(Math.random() * 5);
    const oct = Math.random() < 0.3 ? 24 : 12;
    const o = ctx.createOscillator();
    o.frequency.value = midiHz(CHORDS[ci][ni] + oct);
    const g = ctx.createGain();
    g.gain.setValueAtTime(0, now);
    g.gain.linearRampToValueAtTime(0.07, now + 0.03);
    g.gain.setTargetAtTime(0, now + 0.03, 1.1);
    const p = ctx.createStereoPanner();
    p.pan.value = Math.random() * 1.6 - 0.8;
    o.connect(g).connect(p).connect(this.master);
    o.start(now);
    o.stop(now + 8);
    o.onended = () => { o.disconnect(); g.disconnect(); p.disconnect(); };
  }

  _schedule() {
    this._padTick();
    this._padT = setInterval(() => this._padTick(), 500);
    const chime = () => {
      this._chimeT = setTimeout(() => { this._chime(); chime(); },
        (3.5 + Math.random() * 8) * 1000);
    };
    this._chimeT = setTimeout(() => { this._chime(); chime(); }, 4000);
    const wash = () => {
      // Slow breath: same 0.6+0.4*sin(0.043 t) LFO as the Rust, sampled as a
      // wander target every few seconds.
      const now = this.ctx.currentTime;
      this.washGain.gain.setTargetAtTime(
        0.2 * (0.6 + 0.4 * Math.sin(now * 0.043)), now, 2);
      this._washT = setTimeout(wash, 3000 + Math.random() * 3000);
    };
    wash();
  }

  _unschedule() {
    clearInterval(this._padT);
    clearTimeout(this._chimeT);
    clearTimeout(this._washT);
  }

  // setTargetAtTime computes its start value from the running automation, so
  // gain moves are always click-free without cancelScheduledValues.
  _rampMaster(v) {
    this.master.gain.setTargetAtTime(v, this.ctx.currentTime, 0.1);
  }

  start() {
    if (this._running) return;
    if (!this.ctx) this._build();
    clearTimeout(this._suspendT);
    if (this.ctx.state === 'suspended') this.ctx.resume();
    this._rampMaster(this.g);
    this._schedule();
    this._running = true;
  }

  stop() {
    if (!this._running) return;
    this._running = false;
    this._unschedule();
    this._rampMaster(0);
    this._suspendT = setTimeout(() => {
      if (!this._running) this.ctx.suspend();
    }, 600);
  }

  toggle() {
    this._running ? this.stop() : this.start();
    return this._running;
  }

  setGain(g) {
    this.g = Math.min(1, Math.max(0, g));
    if (this._running) this._rampMaster(this.g);
  }
}

export function initAmbience() {
  return new Ambience();
}
