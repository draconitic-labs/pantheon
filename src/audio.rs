//! Ambience: a small generative audio engine.
//!
//! The piece is generative rather than a looped file: four slow pad chords
//! cross-fade on a ~96 s cycle, sparse pentatonic chimes ring over them, and a
//! faint filtered wash breathes underneath, so it cycles forever with no seam.
//! Output goes through cpal, which talks to ALSA (and PipeWire or PulseAudio
//! through it) on Linux. The device is opened only when someone turns sound on.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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

    /// Render interleaved stereo at `sr` Hz into `out`.
    fn fill(&mut self, out: &mut [f32], target_gain: f32, sr: f64, channels: usize) {
        let dt = 1.0 / sr;
        let n = out.len() / channels;
        for i in 0..n {
            self.t += dt;
            let t = self.t;
            self.gain += (target_gain - self.gain) * (0.0004 * 44_100.0 / sr as f32);

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
            let sat = |v: f64| -> f32 {
                let v = v * g;
                (v / (1.0 + v.abs())) as f32 * 0.92 // smooth limiter
            };
            let frame = &mut out[i * channels..(i + 1) * channels];
            match frame {
                [m] => *m = sat(0.5 * (l + r)),
                [a, b, rest @ ..] => {
                    *a = sat(l);
                    *b = sat(r);
                    rest.fill(0.0);
                }
                [] => {}
            }
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
    /// Returns None when no usable audio device is available.
    pub fn start(initial_gain: f32) -> Option<Ambience> {
        let stop = Arc::new(AtomicBool::new(false));
        let gain = Arc::new(AtomicU32::new(initial_gain.to_bits()));
        let (stop_t, gain_t) = (stop.clone(), gain.clone());
        let (ready_tx, ready_rx) = channel::<bool>();
        // a cpal stream isn't Send on every platform: it lives on its own thread
        let thread = std::thread::Builder::new()
            .name("pantheon-ambience".into())
            .spawn(move || {
                let stream = match open_stream(gain_t) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("ambience unavailable: {e}");
                        let _ = ready_tx.send(false);
                        return;
                    }
                };
                let _ = ready_tx.send(true);
                while !stop_t.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(60));
                }
                drop(stream);
            })
            .ok()?;
        if ready_rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap_or(false) {
            Some(Ambience { stop, gain, thread: Some(thread) })
        } else {
            stop.store(true, Ordering::Relaxed);
            None
        }
    }

    /// 0.0 = silent (ramped smoothly, no clicks), ~0.5 = pleasant default.
    pub fn set_gain(&self, g: f32) {
        self.gain.store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

fn open_stream(gain: Arc<AtomicU32>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no default output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let sr = config.sample_rate.0 as f64;
    let channels = config.channels as usize;
    if channels == 0 {
        return Err("device reports no channels".into());
    }
    let on_err = |e| log::warn!("ambience stream: {e}");
    let mut synth = Synth::new();
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                synth.fill(out, f32::from_bits(gain.load(Ordering::Relaxed)), sr, channels);
            },
            on_err,
            None,
        ),
        cpal::SampleFormat::I16 => {
            let mut tmp: Vec<f32> = Vec::new();
            device.build_output_stream(
                &config,
                move |out: &mut [i16], _| {
                    tmp.resize(out.len(), 0.0);
                    synth.fill(&mut tmp, f32::from_bits(gain.load(Ordering::Relaxed)), sr, channels);
                    for (o, v) in out.iter_mut().zip(&tmp) {
                        *o = (*v * 32_767.0) as i16;
                    }
                },
                on_err,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let mut tmp: Vec<f32> = Vec::new();
            device.build_output_stream(
                &config,
                move |out: &mut [u16], _| {
                    tmp.resize(out.len(), 0.0);
                    synth.fill(&mut tmp, f32::from_bits(gain.load(Ordering::Relaxed)), sr, channels);
                    for (o, v) in out.iter_mut().zip(&tmp) {
                        *o = ((*v * 0.5 + 0.5) * 65_535.0) as u16;
                    }
                },
                on_err,
                None,
            )
        }
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

impl Drop for Ambience {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_makes_bounded_nonsilent_sound_and_fades_with_gain() {
        let mut s = Synth::new();
        // let the gain ramp settle, then measure several seconds at 48 kHz stereo
        let mut buf = vec![0f32; 48_000 * 2 * 4];
        s.fill(&mut buf, 0.5, 48_000.0, 2);
        s.fill(&mut buf, 0.5, 48_000.0, 2);
        let peak = buf.iter().fold(0f32, |a, v| a.max(v.abs()));
        let rms = (buf.iter().map(|v| v * v).sum::<f32>() / buf.len() as f32).sqrt();
        assert!(peak <= 1.0, "peak {peak}");
        assert!(rms > 0.005, "rms {rms}");
        assert!(buf.iter().all(|v| v.is_finite()));
        // with the gain at zero it fades to silence
        let mut quiet = vec![0f32; 48_000 * 2];
        s.fill(&mut quiet, 0.0, 48_000.0, 2);
        s.fill(&mut quiet, 0.0, 48_000.0, 2);
        let tail = quiet[quiet.len() - 4_000..].iter().fold(0f32, |a, v| a.max(v.abs()));
        assert!(tail < 1e-3, "tail {tail}");
    }

    #[test]
    fn synth_handles_mono_and_surround_layouts() {
        let mut s = Synth::new();
        let mut mono = vec![0f32; 4_800];
        s.fill(&mut mono, 0.5, 48_000.0, 1);
        let mut six = vec![0f32; 6 * 1_000];
        s.fill(&mut six, 0.5, 48_000.0, 6);
        assert!(six.chunks(6).all(|c| c[2..].iter().all(|v| *v == 0.0)));
    }
}
