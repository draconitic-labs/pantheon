//! Datasets and provenance. Everything rendered traces back to one of:
//! cached query results in assets/data (Gaia DR3, NASA Exoplanet Archive,
//! JPL Horizons, Natural Earth) or small embedded reference tables.

use std::collections::HashMap;
use std::path::PathBuf;

use glam::DVec3;

use crate::frames::{radec_to_ecl, GalacticFrame, AU, KPC, MPC, PC};

// ------------------------------------------------------------ provenance

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prov {
    Measured,
    Modeled,
    Inferred,
    Procedural,
    Illustrative,
    Unknown,
}

impl Prov {
    pub fn label(self) -> &'static str {
        match self {
            Prov::Measured => "measured",
            Prov::Modeled => "modeled",
            Prov::Inferred => "inferred",
            Prov::Procedural => "procedural",
            Prov::Illustrative => "illustrative",
            Prov::Unknown => "unknown",
        }
    }
    /// Truth-mode ink for this status.
    pub fn rgb(self) -> [f32; 3] {
        match self {
            Prov::Measured => [0.42, 0.66, 1.0],
            Prov::Modeled => [0.89, 0.72, 0.30],
            Prov::Inferred => [0.95, 0.55, 0.25],
            Prov::Procedural => [0.52, 0.55, 0.62],
            Prov::Illustrative => [0.70, 0.52, 0.88],
            Prov::Unknown => [0.90, 0.34, 0.34],
        }
    }
}

// ------------------------------------------------------------- csv utils

fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in line.chars() {
        match c {
            '"' => q = !q,
            ',' if !q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

struct Csv {
    cols: HashMap<String, usize>,
    rows: Vec<Vec<String>>,
}

impl Csv {
    fn load(path: &std::path::Path) -> Option<Csv> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let head = lines.next()?;
        let cols = split_csv(head)
            .into_iter()
            .enumerate()
            .map(|(i, c)| (c.trim().to_lowercase(), i))
            .collect();
        let rows = lines.map(split_csv).collect();
        Some(Csv { cols, rows })
    }
    fn f(&self, row: &[String], col: &str) -> Option<f64> {
        let i = *self.cols.get(col)?;
        row.get(i)?.trim().parse().ok()
    }
    fn s<'a>(&self, row: &'a [String], col: &str) -> &'a str {
        self.cols
            .get(col)
            .and_then(|&i| row.get(i))
            .map(|s| s.trim())
            .unwrap_or("")
    }
}

pub fn asset_root() -> PathBuf {
    // an explicit location wins: for installed copies, and for testing how the
    // app behaves on a fresh clone without the large downloads
    if let Some(dir) = std::env::var_os("PANTHEON_ASSETS") {
        return PathBuf::from(dir);
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("assets"));
            cands.push(dir.join("../../assets"));
        }
    }
    cands.push(PathBuf::from("assets"));
    cands.push(PathBuf::from("pantheon/assets"));
    for c in &cands {
        if c.join("data").exists() {
            return c.clone();
        }
    }
    PathBuf::from("assets")
}

// ----------------------------------------------------------------- stars

pub struct Star {
    pub pos: DVec3,   // root frame, m, epoch J2016
    pub drift: DVec3, // m per julian year (proper motion + radial velocity)
    pub gmag: f32,
    pub absmag: f32,
    pub bp_rp: f32,
    pub dist_pc: f64,
    pub dist_err_pc: f64, // 1σ from parallax error (0 = unknown)
    pub name: Option<&'static str>,
    pub compiled: bool, // true = synthesized from the named-star table
}

impl Star {
    /// Epoch-aware position: J2016 catalog position propagated by proper
    /// motion + radial velocity (one provider for rendering,
    /// picking, navigation, and cards).
    pub fn pos_at(&self, jd_tdb: f64) -> DVec3 {
        let dyr = (jd_tdb - crate::sim::J2000) / 365.25 - 16.0;
        if dyr.abs() > 2.0 {
            self.pos + self.drift * dyr
        } else {
            self.pos
        }
    }
}

pub fn star_rgb(bp_rp: f32) -> [f32; 3] {
    let stops: [(f32, [f32; 3]); 6] = [
        (-0.4, [0.60, 0.71, 1.00]),
        (0.3, [0.79, 0.87, 1.00]),
        (0.7, [1.00, 0.98, 0.94]),
        (1.2, [1.00, 0.90, 0.76]),
        (2.0, [1.00, 0.77, 0.54]),
        (3.6, [1.00, 0.60, 0.40]),
    ];
    let x = bp_rp.clamp(-0.4, 3.6);
    for w in stops.windows(2) {
        if x <= w[1].0 {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            return [
                w[0].1[0] + (w[1].1[0] - w[0].1[0]) * t,
                w[0].1[1] + (w[1].1[1] - w[0].1[1]) * t,
                w[0].1[2] + (w[1].1[2] - w[0].1[2]) * t,
            ];
        }
    }
    stops[5].1
}

pub fn teff_rgb(teff: f64) -> [f32; 3] {
    // rough mapping stellar Teff → bp_rp-like index
    let bp = if teff > 9000.0 {
        -0.2
    } else if teff > 7000.0 {
        0.2
    } else if teff > 5900.0 {
        0.65
    } else if teff > 5200.0 {
        0.85
    } else if teff > 4400.0 {
        1.2
    } else if teff > 3700.0 {
        1.9
    } else {
        2.9
    };
    star_rgb(bp as f32)
}

/// name, ra deg, dec deg, dist pc, vmag, color index (B-V-ish → bp_rp-ish)
const NAMED_STARS: [(&str, f64, f64, f64, f32, f32); 36] = [
    ("Sirius", 101.2871, -16.7161, 2.64, -1.46, 0.0),
    ("Canopus", 95.9880, -52.6957, 95.0, -0.74, 0.19),
    ("α Centauri", 219.9021, -60.8340, 1.34, -0.27, 0.9),
    ("Arcturus", 213.9153, 19.1824, 11.3, -0.05, 1.5),
    ("Vega", 279.2347, 38.7837, 7.68, 0.03, 0.0),
    ("Capella", 79.1723, 45.9980, 13.1, 0.08, 1.0),
    ("Rigel", 78.6345, -8.2016, 260.0, 0.13, -0.03),
    ("Procyon", 114.8255, 5.2250, 3.51, 0.34, 0.53),
    ("Achernar", 24.4285, -57.2368, 43.0, 0.46, -0.16),
    ("Betelgeuse", 88.7929, 7.4071, 170.0, 0.50, 2.3),
    ("Hadar", 210.9559, -60.3730, 120.0, 0.61, -0.23),
    ("Altair", 297.6958, 8.8683, 5.13, 0.76, 0.28),
    ("Acrux", 186.6496, -63.0991, 99.0, 0.76, -0.24),
    ("Aldebaran", 68.9802, 16.5093, 20.4, 0.86, 1.9),
    ("Antares", 247.3519, -26.4320, 170.0, 0.96, 2.4),
    ("Spica", 201.2983, -11.1613, 77.0, 0.97, -0.23),
    ("Pollux", 116.3289, 28.0262, 10.4, 1.14, 1.3),
    ("Fomalhaut", 344.4127, -29.6222, 7.70, 1.16, 0.14),
    ("Deneb", 310.3580, 45.2803, 800.0, 1.25, 0.15),
    ("Mimosa", 191.9303, -59.6888, 85.0, 1.25, -0.23),
    ("Regulus", 152.0930, 11.9672, 24.3, 1.39, -0.11),
    ("Adhara", 104.6564, -28.9721, 132.0, 1.50, -0.21),
    ("Castor", 113.6494, 31.8883, 15.6, 1.62, 0.1),
    ("Polaris", 37.9546, 89.2641, 133.0, 1.98, 0.75),
    ("Proxima Centauri", 217.4289, -62.6795, 1.30, 11.13, 3.6),
    ("Barnard's Star", 269.4521, 4.6934, 1.83, 9.54, 2.8),
    ("Wolf 359", 164.120, 7.014, 2.41, 13.54, 3.9),
    ("Lalande 21185", 165.834, 35.970, 2.55, 7.52, 2.3),
    ("Tau Ceti", 26.017, -15.937, 3.65, 3.50, 0.9),
    ("ε Eridani", 53.233, -9.458, 3.22, 3.73, 1.1),
    ("61 Cygni", 316.745, 38.749, 3.50, 5.21, 1.5),
    ("ε Indi", 330.840, -56.786, 3.64, 4.69, 1.3),
    ("Ross 128", 176.935, 0.799, 3.37, 11.13, 3.4),
    ("Kapteyn's Star", 77.919, -45.018, 3.93, 8.85, 2.1),
    ("Groombridge 34", 4.595, 44.023, 3.56, 8.08, 2.9),
    ("Luyten's Star", 111.853, 5.226, 3.80, 9.87, 2.7),
];

/// All the stars, and how far out gaia_nearby.csv really reaches in parsecs. That catalog
/// is the dense sample; gaia_bright.csv adds naked-eye stars well beyond it.
fn load_stars(root: &std::path::Path) -> (Vec<Star>, f64) {
    let mut stars: Vec<Star> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut reach_pc = 0.0_f64;
    for file in ["gaia_nearby.csv", "gaia_bright.csv"] {
        let Some(csv) = Csv::load(&root.join("data").join(file)) else {
            continue;
        };
        for row in &csv.rows {
            let sid = csv.s(row, "source_id").to_string();
            if sid.is_empty() || seen.contains_key(&sid) {
                continue;
            }
            let (Some(ra), Some(dec), Some(par), Some(g)) = (
                csv.f(row, "ra"),
                csv.f(row, "dec"),
                csv.f(row, "parallax"),
                csv.f(row, "phot_g_mean_mag"),
            ) else {
                continue;
            };
            if par <= 0.05 {
                continue;
            }
            let dist_pc = 1000.0 / par;
            if file == "gaia_nearby.csv" {
                reach_pc = reach_pc.max(dist_pc);
            }
            let pos = radec_to_ecl(ra, dec, dist_pc * PC);
            let bp_rp = csv.f(row, "bp_rp").unwrap_or(0.8) as f32;
            // proper motion (mas/yr) → transverse m/yr; radial velocity km/s → m/yr
            let pm_ra = csv.f(row, "pmra").unwrap_or(0.0);
            let pm_dec = csv.f(row, "pmdec").unwrap_or(0.0);
            let rv = csv.f(row, "radial_velocity").unwrap_or(0.0);
            // Gaia pmra is μα·cosδ, so scaling unit sky-basis vectors is direct.
            let e_hat = (radec_to_ecl(ra + 0.001, dec, dist_pc * PC) - pos).normalize_or_zero();
            let n_hat = (radec_to_ecl(ra, dec + 0.001, dist_pc * PC) - pos).normalize_or_zero();
            let masyr_to_m = dist_pc * PC * (1e-3 / 3600.0f64).to_radians();
            let mut v = e_hat * (pm_ra * masyr_to_m) + n_hat * (pm_dec * masyr_to_m);
            v += pos.normalize_or_zero() * (rv * 1e3 * 3.155_76e7); // km/s → m/yr
            let absmag = (g + 5.0 - 5.0 * dist_pc.log10()) as f32;
            let dist_err_pc = csv
                .f(row, "parallax_error")
                .map(|e| (e / par).abs() * dist_pc)
                .unwrap_or(0.0);
            seen.insert(sid, stars.len());
            stars.push(Star {
                pos,
                drift: v,
                gmag: g as f32,
                absmag,
                bp_rp,
                dist_pc,
                dist_err_pc,
                name: None,
                compiled: false,
            });
        }
    }
    // attach names by angular + distance proximity; synthesize when missing
    for (name, ra, dec, dist, vmag, ci) in NAMED_STARS {
        let dir = radec_to_ecl(ra, dec, 1.0);
        let mut best: Option<(usize, f64)> = None;
        for (i, s) in stars.iter().enumerate() {
            if s.name.is_some() {
                continue;
            }
            // tightened crossmatch: ~0.11° cone, distance
            // within ~40%, magnitude within 2.5 (G vs V tolerance)
            let d = s.pos.normalize_or_zero().dot(dir);
            if d > 0.999_998 && (s.dist_pc / dist).ln().abs() < 0.35 && (s.gmag - vmag).abs() < 2.5
            {
                let score = 1.0 - d;
                if best.map(|(_, b)| score < b).unwrap_or(true) {
                    best = Some((i, score));
                }
            }
        }
        match best {
            Some((i, _)) => stars[i].name = Some(name),
            None => {
                let absmag = vmag + 5.0 - 5.0 * (dist.log10() as f32);
                stars.push(Star {
                    pos: radec_to_ecl(ra, dec, dist * PC),
                    drift: DVec3::ZERO,
                    gmag: vmag,
                    absmag,
                    bp_rp: ci,
                    dist_pc: dist,
                    dist_err_pc: 0.0,
                    name: Some(name),
                    compiled: true,
                });
            }
        }
    }
    (stars, reach_pc)
}

// ------------------------------------------------------------ exoplanets

pub struct ExoPlanet {
    pub name: String,
    pub per_d: Option<f64>,
    pub a_au: Option<f64>,
    pub e: Option<f64>,
    pub incl_deg: Option<f64>,
    pub rade: Option<f64>,
    pub masse: Option<f64>,
    pub eqt: Option<f64>,
    pub year: Option<i32>,
    pub method: String,
}

pub struct ExoSystem {
    pub host: String,
    pub pos: DVec3,
    pub dist_pc: Option<f64>,
    pub st_teff: Option<f64>,
    pub st_rad: Option<f64>,
    pub st_mass: Option<f64>,
    pub spec: String,
    pub vmag: Option<f64>,
    pub planets: Vec<ExoPlanet>,
}

impl ExoSystem {
    /// Semi-major axis with fallback via Kepler's third law (status=inferred).
    pub fn a_au_of(&self, p: &ExoPlanet) -> Option<(f64, Prov)> {
        if let Some(a) = p.a_au {
            return Some((a, Prov::Measured));
        }
        let per = p.per_d?;
        let m = self.st_mass.unwrap_or(1.0);
        let a = (m * (per / 365.25) * (per / 365.25)).powf(1.0 / 3.0);
        Some((a, Prov::Inferred))
    }
}

fn load_exo(root: &std::path::Path) -> Vec<ExoSystem> {
    let Some(csv) = Csv::load(&root.join("data").join("exoplanets.csv")) else {
        return Vec::new();
    };
    let mut sys: HashMap<String, ExoSystem> = HashMap::new();
    for row in &csv.rows {
        let host = csv.s(row, "hostname").to_string();
        if host.is_empty() {
            continue;
        }
        let (Some(ra), Some(dec)) = (csv.f(row, "ra"), csv.f(row, "dec")) else {
            continue;
        };
        let dist = csv.f(row, "sy_dist");
        let entry = sys.entry(host.clone()).or_insert_with(|| ExoSystem {
            host,
            pos: radec_to_ecl(ra, dec, dist.unwrap_or(500.0) * PC),
            dist_pc: dist,
            st_teff: csv.f(row, "st_teff"),
            st_rad: csv.f(row, "st_rad"),
            st_mass: csv.f(row, "st_mass"),
            spec: csv.s(row, "st_spectype").to_string(),
            vmag: csv.f(row, "sy_vmag"),
            planets: Vec::new(),
        });
        if entry.st_teff.is_none() {
            entry.st_teff = csv.f(row, "st_teff");
        }
        if entry.st_mass.is_none() {
            entry.st_mass = csv.f(row, "st_mass");
        }
        if entry.st_rad.is_none() {
            entry.st_rad = csv.f(row, "st_rad");
        }
        entry.planets.push(ExoPlanet {
            name: csv.s(row, "pl_name").to_string(),
            per_d: csv.f(row, "pl_orbper"),
            a_au: csv.f(row, "pl_orbsmax"),
            e: csv.f(row, "pl_orbeccen"),
            incl_deg: csv.f(row, "pl_orbincl"),
            rade: csv.f(row, "pl_rade"),
            masse: csv.f(row, "pl_bmasse"),
            eqt: csv.f(row, "pl_eqt"),
            year: csv.f(row, "disc_year").map(|y| y as i32),
            method: csv.s(row, "discoverymethod").to_string(),
        });
    }
    let mut v: Vec<ExoSystem> = sys.into_values().collect();
    // sort by one normalized semi-major-axis estimate — never
    // compare AU against days.
    for s in &mut v {
        let planets = std::mem::take(&mut s.planets);
        let mut keyed: Vec<(f64, ExoPlanet)> = planets
            .into_iter()
            .map(|p| {
                let k = s.a_au_of(&p).map(|(a, _)| a).unwrap_or(f64::MAX);
                (k, p)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        s.planets = keyed.into_iter().map(|(_, p)| p).collect();
    }
    v.sort_by(|a, b| a.host.cmp(&b.host));
    v
}

// ----------------------------------------------------- horizons tracks

pub struct Track {
    /// jd (TDB), position (m), velocity (m per day) — heliocentric ecliptic J2000
    pub samples: Vec<(f64, DVec3, DVec3)>,
}

impl Track {
    /// Cubic Hermite interpolation using Horizons velocities
    /// (linear interpolation at multi-day cadence was wrong by up to ~1e6 km).
    /// Returns None outside the covered interval — never extrapolates.
    pub fn pos_at(&self, jd: f64) -> Option<DVec3> {
        let s = &self.samples;
        if s.is_empty() || jd < s[0].0 || jd > s[s.len() - 1].0 {
            return None;
        }
        let i = s
            .partition_point(|(t, _, _)| *t <= jd)
            .min(s.len() - 1)
            .max(1);
        let (t0, p0, v0) = s[i - 1];
        let (t1, p1, v1) = s[i];
        let h = t1 - t0;
        if h <= 0.0 {
            return Some(p0);
        }
        let u = ((jd - t0) / h).clamp(0.0, 1.0);
        let (u2, u3) = (u * u, u * u * u);
        let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
        let h10 = u3 - 2.0 * u2 + u;
        let h01 = -2.0 * u3 + 3.0 * u2;
        let h11 = u3 - u2;
        Some(p0 * h00 + v0 * (h10 * h) + p1 * h01 + v1 * (h11 * h))
    }
    pub fn span(&self) -> (f64, f64) {
        if self.samples.is_empty() {
            (0.0, 0.0)
        } else {
            (self.samples[0].0, self.samples[self.samples.len() - 1].0)
        }
    }
    pub fn covers(&self, jd: f64, slack_days: f64) -> bool {
        let (a, b) = self.span();
        jd >= a - slack_days && jd <= b + slack_days
    }
}

/// Test-visible wrapper (kernel gate arcs live outside the Universe set).
pub fn load_horizons_pub(path: &std::path::Path, expect_center: &str) -> Result<Track, String> {
    load_horizons(path, expect_center)
}

/// Parse a cached Horizons vector file, validating center and reference frame
/// from the header and requiring monotonic finite samples.
fn load_horizons(path: &std::path::Path, expect_center: &str) -> Result<Track, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string();
    let text = std::fs::read_to_string(path).map_err(|_| format!("{name}: MISSING"))?;
    if !text.contains("Ecliptic of J2000.0") {
        return Err(format!(
            "{name}: INVALID — reference plane is not ecliptic J2000"
        ));
    }
    let center_ok = text
        .lines()
        .find(|l| l.contains("Center body name"))
        .map(|l| l.contains(expect_center))
        .unwrap_or(false);
    if !center_ok {
        return Err(format!("{name}: INVALID — center is not {expect_center}"));
    }
    let soe = text
        .find("$$SOE")
        .ok_or_else(|| format!("{name}: INVALID — no $$SOE"))?;
    let eoe = text
        .find("$$EOE")
        .ok_or_else(|| format!("{name}: INVALID — no $$EOE"))?;
    let mut samples: Vec<(f64, DVec3, DVec3)> = Vec::new();
    for line in text[soe + 5..eoe].lines() {
        let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if f.len() < 8 {
            continue;
        }
        let nums: Option<Vec<f64>> = [0usize, 2, 3, 4, 5, 6, 7]
            .iter()
            .map(|&k| f[k].parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect();
        let Some(n) = nums else { continue };
        if let Some(last) = samples.last() {
            if n[0] <= last.0 {
                return Err(format!("{name}: INVALID — non-monotonic time"));
            }
        }
        samples.push((
            n[0],
            DVec3::new(n[1] * AU, n[2] * AU, n[3] * AU),
            DVec3::new(n[4] * AU, n[5] * AU, n[6] * AU), // AU/day → m/day
        ));
    }
    if samples.len() < 4 {
        return Err(format!("{name}: INVALID — only {} samples", samples.len()));
    }
    Ok(Track { samples })
}

pub struct Craft {
    pub name: &'static str,
    pub color: [f32; 3],
    pub track: Track,
    pub note: &'static str,
}

const CRAFT_FILES: [(&str, &str, [f32; 3], &str); 7] = [
    (
        "voyager1",
        "Voyager 1",
        [0.55, 0.75, 1.0],
        "launched 1977; interstellar space since 2012",
    ),
    (
        "voyager2",
        "Voyager 2",
        [0.55, 0.9, 0.9],
        "launched 1977; interstellar space since 2018",
    ),
    (
        "pioneer10",
        "Pioneer 10",
        [0.9, 0.7, 0.5],
        "launched 1972; contact lost 2003",
    ),
    (
        "pioneer11",
        "Pioneer 11",
        [0.9, 0.6, 0.6],
        "launched 1973; contact lost 1995",
    ),
    (
        "newhorizons",
        "New Horizons",
        [0.7, 0.85, 1.0],
        "Pluto flyby 2015; Kuiper belt",
    ),
    (
        "parker",
        "Parker Solar Probe",
        [1.0, 0.85, 0.4],
        "corona-grazing solar orbits",
    ),
    (
        "cassini",
        "Cassini",
        [0.85, 0.8, 0.55],
        "Saturn orbiter 2004–2017 (mission ended)",
    ),
];

// ------------------------------------------------- local group & cosmos

pub struct GalaxyDef {
    pub name: &'static str,
    pub pos: DVec3,
    pub dist_kpc: f64,
    pub size_kpc: f64,
    pub spiral: bool,
}

const LOCAL_GROUP: [(&str, f64, f64, f64, f64, bool); 20] = [
    ("Large Magellanic Cloud", 80.894, -69.756, 50.0, 9.9, false),
    ("Small Magellanic Cloud", 13.187, -72.829, 62.0, 5.8, false),
    ("Sagittarius dSph", 283.831, -30.545, 26.0, 7.0, false),
    ("Draco dSph", 260.052, 57.915, 76.0, 0.7, false),
    ("Ursa Minor dSph", 227.285, 67.222, 76.0, 0.6, false),
    ("Sculptor dSph", 15.039, -33.709, 86.0, 0.8, false),
    ("Sextans dSph", 153.262, -1.615, 95.0, 0.8, false),
    ("Carina dSph", 100.403, -50.966, 105.0, 0.7, false),
    ("Fornax dSph", 39.997, -34.449, 147.0, 2.0, false),
    ("Leo II", 168.370, 22.152, 233.0, 0.7, false),
    ("Leo I", 152.117, 12.306, 254.0, 0.8, false),
    ("Phoenix dwarf", 27.776, -44.445, 415.0, 0.6, false),
    ("NGC 6822", 296.235, -14.789, 500.0, 2.4, false),
    ("IC 10", 5.072, 59.303, 660.0, 1.5, false),
    ("NGC 185", 9.742, 48.337, 617.0, 2.4, false),
    ("NGC 147", 8.301, 48.509, 676.0, 2.7, false),
    ("Andromeda (M31)", 10.685, 41.269, 785.0, 46.0, true),
    ("M32", 10.674, 40.865, 785.0, 2.5, false),
    ("Triangulum (M33)", 23.462, 30.660, 870.0, 18.0, true),
    ("WLM", 0.492, -15.461, 933.0, 3.0, false),
];

pub struct CosmicNode {
    pub name: &'static str,
    pub pos: DVec3,
    pub dist_mpc: f64,
    pub size_mpc: f64,
}

const COSMIC_NODES: [(&str, f64, f64, f64, f64); 15] = [
    ("Virgo Cluster", 187.70, 12.34, 16.5, 4.4),
    ("Fornax Cluster", 54.62, -35.45, 19.0, 2.0),
    ("Ursa Major Cluster", 176.0, 49.0, 18.0, 2.0),
    ("Antlia Cluster", 157.51, -35.32, 41.0, 2.0),
    ("Centaurus Cluster", 192.20, -41.31, 52.0, 3.0),
    ("Pegasus I Cluster", 340.5, 8.2, 53.0, 2.2),
    ("Hydra Cluster", 139.52, -12.10, 58.0, 3.0),
    ("Norma / Great Attractor", 243.9, -60.9, 68.0, 8.0),
    ("Perseus Cluster", 49.95, 41.51, 74.0, 4.5),
    ("Leo Cluster (A1367)", 176.12, 19.84, 92.0, 4.0),
    ("Coma Cluster", 194.95, 27.98, 100.0, 6.0),
    ("Ophiuchus Cluster", 258.1, -23.4, 120.0, 5.0),
    ("Hercules Cluster (A2151)", 241.15, 17.75, 155.0, 5.0),
    ("Shapley Concentration", 202.5, -31.5, 200.0, 12.0),
    ("Horologium-Reticulum", 49.5, -50.6, 210.0, 10.0),
];

// -------------------------------------------------------------- universe

pub struct Universe {
    pub gal: GalacticFrame,
    pub stars: Vec<Star>,
    /// where the dense Gaia sample really ends, measured from the loaded catalog; 0
    /// if it isn't loaded
    pub gaia_reach_pc: f64,
    pub exos: Vec<ExoSystem>,
    pub exo_by_host: HashMap<String, usize>,
    pub craft: Vec<Craft>,
    pub planet_truth: Vec<Option<Track>>,
    pub moon_truth: Option<Track>,
    pub coast: Vec<Vec<[f32; 2]>>,
    /// lon/lat of illustrative settlement lights (land-masked, deterministic)
    pub night_lights: Vec<[f32; 2]>,
    pub galaxies: Vec<GalaxyDef>,
    pub nodes: Vec<CosmicNode>,
    /// when each dataset was retrieved; datasets with no known date have no entry
    pub retrieved: std::collections::BTreeMap<&'static str, String>,
    graph: std::sync::OnceLock<crate::entity::FrameGraph>,
}

impl Universe {
    /// The frame graph — built lazily from the loaded catalogs;
    /// the single authority for object positions via `atlas::obj_pos`.
    pub fn graph(&self) -> &crate::entity::FrameGraph {
        self.graph
            .get_or_init(|| crate::entity::FrameGraph::build(self))
    }
}

pub struct DataHealth {
    pub lines: Vec<String>,
}

impl Universe {
    pub fn load() -> Self {
        Self::load_with_health().0
    }

    /// Load all datasets, reporting each one explicitly
    /// (silent partial loads must be visible).
    pub fn load_with_health() -> (Self, DataHealth) {
        let mut health: Vec<String> = Vec::new();
        let root = asset_root();
        let gal = crate::frames::galactic_frame();
        let (stars, gaia_reach_pc) = load_stars(&root);
        health.push(if stars.len() > 1000 {
            format!("stars: OK — {} sources", stars.len())
        } else {
            format!(
                "stars: MISSING/INVALID — only {} sources loaded",
                stars.len()
            )
        });
        let exos = load_exo(&root);
        health.push(if exos.len() > 500 {
            format!("exoplanets: OK — {} systems", exos.len())
        } else {
            format!("exoplanets: MISSING/INVALID — only {} systems", exos.len())
        });
        let exo_by_host = exos
            .iter()
            .enumerate()
            .map(|(i, s)| (s.host.clone(), i))
            .collect();
        let craft: Vec<Craft> = CRAFT_FILES
            .iter()
            .filter_map(|(file, name, color, note)| {
                match load_horizons(
                    &root.join("data/spacecraft").join(format!("{file}.txt")),
                    "Sun",
                ) {
                    Ok(track) => {
                        health.push(format!(
                            "craft {name}: OK — {} samples",
                            track.samples.len()
                        ));
                        Some(Craft {
                            name,
                            color: *color,
                            track,
                            note,
                        })
                    }
                    Err(e) => {
                        health.push(format!("craft {name}: {e}"));
                        None
                    }
                }
            })
            .collect();
        let planet_truth: Vec<Option<Track>> = crate::sim::PLANETS
            .iter()
            .map(|p| {
                let path = root
                    .join("data/spacecraft")
                    .join(format!("planet_{}.txt", p.name.to_lowercase()));
                match load_horizons(&path, "Sun") {
                    Ok(t) => Some(t),
                    Err(e) => {
                        health.push(format!("truth {}: {e}", p.name));
                        None
                    }
                }
            })
            .collect();
        let moon_truth = match load_horizons(&root.join("data/spacecraft/moon_geo.txt"), "Earth") {
            Ok(t) => Some(t),
            Err(e) => {
                health.push(format!("truth Moon: {e}"));
                None
            }
        };
        let coast = load_coast(&root);
        health.push(if coast.len() > 50 {
            format!("coastlines: OK — {} lines", coast.len())
        } else {
            format!("coastlines: MISSING/INVALID — {} lines", coast.len())
        });
        let land = load_land_rings(&root);
        let night_lights = generate_night_lights(&land);
        health.push(if night_lights.len() > 500 {
            format!(
                "land/night lights: OK — {} rings, {} lights",
                land.len(),
                night_lights.len()
            )
        } else {
            format!(
                "land/night lights: MISSING/INVALID — {} rings, {} lights (optional)",
                land.len(),
                night_lights.len()
            )
        });
        let galaxies = LOCAL_GROUP
            .iter()
            .map(|(name, ra, dec, dkpc, size, spiral)| GalaxyDef {
                name,
                pos: radec_to_ecl(*ra, *dec, dkpc * KPC),
                dist_kpc: *dkpc,
                size_kpc: *size,
                spiral: *spiral,
            })
            .collect();
        let nodes = COSMIC_NODES
            .iter()
            .map(|(name, ra, dec, dmpc, size)| CosmicNode {
                name,
                pos: radec_to_ecl(*ra, *dec, dmpc * MPC),
                dist_mpc: *dmpc,
                size_mpc: *size,
            })
            .collect();
        let retrieved = load_retrieved(&root);
        (
            Self {
                gal,
                stars,
                gaia_reach_pc,
                exos,
                exo_by_host,
                craft,
                planet_truth,
                moon_truth,
                coast,
                night_lights,
                galaxies,
                nodes,
                retrieved,
                graph: std::sync::OnceLock::new(),
            },
            DataHealth { lines: health },
        )
    }

    /// Find a system by host name, falling back to planet-name prefix
    /// (e.g. "Kepler-90" resolves to host "KOI-351").
    pub fn find_system(&self, name: &str) -> Option<usize> {
        if let Some(&i) = self.exo_by_host.get(name) {
            return Some(i);
        }
        // case-insensitive host, then planet-name prefix
        let lower = name.to_lowercase();
        if let Some(i) = self
            .exos
            .iter()
            .position(|s| s.host.to_lowercase() == lower)
        {
            return Some(i);
        }
        let prefix = format!("{lower} ");
        self.exos.iter().position(|s| {
            s.planets
                .iter()
                .any(|p| p.name.to_lowercase().starts_with(&prefix))
        })
    }

    /// Model-vs-Horizons residual for a planet at jd, if truth data covers it.
    /// Compares like centers: Earth model = derived Earth center vs Horizons 399.
    pub fn planet_residual_km(&self, pi: usize, jd: f64) -> Option<f64> {
        let t = self.planet_truth.get(pi)?.as_ref()?;
        let truth = t.pos_at(jd)?;
        Some((crate::sim::body_pos(pi, jd) - truth).length() / 1e3)
    }
}

/// The dataset a file under assets/data belongs to, as evidence records name it.
fn dataset_of(file: &str) -> Option<&'static str> {
    Some(match file {
        "gaia_nearby.csv" | "gaia_bright.csv" => "gaia",
        "exoplanets.csv" => "exoplanets",
        "ne_110m_coastline.geojson" | "ne_110m_land.geojson" => "natural-earth",
        "textures/eso_milkyway.jpg" => "eso-panorama",
        _ if file.starts_with("textures/") => "maps",
        _ if file.starts_with("spacecraft/") => "horizons",
        _ if file.starts_with("spice/") => "kernels",
        _ => return None,
    })
}

fn date_map(path: &std::path::Path, key: Option<&str>) -> HashMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let map = key.map_or(&value, |k| &value[k]);
    map.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(file, date)| Some((file.clone(), date.as_str()?.to_string())))
        .collect()
}

/// When each dataset was retrieved. provenance.json dates the catalogs that ship in the
/// repository; fetched.json, written by tools/fetch_data.py, dates what was downloaded
/// on this computer, file by file. A dataset fetched across several days shows the range.
fn load_retrieved(root: &std::path::Path) -> std::collections::BTreeMap<&'static str, String> {
    let mut dates = date_map(&root.join("data/provenance.json"), Some("retrieved"));
    dates.extend(date_map(&root.join("data/fetched.json"), None));
    retrieved_by_dataset(&dates)
}

fn retrieved_by_dataset(
    dates: &HashMap<String, String>,
) -> std::collections::BTreeMap<&'static str, String> {
    let mut range: std::collections::BTreeMap<&'static str, (&str, &str)> = Default::default();
    for (file, date) in dates {
        let Some(dataset) = dataset_of(file) else {
            continue;
        };
        let r = range.entry(dataset).or_insert((date, date));
        r.0 = r.0.min(date.as_str());
        r.1 = r.1.max(date.as_str());
    }
    range
        .into_iter()
        .map(|(dataset, (first, last))| {
            let shown = if first == last {
                first.to_string()
            } else {
                format!("{first} to {last}")
            };
            (dataset, shown)
        })
        .collect()
}

/// Land polygon rings (outer + holes together; even-odd containment).
fn load_land_rings(root: &std::path::Path) -> Vec<Vec<[f64; 2]>> {
    let Ok(text) = std::fs::read_to_string(root.join("data/ne_110m_land.geojson")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return Vec::new();
    };
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut push_ring = |coords: &serde_json::Value| {
        if let Some(arr) = coords.as_array() {
            let ring: Vec<[f64; 2]> = arr
                .iter()
                .filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?]))
                .collect();
            if ring.len() > 3 {
                rings.push(ring);
            }
        }
    };
    let Some(features) = v["features"].as_array() else {
        return rings;
    };
    for f in features {
        let geom = &f["geometry"];
        match geom["type"].as_str() {
            Some("Polygon") => {
                if let Some(rs) = geom["coordinates"].as_array() {
                    for r in rs {
                        push_ring(r);
                    }
                }
            }
            Some("MultiPolygon") => {
                if let Some(polys) = geom["coordinates"].as_array() {
                    for poly in polys {
                        if let Some(rs) = poly.as_array() {
                            for r in rs {
                                push_ring(r);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    rings
}

fn point_in_rings(rings: &[Vec<[f64; 2]>], lon: f64, lat: f64) -> bool {
    // even-odd ray cast across every ring (holes flip containment back off)
    let mut inside = false;
    for ring in rings {
        let n = ring.len();
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = (ring[i][0], ring[i][1]);
            let (xj, yj) = (ring[j][0], ring[j][1]);
            if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

/// Deterministic land-masked light points for Earth's night side. The land
/// mask is measured (Natural Earth); the placement is a human-geography
/// prior (mid-latitude weighting), status = illustrative.
fn generate_night_lights(rings: &[Vec<[f64; 2]>]) -> Vec<[f32; 2]> {
    if rings.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<[f32; 2]> = Vec::new();
    let mut rng: u64 = 0x51_9E37_79B9_7F4A;
    let mut next = move || -> f64 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut tries = 0;
    while out.len() < 1_400 && tries < 80_000 {
        tries += 1;
        let lon = next() * 360.0 - 180.0;
        let lat = (2.0 * next() - 1.0).asin().to_degrees(); // area-uniform on the sphere
        let alat = lat.abs();
        // settlement prior: dense 10–55°, sparse subpolar, nearly none polar
        let keep = if alat > 66.0 {
            0.015
        } else if alat > 55.0 {
            0.25
        } else if alat < 5.0 {
            0.55
        } else {
            1.0
        };
        if next() > keep {
            continue;
        }
        if point_in_rings(rings, lon, lat) {
            out.push([lon as f32, lat as f32]);
        }
    }
    out
}

fn load_coast(root: &std::path::Path) -> Vec<Vec<[f32; 2]>> {
    let Ok(text) = std::fs::read_to_string(root.join("data/ne_110m_coastline.geojson")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Some(features) = v["features"].as_array() else {
        return out;
    };
    for f in features {
        let geom = &f["geometry"];
        let push_line = |out: &mut Vec<Vec<[f32; 2]>>, coords: &serde_json::Value| {
            if let Some(arr) = coords.as_array() {
                let line: Vec<[f32; 2]> = arr
                    .iter()
                    .filter_map(|p| {
                        let x = p.get(0)?.as_f64()?;
                        let y = p.get(1)?.as_f64()?;
                        Some([x as f32, y as f32])
                    })
                    .collect();
                if line.len() > 1 {
                    out.push(line);
                }
            }
        };
        match geom["type"].as_str() {
            Some("LineString") => push_line(&mut out, &geom["coordinates"]),
            Some("MultiLineString") => {
                if let Some(lines) = geom["coordinates"].as_array() {
                    for l in lines {
                        push_line(&mut out, l);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dates(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(f, d)| (f.to_string(), d.to_string()))
            .collect()
    }

    #[test]
    fn the_gaia_reach_comes_from_the_nearby_catalog_alone() {
        let root = std::env::temp_dir().join(format!("pantheon-data-test-{}", std::process::id()));
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let header = "source_id,ra,dec,parallax,phot_g_mean_mag";
        let (near, bright) = (data.join("gaia_nearby.csv"), data.join("gaia_bright.csv"));
        // parallaxes of 100 and 40 mas: 10 and 25 pc
        std::fs::write(&near, format!("{header}\n1,10,10,100,8\n2,20,20,40,9\n")).unwrap();
        // 1 mas is 1000 pc, far past the dense sample
        std::fs::write(&bright, format!("{header}\n3,30,30,1,5\n")).unwrap();
        let (stars, reach) = load_stars(&root);
        std::fs::remove_file(&near).unwrap();
        std::fs::remove_file(&bright).unwrap();
        std::fs::remove_dir(&data).unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert!((reach - 25.0).abs() < 1e-9, "{reach}");
        assert!(stars.iter().any(|s| s.dist_pc > 900.0));
    }

    #[test]
    fn the_shipped_catalog_reaches_nowhere_near_83_pc() {
        let u = Universe::load();
        assert!(
            u.gaia_reach_pc > 20.0 && u.gaia_reach_pc < 60.0,
            "{}",
            u.gaia_reach_pc
        );
    }

    #[test]
    fn each_dataset_keeps_its_own_retrieval_date() {
        let got = retrieved_by_dataset(&dates(&[
            ("gaia_nearby.csv", "2026-07-11"),
            ("gaia_bright.csv", "2026-07-11"),
            ("textures/8k_moon.jpg", "2026-11-04"),
            ("textures/eso_milkyway.jpg", "2026-08-01"),
            ("not_a_dataset.bin", "2020-01-01"),
        ]));
        assert_eq!(got["gaia"], "2026-07-11");
        assert_eq!(got["maps"], "2026-11-04");
        assert_eq!(got["eso-panorama"], "2026-08-01");
        assert!(
            !got.contains_key("horizons"),
            "nothing dates the tracks here"
        );
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn a_dataset_fetched_across_days_shows_the_range() {
        let got = retrieved_by_dataset(&dates(&[
            ("spacecraft/voyager1.txt", "2026-07-11"),
            ("spacecraft/pioneer10.txt", "2026-11-04"),
            ("spacecraft/parker.txt", "2026-08-01"),
        ]));
        assert_eq!(got["horizons"], "2026-07-11 to 2026-11-04");
    }

    #[test]
    fn every_evidence_record_names_a_dataset_that_exists() {
        let mapped = [
            "gaia_nearby.csv",
            "exoplanets.csv",
            "ne_110m_land.geojson",
            "textures/8k_moon.jpg",
            "textures/eso_milkyway.jpg",
            "spacecraft/voyager1.txt",
            "spice/de440s.bsp",
        ]
        .map(|f| dataset_of(f).unwrap());
        for dataset in [
            "gaia",
            "exoplanets",
            "natural-earth",
            "maps",
            "eso-panorama",
            "horizons",
        ] {
            assert!(mapped.contains(&dataset), "{dataset}");
        }
    }
}
