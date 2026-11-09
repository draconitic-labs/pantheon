//! A dependency-free reader for NAIF SPICE kernels.
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
//!
//! A kernel on disk can be truncated or damaged. Loading checks every offset against
//! the file's real length, so a bad file is reported in [`problems`] and skipped, and
//! the app carries on with what remains.

use std::path::Path;

use glam::DVec3;

pub const J2000_JD: f64 = 2_451_545.0;

// ------------------------------------------------------------------ mmap

/// Read-only memory mapping (Windows kernel32 FFI, no dependencies): the
/// kernel set is ~2 GB on disk — pages fault in on demand instead of being
/// read into RAM at startup.
#[cfg(windows)]
mod mapped {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileMappingW(
            file: isize,
            attrs: *const core::ffi::c_void,
            protect: u32,
            size_hi: u32,
            size_lo: u32,
            name: *const u16,
        ) -> isize;
        fn MapViewOfFile(
            map: isize,
            access: u32,
            off_hi: u32,
            off_lo: u32,
            size: usize,
        ) -> *const u8;
        fn UnmapViewOfFile(ptr: *const u8) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    const PAGE_READONLY: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x0004;

    pub struct Mmap {
        ptr: *const u8,
        len: usize,
        map: isize,
    }
    // read-only view over an immutable file
    unsafe impl Send for Mmap {}
    unsafe impl Sync for Mmap {}

    impl Mmap {
        pub fn open(path: &std::path::Path) -> std::io::Result<Mmap> {
            let f = std::fs::File::open(path)?;
            let len = f.metadata()?.len() as usize;
            if len == 0 {
                return Err(std::io::Error::other("empty file"));
            }
            unsafe {
                let map = CreateFileMappingW(
                    f.as_raw_handle() as isize,
                    std::ptr::null(),
                    PAGE_READONLY,
                    0,
                    0,
                    std::ptr::null(),
                );
                if map == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let ptr = MapViewOfFile(map, FILE_MAP_READ, 0, 0, 0);
                if ptr.is_null() {
                    CloseHandle(map);
                    return Err(std::io::Error::last_os_error());
                }
                Ok(Mmap { ptr, len, map })
            }
            // the File handle may close: the mapping keeps its own reference
        }
        pub fn as_slice(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    impl Drop for Mmap {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.ptr);
                CloseHandle(self.map);
            }
        }
    }
}

/// Kernel bytes: memory-mapped where the platform allows, heap-read otherwise.
enum Buffer {
    Heap(Vec<u8>),
    #[cfg(windows)]
    Map(mapped::Mmap),
    #[cfg(unix)]
    Unix(memmap2::Mmap),
}

impl Buffer {
    fn open(path: &Path) -> Result<Buffer, String> {
        #[cfg(windows)]
        {
            if let Ok(m) = mapped::Mmap::open(path) {
                return Ok(Buffer::Map(m));
            }
        }
        #[cfg(unix)]
        {
            if let Ok(file) = std::fs::File::open(path) {
                // SAFETY: the kernels are read-only data that nothing rewrites while the app runs.
                if let Ok(map) = unsafe { memmap2::Mmap::map(&file) } {
                    return Ok(Buffer::Unix(map));
                }
            }
        }
        std::fs::read(path)
            .map(Buffer::Heap)
            .map_err(|e| format!("read {}: {e}", path.display()))
    }
    fn as_slice(&self) -> &[u8] {
        match self {
            Buffer::Heap(v) => v,
            #[cfg(windows)]
            Buffer::Map(m) => m.as_slice(),
            #[cfg(unix)]
            Buffer::Unix(m) => m,
        }
    }
}

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
    buf: Buffer,
    segments: Vec<Segment>,
}

fn f64_at(buf: &[u8], word: usize) -> f64 {
    let off = (word - 1) * 8;
    f64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

fn i32_at_byte(buf: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

/// A DAF stores some counts and addresses as doubles. `None` unless it's a sane one.
fn whole_number(x: f64) -> Option<usize> {
    (x.is_finite() && x >= 0.0 && x.fract() == 0.0 && x < 1e15).then_some(x as usize)
}

/// Read the segment table, checking every address against the file's real length. A
/// segment that passes can be evaluated at any time without reading out of bounds.
fn parse_segments(buf: &[u8]) -> Result<Vec<Segment>, String> {
    if buf.len() < 1024 {
        return Err(format!("the file ends early, at {} bytes", buf.len()));
    }
    if &buf[0..8] != b"DAF/SPK " {
        return Err("not a DAF/SPK file".into());
    }
    let nd = i32_at_byte(buf, 8);
    let ni = i32_at_byte(buf, 12);
    if nd != 2 || ni != 6 {
        return Err(format!("unexpected DAF ND/NI = {nd}/{ni}"));
    }
    if &buf[88..96] != b"LTL-IEEE" {
        return Err("only little-endian (LTL-IEEE) kernels supported".into());
    }
    let ends_early = |need: usize| {
        format!(
            "the file ends early: it has {} bytes and needs at least {need}",
            buf.len()
        )
    };
    let ss = (nd + (ni + 1) / 2) as usize; // doubles per summary = 5
    let max_summaries = (1024 - 24) / (ss * 8);
    let words = buf.len() / 8;
    let mut segments = Vec::new();
    let mut rec = usize::try_from(i32_at_byte(buf, 76))
        .map_err(|_| "the first summary record's address is invalid".to_string())?;
    if rec == 0 {
        return Err("the first summary record's address is invalid".into());
    }
    // a damaged chain of records could loop forever
    let mut budget = buf.len() / 1024;
    while rec != 0 {
        budget = budget
            .checked_sub(1)
            .ok_or("the chain of summary records doesn't end")?;
        let base = (rec - 1)
            .checked_mul(1024)
            .ok_or("a summary record's address is invalid")?;
        let end = base
            .checked_add(1024)
            .ok_or("a summary record's address is invalid")?;
        if end > buf.len() {
            return Err(ends_early(end));
        }
        let double = |off: usize| f64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
        let next = whole_number(double(base)).ok_or("a summary record's link is invalid")?;
        let nsum = whole_number(double(base + 16))
            .filter(|n| *n <= max_summaries)
            .ok_or("a summary record's count is invalid")?;
        for k in 0..nsum {
            let s = base + 24 + k * ss * 8;
            let dtype = i32_at_byte(buf, s + 28);
            if dtype != 2 && dtype != 3 {
                continue; // planetary DE kernels are Type 2/3
            }
            let t0 = double(s);
            let t1 = double(s + 8);
            let target = i32_at_byte(buf, s + 16);
            let center = i32_at_byte(buf, s + 20);
            let invalid = || format!("segment for body {target} is invalid");
            let begin_word = usize::try_from(i32_at_byte(buf, s + 32)).map_err(|_| invalid())?;
            let end_word = usize::try_from(i32_at_byte(buf, s + 36)).map_err(|_| invalid())?;
            // the data and its four-word trailer: INIT, INTLEN, RSIZE, N
            if begin_word < 1 || end_word < begin_word + 3 {
                return Err(invalid());
            }
            if end_word > words {
                return Err(ends_early(end_word * 8));
            }
            let init = f64_at(buf, end_word - 3);
            let intlen = f64_at(buf, end_word - 2);
            let rsize = whole_number(f64_at(buf, end_word - 1)).ok_or_else(invalid)?;
            let n = whole_number(f64_at(buf, end_word)).ok_or_else(invalid)?;
            // each record: mid, radius, then at least one coefficient per component
            let sets = if dtype == 2 { 3 } else { 6 };
            let records = n.checked_mul(rsize).ok_or_else(invalid)?;
            let fits = begin_word
                .checked_add(records)
                .is_some_and(|after| after + 3 <= end_word);
            if n == 0
                || rsize < 2 + sets
                || !fits
                || !init.is_finite()
                || !intlen.is_finite()
                || intlen <= 0.0
            {
                return Err(invalid());
            }
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
    Ok(segments)
}

impl SpkFile {
    pub fn load(path: &Path) -> Result<SpkFile, String> {
        let buffer = Buffer::open(path)?;
        let segments = parse_segments(buffer.as_slice())?;
        Ok(SpkFile {
            buf: buffer,
            segments,
        })
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
        let buf = self.buf.as_slice();
        let idx = (((et - seg.init) / seg.intlen) as usize).min(seg.n - 1);
        let rec = seg.begin_word + idx * seg.rsize;
        let mid = f64_at(buf, rec);
        let radius = f64_at(buf, rec + 1);
        let sets = if seg.dtype == 2 { 3 } else { 6 };
        let ncoef = (seg.rsize - 2) / sets;
        let tau = ((et - mid) / radius).clamp(-1.0, 1.0);
        let mut out = [0.0f64; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let base = rec + 2 + c * ncoef;
            // Chebyshev recurrence
            let (mut t0, mut t1) = (1.0f64, tau);
            let mut acc = f64_at(buf, base);
            if ncoef > 1 {
                acc += f64_at(buf, base + 1) * t1;
            }
            for k in 2..ncoef {
                let t2 = 2.0 * tau * t1 - t0;
                acc += f64_at(buf, base + k) * t2;
                t0 = t1;
                t1 = t2;
            }
            *o = acc;
        }
        DVec3::new(out[0], out[1], out[2])
    }

    fn find(&self, target: i32, et: f64) -> Option<&Segment> {
        self.segments
            .iter()
            .find(|s| s.target == target && et >= s.t0 && et <= s.t1)
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
        let tokens = text
            .split(&[',', '(', ')', '\n', '\r', ' ', '\t'][..])
            .filter(|t| !t.is_empty());
        let mut pending: Option<f64> = None;
        for tok in tokens {
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

/// What to do about a bad kernel. tools/fetch_data.py can't tell every kind of damage from a
/// good file of the right size, but it always fetches a file that isn't there.
const REPAIR: &str = "Delete it, run tools/fetch_data.py to get a new copy, then restart.";

/// What was found in the kernel folder: the usable set, if any, and what was wrong.
pub struct KernelScan {
    pub set: Option<SpiceSet>,
    /// one plain sentence per kernel that exists but can't be used
    pub problems: Vec<String>,
}

impl SpiceSet {
    /// Load every kernel in `dir`. A bad file is reported and skipped; only a usable
    /// planetary kernel makes a set, because everything else chains through it.
    pub fn scan(dir: &Path) -> KernelScan {
        let mut spks: Vec<(String, SpkFile)> = Vec::new();
        let mut problems = Vec::new();
        let mut planetary: Option<(f64, f64)> = None;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
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
                    Err(e) => {
                        let falls_back = if name.starts_with("de440") {
                            "Every position falls back to orbital elements"
                        } else {
                            "The moons it carries fall back to mean elements"
                        };
                        problems.push(format!(
                            "Kernel {name} can't be used: {e}. {falls_back}. {REPAIR}"
                        ));
                    }
                }
            }
        }
        problems.sort();
        let tls = dir.join("naif0012.tls");
        let lsk = match Lsk::load(&tls) {
            Ok(lsk) => Some(lsk),
            Err(e) => {
                if tls.exists() {
                    problems.push(format!(
                        "Leap-second kernel naif0012.tls can't be used: {e}. UTC times before \
                         2017 can be up to half a minute off. {REPAIR}"
                    ));
                }
                None
            }
        };
        let Some((et0, et1)) = planetary else {
            return KernelScan {
                set: None,
                problems,
            };
        };
        // planetary kernel first so common lookups resolve in one probe
        spks.sort_by_key(|(n, _)| !n.starts_with("de440") as u8);
        KernelScan {
            set: Some(SpiceSet {
                spks,
                lsk,
                jd0: J2000_JD + et0 / 86_400.0,
                jd1: J2000_JD + et1 / 86_400.0,
            }),
            problems,
        }
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

static SPICE: std::sync::OnceLock<KernelScan> = std::sync::OnceLock::new();

fn loaded() -> &'static KernelScan {
    SPICE.get_or_init(|| {
        let scan = SpiceSet::scan(&crate::data::asset_path("data/spice"));
        for p in &scan.problems {
            log::warn!("spice: {p}");
        }
        match &scan.set {
            Some(s) => log::info!(
                "spice: loaded {} · JD {:.1}–{:.1}",
                s.kernel_summary(),
                s.jd0,
                s.jd1
            ),
            None => log::warn!("spice: unavailable — analytic elements only"),
        }
        scan
    })
}

/// The globally loaded kernel set (None when kernels are absent — every
/// consumer must have an analytic fallback).
pub fn spice() -> Option<&'static SpiceSet> {
    loaded().set.as_ref()
}

/// Kernels that exist but couldn't be used, for the app to say so.
pub fn problems() -> &'static [String] {
    &loaded().problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::J2000;

    /// The kernels are an optional download, and a fresh clone or CI doesn't have them.
    fn kernels_or_skip() -> Option<&'static SpiceSet> {
        let s = spice();
        if s.is_none() {
            crate::test_skipped("no kernels here");
        }
        s
    }

    #[test]
    fn lsk_parses_known_epochs() {
        let Some(s) = kernels_or_skip() else { return };
        let Some(lsk) = &s.lsk else {
            panic!("lsk should load")
        };
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
        let Some(s) = kernels_or_skip() else { return };
        let u = crate::data::Universe::load();
        let (Some(earth), Some(moon)) = (&u.planet_truth[2], &u.moon_truth) else {
            crate::test_skipped("kernels are here but the Horizons tracks aren't");
            return;
        };
        for k in [5usize, 60, 130] {
            let (jd, pos, _) = earth.samples[k];
            let spk = s.helio_ecl_m(399, jd).expect("coverage");
            let err_km = (spk - pos).length() / 1e3;
            assert!(err_km < 2.0, "earth sample {k}: {err_km} km");
        }
        // Moon relative to Earth against the geocentric Horizons arc
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

    /// Satellite-kernel gates: each loaded kernel must reproduce an
    /// independently fetched planetocentric Horizons arc. Skips per-kernel
    /// when absent.
    #[test]
    fn satellite_kernels_match_horizons() {
        let Some(s) = kernels_or_skip() else { return };
        let tracks = crate::data::asset_path("data/spacecraft");
        for (moon_id, planet_id, file, center) in [
            (501, 599, "io_gate.txt", "Jupiter"),
            (606, 699, "titan_gate.txt", "Saturn"),
        ] {
            if !s.has(moon_id, J2000 + 9_650.0) {
                crate::test_skipped(&format!("kernel for {moon_id} not loaded"));
                continue;
            }
            // these arcs were fetched by hand to check the reader, and aren't in
            // tools/fetch_data.py, so a normal fetch doesn't have them
            let path = tracks.join(file);
            if !path.is_file() {
                crate::test_skipped(&format!(
                    "{file} isn't on this computer, so no gate for {moon_id}"
                ));
                continue;
            }
            let track = crate::data::load_horizons_pub(&path, center)
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            let last = track.samples.len() - 1;
            for k in [0usize, last / 2, last] {
                let (jd, rel, _) = track.samples[k];
                let spk = s.rel_ecl_m(moon_id, planet_id, jd).expect("state");
                let err_km = (spk - rel).length() / 1e3;
                assert!(err_km < 10.0, "{file} sample {k}: {err_km} km");
            }
        }
    }

    /// A small but real Type 2 kernel: one body, a straight-line position in two
    /// records, built by hand so these tests need no downloaded data.
    fn tiny_kernel() -> Vec<u8> {
        let mut buf = vec![0u8; 1024];
        buf[0..8].copy_from_slice(b"DAF/SPK ");
        buf[8..12].copy_from_slice(&2i32.to_le_bytes());
        buf[12..16].copy_from_slice(&6i32.to_le_bytes());
        buf[76..80].copy_from_slice(&2i32.to_le_bytes()); // first summary record
        buf[88..96].copy_from_slice(b"LTL-IEEE");
        // record 2 holds the summaries, record 3 the names, record 4 on the data
        let mut summary = vec![0u8; 1024];
        summary[16..24].copy_from_slice(&1f64.to_le_bytes()); // one summary
        summary[24..32].copy_from_slice(&(-100f64).to_le_bytes()); // coverage
        summary[32..40].copy_from_slice(&100f64.to_le_bytes());
        summary[40..44].copy_from_slice(&399i32.to_le_bytes()); // target
        summary[44..48].copy_from_slice(&0i32.to_le_bytes()); // center
        summary[48..52].copy_from_slice(&1i32.to_le_bytes()); // frame
        summary[52..56].copy_from_slice(&2i32.to_le_bytes()); // type
        let begin = 2 * 128 + 128 + 1; // first word after three records
        let rsize = 8usize; // mid, radius, two coefficients for each of x y z
        let n = 2usize;
        let end = begin + n * rsize + 4 - 1;
        summary[56..60].copy_from_slice(&(begin as i32).to_le_bytes());
        summary[60..64].copy_from_slice(&(end as i32).to_le_bytes());
        buf.extend_from_slice(&summary);
        buf.extend_from_slice(&[0u8; 1024]);
        let mut words: Vec<f64> = Vec::new();
        for mid in [-50.0, 50.0] {
            // x = 1000 + et, y = 2000, z = 3000 across this record
            words.extend([mid, 50.0, 1000.0 + mid, 50.0, 2000.0, 0.0, 3000.0, 0.0]);
        }
        words.extend([-100.0, 100.0, rsize as f64, n as f64]);
        for w in words {
            buf.extend_from_slice(&w.to_le_bytes());
        }
        buf
    }

    #[test]
    fn a_whole_kernel_evaluates() {
        let segments = parse_segments(&tiny_kernel()).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].target, 399);
    }

    #[test]
    fn a_kernel_cut_anywhere_is_an_error_not_a_panic() {
        let whole = tiny_kernel();
        for len in 0..whole.len() {
            assert!(
                parse_segments(&whole[..len]).is_err(),
                "cut to {len} bytes of {} still parsed",
                whole.len()
            );
        }
    }

    #[test]
    fn a_kernel_with_damaged_words_is_an_error_or_safe_to_evaluate() {
        // overwrite every word in turn with values that used to index out of bounds
        let whole = tiny_kernel();
        for bad in [f64::NAN, f64::INFINITY, -1.0, 1e300, 0.0, 1.5] {
            for word in 0..whole.len() / 8 {
                let mut buf = whole.clone();
                buf[word * 8..word * 8 + 8].copy_from_slice(&bad.to_le_bytes());
                let Ok(segments) = parse_segments(&buf) else {
                    continue;
                };
                let spk = SpkFile {
                    buf: Buffer::Heap(buf),
                    segments,
                };
                for seg in &spk.segments {
                    for et in [-1e300, -100.0, 0.0, 100.0, 1e300, f64::NAN] {
                        spk.eval_segment(seg, et);
                    }
                }
            }
        }
    }

    #[test]
    fn a_truncated_planetary_kernel_is_reported_and_the_set_is_empty() {
        let dir = std::env::temp_dir().join(format!("pantheon-spice-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("de440s.bsp");
        std::fs::write(&file, &tiny_kernel()[..2500]).unwrap();
        let scan = SpiceSet::scan(&dir);
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert!(scan.set.is_none());
        assert_eq!(scan.problems.len(), 1);
        assert!(
            scan.problems[0].contains("de440s.bsp"),
            "{:?}",
            scan.problems
        );
        assert!(
            scan.problems[0].contains("ends early"),
            "{:?}",
            scan.problems
        );
    }

    #[test]
    fn a_damaged_leap_second_kernel_is_reported() {
        let dir = std::env::temp_dir().join(format!("pantheon-lsk-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("naif0012.tls");
        std::fs::write(&file, "not a leap-second table").unwrap();
        let scan = SpiceSet::scan(&dir);
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(scan.problems.len(), 1, "{:?}", scan.problems);
        assert!(
            scan.problems[0].contains("naif0012.tls"),
            "{:?}",
            scan.problems
        );
        assert!(
            scan.problems[0].contains("Delete it"),
            "{:?}",
            scan.problems
        );
    }

    #[test]
    fn a_folder_with_no_kernels_is_not_a_problem() {
        let scan = SpiceSet::scan(Path::new("/nonexistent/pantheon/kernels"));
        assert!(scan.set.is_none());
        assert!(scan.problems.is_empty());
    }
}
