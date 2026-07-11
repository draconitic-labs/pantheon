//! Ambience: a tiny dependency-free generative audio engine.
//!
//! Streams synthesized audio through the classic winmm `waveOut` API (no
//! external crates — links the system `winmm` import library). The piece is
//! generative rather than a looped file: four slow pad chords cross-fade on a
//! ~96 s cycle, sparse pentatonic chimes ring over them, and a faint filtered
//! wash breathes underneath — so it cycles forever with no seam.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

const SR: u32 = 44_100;
const CHANNELS: u16 = 2;
const BUF_FRAMES: usize = 11_025; // 0.25 s per buffer
const N_BUFS: usize = 4;

#[repr(C)]
struct WaveFormatEx {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    cb_size: u16,
}

#[repr(C)]
struct WaveHdr {
    data: *mut u8,
    buffer_length: u32,
    bytes_recorded: u32,
    user: usize,
    flags: u32,
    loops: u32,
    next: *mut WaveHdr,
    reserved: usize,
}

type HWaveOut = isize;
const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
const WHDR_DONE: u32 = 0x0000_0001;

#[link(name = "winmm")]
extern "system" {
    fn waveOutOpen(
        ph: *mut HWaveOut,
        device: u32,
        format: *const WaveFormatEx,
        callback: usize,
        instance: usize,
        flags: u32,
    ) -> u32;
    fn waveOutPrepareHeader(h: HWaveOut, hdr: *mut WaveHdr, size: u32) -> u32;
    fn waveOutUnprepareHeader(h: HWaveOut, hdr: *mut WaveHdr, size: u32) -> u32;
    fn waveOutWrite(h: HWaveOut, hdr: *mut WaveHdr, size: u32) -> u32;
    fn waveOutReset(h: HWaveOut) -> u32;
    fn waveOutClose(h: HWaveOut) -> u32;
}

// --------------------------------------------------------------- synth

fn midi_hz(n: f64) -> f64 {
    440.0 * ((n - 69.0) / 12.0).exp2()
}

/// Four dreamy chords (midi notes), each holding ~18 s with ~6 s cross-fades.
const CHORDS: [[f64; 5]; 4] = [
    [45.0, 52.0, 57.0, 59.0, 64.0], // Asus2
    [41.0, 48.0, 53.0, 57.0, 60.0], // Fmaj
    [48.0, 55.0, 60.0, 62.0, 67.0], // Csus2
    [43.0, 50.0, 55.0, 59.0, 62.0], // G
];
const CHORD_S: f64 = 24.0; // seconds per chord slot

struct Chime {
    freq: f64,
    born: f64,
    pan: f32,
}

struct Synth {
    t: f64, // seconds
    // pad oscillator phases: [chord][note][detune 0=+,1=-]
    ph: [[[f64; 2]; 5]; 4],
    chimes: Vec<Chime>,
    next_chime: f64,
    rng: u64,
    lp1: f64,
    lp2: f64,
    gain: f32, // smoothed
}

impl Synth {
    fn new() -> Self {
        Self {
            t: 0.0,
            ph: [[[0.0; 2]; 5]; 4],
            chimes: Vec::new(),
            next_chime: 4.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            lp1: 0.0,
            lp2: 0.0,
            gain: 0.0,
        }
    }

    fn rand(&mut self) -> f64 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn fill(&mut self, out: &mut [i16], target_gain: f32) {
        let dt = 1.0 / SR as f64;
        let n = out.len() / 2;
        for i in 0..n {
            self.t += dt;
            let t = self.t;
            self.gain += (target_gain - self.gain) * 0.0004;

            // chord weights: circular distance on the 4-slot cycle
            let x = (t / CHORD_S) % 4.0;
            let (mut l, mut r) = (0.0f64, 0.0f64);
            for (ci, chord) in CHORDS.iter().enumerate() {
                let mut d = (x - ci as f64).abs();
                if d > 2.0 {
                    d = 4.0 - d;
                }
                let w = ((1.0 - d) / 0.30).clamp(0.0, 1.0);
                if w <= 0.0 {
                    // keep phases advancing cheaply skipped — they re-sync fine
                    continue;
                }
                let w = w * w * (3.0 - 2.0 * w);
                for (ni, &note) in chord.iter().enumerate() {
                    let f = midi_hz(note);
                    let amp = 0.050 / (1.0 + ni as f64 * 0.22)
                        * (1.0 + 0.10 * (t * 0.11 + ni as f64 * 1.7 + ci as f64).sin());
                    let det = 1.0 + 0.0011 * (1.0 + ni as f64 * 0.3);
                    self.ph[ci][ni][0] = (self.ph[ci][ni][0] + f * det * dt).fract();
                    self.ph[ci][ni][1] = (self.ph[ci][ni][1] + f / det * dt).fract();
                    let s0 = (self.ph[ci][ni][0] * std::f64::consts::TAU).sin();
                    let s1 = (self.ph[ci][ni][1] * std::f64::consts::TAU).sin();
                    l += w * amp * (0.65 * s0 + 0.35 * s1);
                    r += w * amp * (0.35 * s0 + 0.65 * s1);
                }
            }

            // sparse star chimes from the active chord, one octave up
            if t >= self.next_chime {
                let ci = (x.round() as usize) % 4;
                let ni = (self.rand() * 5.0) as usize % 5;
                let oct = if self.rand() < 0.3 { 24.0 } else { 12.0 };
                let pan = (self.rand() as f32) * 0.8 + 0.1;
                self.chimes.push(Chime { freq: midi_hz(CHORDS[ci][ni] + oct), born: t, pan });
                if self.chimes.len() > 5 {
                    self.chimes.remove(0);
                }
                self.next_chime = t + 3.5 + self.rand() * 8.0;
            }
            for c in &self.chimes {
                let age = t - c.born;
                if age > 8.0 {
                    continue;
                }
                let env = (-age * 0.9).exp() * (1.0 - (-age * 40.0).exp());
                let ph = c.freq * age * std::f64::consts::TAU;
                let v = env * 0.085 * (ph.sin() + 0.28 * (ph * 2.0).sin());
                l += v * (1.0 - c.pan as f64);
                r += v * c.pan as f64;
            }

            // soft breathing wash (double one-pole lowpassed noise)
            let w = self.rand() - 0.5;
            self.lp1 += 0.018 * (w - self.lp1);
            self.lp2 += 0.018 * (self.lp1 - self.lp2);
            let wash = self.lp2 * 1.4 * (0.6 + 0.4 * (t * 0.043).sin());
            l += wash;
            r += wash;

            // gentle soft-clip and output
            let g = self.gain as f64;
            let sat = |v: f64| -> i16 {
                let v = v * g;
                let v = v / (1.0 + v.abs()); // smooth limiter
                (v * 30_000.0) as i16
            };
            out[i * 2] = sat(l);
            out[i * 2 + 1] = sat(r);
        }
    }
}

// --------------------------------------------------------------- engine

pub struct Ambience {
    stop: Arc<AtomicBool>,
    gain: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Ambience {
    /// Open the default output device and start the generative stream.
    /// Returns None (silently) when no audio device is available.
    pub fn start(initial_gain: f32) -> Option<Ambience> {
        let stop = Arc::new(AtomicBool::new(false));
        let gain = Arc::new(AtomicU32::new(initial_gain.to_bits()));
        let (stop_t, gain_t) = (stop.clone(), gain.clone());

        let fmt = WaveFormatEx {
            format_tag: 1, // PCM
            channels: CHANNELS,
            samples_per_sec: SR,
            avg_bytes_per_sec: SR * CHANNELS as u32 * 2,
            block_align: CHANNELS * 2,
            bits_per_sample: 16,
            cb_size: 0,
        };
        let mut h: HWaveOut = 0;
        let rc = unsafe { waveOutOpen(&mut h, WAVE_MAPPER, &fmt, 0, 0, 0) };
        if rc != 0 {
            log::warn!("ambience: waveOutOpen failed ({rc}) — audio disabled");
            return None;
        }

        let thread = std::thread::Builder::new()
            .name("pantheon-ambience".into())
            .spawn(move || {
                let mut synth = Synth::new();
                let mut bufs: Vec<Box<[i16]>> =
                    (0..N_BUFS).map(|_| vec![0i16; BUF_FRAMES * 2].into_boxed_slice()).collect();
                let mut hdrs: Vec<WaveHdr> = bufs
                    .iter_mut()
                    .map(|b| WaveHdr {
                        data: b.as_mut_ptr() as *mut u8,
                        buffer_length: (b.len() * 2) as u32,
                        bytes_recorded: 0,
                        user: 0,
                        flags: 0,
                        loops: 0,
                        next: std::ptr::null_mut(),
                        reserved: 0,
                    })
                    .collect();
                let hsz = std::mem::size_of::<WaveHdr>() as u32;
                unsafe {
                    for (k, hdr) in hdrs.iter_mut().enumerate() {
                        let g = f32::from_bits(gain_t.load(Ordering::Relaxed));
                        synth.fill(&mut bufs[k], g);
                        waveOutPrepareHeader(h, hdr, hsz);
                        waveOutWrite(h, hdr, hsz);
                    }
                    while !stop_t.load(Ordering::Relaxed) {
                        let mut wrote = false;
                        for (k, hdr) in hdrs.iter_mut().enumerate() {
                            if hdr.flags & WHDR_DONE != 0 {
                                let g = f32::from_bits(gain_t.load(Ordering::Relaxed));
                                synth.fill(&mut bufs[k], g);
                                hdr.flags &= !WHDR_DONE;
                                waveOutWrite(h, hdr, hsz);
                                wrote = true;
                            }
                        }
                        if !wrote {
                            std::thread::sleep(std::time::Duration::from_millis(15));
                        }
                    }
                    waveOutReset(h);
                    for hdr in hdrs.iter_mut() {
                        waveOutUnprepareHeader(h, hdr, hsz);
                    }
                    waveOutClose(h);
                }
                drop(bufs);
            })
            .ok()?;
        Some(Ambience { stop, gain, thread: Some(thread) })
    }

    /// 0.0 = silent (ramped smoothly — no clicks), ~0.5 = pleasant default.
    pub fn set_gain(&self, g: f32) {
        self.gain.store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

impl Drop for Ambience {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
