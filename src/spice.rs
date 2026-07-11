//! SPICE foundation (audit EPH-004 / plan "SPICE kernel foundation"):
//! a dependency-free reader for NAIF kernels.
//!
//! - `SpkFile`: binary DAF/SPK (little-endian) with Type 2 (Chebyshev
//!   position) and Type 3 (Chebyshev position+velocity) segments — enough for
//!   the planetary DE440s kernel. States chain through segment centers to the
//!   solar-system barycenter exactly as CSPICE would.
//! - `Lsk`: leap-second kernel (DELTA_AT table) for proper UTC↔TT/TDB.
//!
//! `sim::body_pos` prefers these states inside kernel coverage (1849–2150 for
//! de440s) and falls back to the analytic elements outside it; the inspector
//! reports which source produced the shown state.

use std::path::Path;

use glam::DVec3;

pub const J2000_JD: f64 = 2_451_545.0;

// ------------------------------------------------------------------ SPK/DAF

struct Segment {
    target: i32,
    center: i32,
    dtype: i32,
    t0: f64, // ET seconds past J2000
    t1: f64,
    init: f64,
    intlen: f64,
    rsize: usize,
    n: usize,
    begin_word: usize, // 1-based DAF word address
}

pub struct SpkFile {
    buf: Vec<u8>,
    segments: Vec<Segment>,
}

fn f64_at(buf: &[u8], word: usize) -> f64 {
    let off = (word - 1) * 8;
    f64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

fn i32_at_byte(buf: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

impl SpkFile {
    pub fn load(path: &Path) -> Result<SpkFile, String> {
        let buf = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if buf.len() < 1024 || &buf[0..8] != b"DAF/SPK " {
            return Err("not a DAF/SPK file".into());
        }
        let nd = i32_at_byte(&buf, 8);
        let ni = i32_at_byte(&buf, 12);
        if nd != 2 || ni != 6 {
            return Err(format!("unexpected DAF ND/NI = {nd}/{ni}"));
        }
        let fward = i32_at_byte(&buf, 76) as usize;
        if &buf[88..96] != b"LTL-IEEE" {
            return Err("only little-endian (LTL-IEEE) kernels supported".into());
        }
        let ss = (nd + (ni + 1) / 2) as usize; // doubles per summary = 5
        let mut segments = Vec::new();
        let mut rec = fward;
        while rec != 0 {
            let base = (rec - 1) * 1024;
            let next = f64::from_le_bytes(buf[base..base + 8].try_into().unwrap()) as usize;
            let nsum = f64::from_le_bytes(buf[base + 16..base + 24].try_into().unwrap()) as usize;
            for k in 0..nsum {
                let s = base + 24 + k * ss * 8;
                let t0 = f64::from_le_bytes(buf[s..s + 8].try_into().unwrap());
                let t1 = f64::from_le_bytes(buf[s + 8..s + 16].try_into().unwrap());
                let target = i32_at_byte(&buf, s + 16);
                let center = i32_at_byte(&buf, s + 20);
                let _frame = i32_at_byte(&buf, s + 24);
                let dtype = i32_at_byte(&buf, s + 28);
                let begin_word = i32_at_byte(&buf, s + 32) as usize;
                let end_word = i32_at_byte(&buf, s + 36) as usize;
                if dtype != 2 && dtype != 3 {
                    continue; // planetary DE kernels are Type 2/3
                }
                // segment trailer: INIT, INTLEN, RSIZE, N
                let init = f64_at(&buf, end_word - 3);
                let intlen = f64_at(&buf, end_word - 2);
                let rsize = f64_at(&buf, end_word - 1) as usize;
                let n = f64_at(&buf, end_word) as usize;
                segments.push(Segment {
                    target,
                    center,
                    dtype,
                    t0,
                    t1,
                    init,
                    intlen,
                    rsize,
                    n,
                    begin_word,
                });
            }
            rec = next;
        }
        if segments.is_empty() {
            return Err("no Type 2/3 segments found".into());
        }
        Ok(SpkFile { buf, segments })
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Common coverage (ET s) across the planetary segments.
    pub fn coverage_et(&self) -> (f64, f64) {
        let mut lo = f64::MIN;
        let mut hi = f64::MAX;
        for s in &self.segments {
            lo = lo.max(s.t0);
            hi = hi.min(s.t1);
        }
        (lo, hi)
    }

    /// Position of `target` relative to `center` (its DAF center) in km,
    /// J2000 equatorial frame — one segment, Chebyshev evaluated.
    fn eval_segment(&self, seg: &Segment, et: f64) -> DVec3 {
        let idx = (((et - seg.init) / seg.intlen) as usize).min(seg.n - 1);
        let rec = seg.begin_word + idx * seg.rsize;
        let mid = f64_at(&self.buf, rec);
        let radius = f64_at(&self.buf, rec + 1);
        let sets = if seg.dtype == 2 { 3 } else { 6 };
        let ncoef = (seg.rsize - 2) / sets;
        let tau = ((et - mid) / radius).clamp(-1.0, 1.0);
        let mut out = [0.0f64; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let base = rec + 2 + c * ncoef;
            // Chebyshev recurrence
            let (mut t0, mut t1) = (1.0f64, tau);
            let mut acc = f64_at(&self.buf, base);
            if ncoef > 1 {
                acc += f64_at(&self.buf, base + 1) * t1;
            }
            for k in 2..ncoef {
                let t2 = 2.0 * tau * t1 - t0;
                acc += f64_at(&self.buf, base + k) * t2;
                t0 = t1;
                t1 = t2;
            }
            *o = acc;
        }
        DVec3::new(out[0], out[1], out[2])
    }

    fn find(&self, target: i32, et: f64) -> Option<&Segment> {
        self.segments.iter().find(|s| s.target == target && et >= s.t0 && et <= s.t1)
    }
}

// --------------------------------------------------------------------- LSK

pub struct Lsk {
    /// (jd_utc effective date, ΔAT seconds), ascending
    pub entries: Vec<(f64, f64)>,
}

impl Lsk {
    pub fn load(path: &Path) -> Result<Lsk, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{e}"))?;
        let months = [
            "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
        ];
        let mut entries = Vec::new();
        // pairs look like:  37,   @2017-JAN-1
        let mut tokens = text.split(&[',', '(', ')', '\n', '\r', ' ', '\t'][..])
            .filter(|t| !t.is_empty());
        let mut pending: Option<f64> = None;
        while let Some(tok) = tokens.next() {
            if let Some(date) = tok.strip_prefix('@') {
                if let Some(dat) = pending.take() {
                    let parts: Vec<&str> = date.split('-').collect();
                    if parts.len() == 3 {
                        let y: i32 = parts[0].parse().unwrap_or(0);
                        let m = months
                            .iter()
                            .position(|&mn| mn.eq_ignore_ascii_case(parts[1]))
                            .map(|k| k as i32 + 1)
                            .unwrap_or(1);
                        let d: f64 = parts[2].parse().unwrap_or(1.0);
                        entries.push((crate::sim::cal_to_jd(y, m, d) - 0.5, dat));
                    }
                }
            } else if let Ok(v) = tok.parse::<f64>() {
                pending = Some(v);
            } else {
                pending = None;
            }
        }
        entries.retain(|(jd, dat)| *jd > 2_400_000.0 && (*dat >= 10.0 && *dat < 120.0));
        entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        entries.dedup_by(|a, b| (a.0 - b.0).abs() < 0.5);
        if entries.len() < 20 {
            return Err(format!("leap-second table too short ({})", entries.len()));
        }
        Ok(Lsk { entries })
    }

    /// ΔAT (TAI−UTC) at a UTC julian date.
    pub fn delta_at(&self, jd_utc: f64) -> f64 {
        let mut dat = self.entries.first().map(|e| e.1).unwrap_or(37.0);
        for (jd, v) in &self.entries {
            if jd_utc >= *jd {
                dat = *v;
            } else {
                break;
            }
        }
        dat
    }

    /// TT − UTC seconds at a UTC julian date.
    pub fn tt_minus_utc(&self, jd_utc: f64) -> f64 {
        32.184 + self.delta_at(jd_utc)
    }
}

// ---------------------------------------------------------------- provider

pub struct SpiceSet {
    /// every .bsp in the kernel directory: (file name, parsed DAF)
    pub spks: Vec<(String, SpkFile)>,
    pub lsk: Option<Lsk>,
    /// planetary (de440s) coverage as TDB julian dates
    pub jd0: f64,
    pub jd1: f64,
}

impl SpiceSet {
    pub fn load(dir: &Path) -> Result<SpiceSet, String> {
        let mut spks: Vec<(String, SpkFile)> = Vec::new();
        let mut planetary: Option<(f64, f64)> = None;
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if !name.ends_with(".bsp") {
                continue;
            }
            match SpkFile::load(&path) {
                Ok(spk) => {
                    if name.starts_with("de440") {
                        planetary = Some(spk.coverage_et());
                    }
                    spks.push((name, spk));
                }
                Err(e) => log::warn!("spice: skipping {name}: {e}"),
            }
        }
        let (et0, et1) = planetary.ok_or("de440s.bsp missing (planetary states)")?;
        // planetary kernel first so common lookups resolve in one probe
        spks.sort_by_key(|(n, _)| !n.starts_with("de440") as u8);
        let lsk = Lsk::load(&dir.join("naif0012.tls")).ok();
        Ok(SpiceSet {
            spks,
            lsk,
            jd0: J2000_JD + et0 / 86_400.0,
            jd1: J2000_JD + et1 / 86_400.0,
        })
    }

    pub fn covers(&self, jd_tdb: f64) -> bool {
        jd_tdb >= self.jd0 && jd_tdb <= self.jd1
    }

    /// Whether any loaded kernel carries `target` at this epoch.
    pub fn has(&self, target: i32, jd_tdb: f64) -> bool {
        let et = (jd_tdb - J2000_JD) * 86_400.0;
        self.spks.iter().any(|(_, s)| s.find(target, et).is_some())
    }

    /// Position of `target` relative to the solar-system barycenter (km,
    /// J2000 equatorial), chaining centers across ALL loaded kernels — a
    /// Galilean resolves moon→jupiter-barycenter (jup365) →SSB (de440s).
    fn pos_ssb(&self, target: i32, et: f64) -> Option<DVec3> {
        if target == 0 {
            return Some(DVec3::ZERO);
        }
        for (_, spk) in &self.spks {
            if let Some(seg) = spk.find(target, et) {
                let rel = spk.eval_segment(seg, et);
                return Some(rel + self.pos_ssb(seg.center, et)?);
            }
        }
        None
    }

    /// Heliocentric ecliptic-J2000 position in meters (the app root frame).
    pub fn helio_ecl_m(&self, naif_id: i32, jd_tdb: f64) -> Option<DVec3> {
        let et = (jd_tdb - J2000_JD) * 86_400.0;
        let t = self.pos_ssb(naif_id, et)?;
        let sun = self.pos_ssb(10, et)?;
        Some(crate::frames::ecl_from_icrs() * ((t - sun) * 1_000.0))
    }

    /// Position of `target` relative to `center` (both NAIF ids), ecliptic m.
    pub fn rel_ecl_m(&self, target: i32, center: i32, jd_tdb: f64) -> Option<DVec3> {
        let et = (jd_tdb - J2000_JD) * 86_400.0;
        let t = self.pos_ssb(target, et)?;
        let c = self.pos_ssb(center, et)?;
        Some(crate::frames::ecl_from_icrs() * ((t - c) * 1_000.0))
    }

    pub fn kernel_summary(&self) -> String {
        self.spks
            .iter()
            .map(|(n, s)| format!("{n} ({} seg)", s.segment_count()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

static SPICE: std::sync::OnceLock<Option<SpiceSet>> = std::sync::OnceLock::new();

/// The globally loaded kernel set (None when kernels are absent — every
/// consumer must have an analytic fallback).
pub fn spice() -> Option<&'static SpiceSet> {
    SPICE
        .get_or_init(|| {
            let dir = crate::data::asset_root().join("data/spice");
            match SpiceSet::load(&dir) {
                Ok(s) => {
                    log::info!("spice: loaded {} · JD {:.1}–{:.1}", s.kernel_summary(), s.jd0, s.jd1);
                    Some(s)
                }
                Err(e) => {
                    log::warn!("spice: unavailable ({e}) — analytic elements only");
                    None
                }
            }
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::J2000;

    #[test]
    fn lsk_parses_known_epochs() {
        let Some(s) = spice() else { return }; // kernels optional in CI
        let Some(lsk) = &s.lsk else { panic!("lsk should load") };
        // 2020: ΔAT = 37 → TT−UTC = 69.184 ; 1999: ΔAT = 32
        assert!((lsk.tt_minus_utc(J2000 + 7_500.0) - 69.184).abs() < 1e-9);
        assert!((lsk.tt_minus_utc(J2000 - 360.0) - 64.184).abs() < 1e-9);
        assert!(lsk.entries.len() >= 28);
    }

    /// THE gate: DE440s must reproduce the independently cached Horizons
    /// vectors at their exact sample epochs (same time scale, same frame
    /// chain). Kilometer-level agreement or the reader is wrong.
    #[test]
    fn de440_matches_cached_horizons() {
        let Some(s) = spice() else { return };
        let u = crate::data::Universe::load();
        let earth = u.planet_truth[2].as_ref().expect("earth track");
        for k in [5usize, 60, 130] {
            let (jd, pos, _) = earth.samples[k];
            let spk = s.helio_ecl_m(399, jd).expect("coverage");
            let err_km = (spk - pos).length() / 1e3;
            assert!(err_km < 2.0, "earth sample {k}: {err_km} km");
        }
        // Moon relative to Earth against the geocentric Horizons arc
        let moon = u.moon_truth.as_ref().expect("moon track");
        for k in [10usize, 200] {
            let (jd, rel, _) = moon.samples[k];
            let spk = s.rel_ecl_m(301, 399, jd).expect("coverage");
            let err_km = (spk - rel).length() / 1e3;
            assert!(err_km < 2.0, "moon sample {k}: {err_km} km");
        }
        // refuses outside ALL kernel coverage (jup365 reaches back to 1600,
        // so probe the 14th century)
        assert!(s.helio_ecl_m(399, J2000 - 220_000.0).is_none());
    }

    /// Satellite-kernel gate: jup365 must reproduce an independently fetched
    /// jovicentric Io arc from Horizons. Skips when the kernel isn't present.
    #[test]
    fn jup365_matches_horizons_io() {
        let Some(s) = spice() else { return };
        if !s.has(501, J2000 + 9_650.0) {
            eprintln!("jup365 not loaded — satellite gate skipped");
            return;
        }
        let root = crate::data::asset_root();
        let track = crate::data::load_horizons_pub(
            &root.join("data/spacecraft/io_gate.txt"),
            "Jupiter",
        )
        .expect("io gate arc");
        for k in [0usize, 7, 19] {
            let (jd, rel, _) = track.samples[k];
            let spk = s.rel_ecl_m(501, 599, jd).expect("io state");
            let err_km = (spk - rel).length() / 1e3;
            assert!(err_km < 10.0, "io sample {k}: {err_km} km");
        }
    }
}
