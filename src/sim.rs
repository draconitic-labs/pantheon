//! Time, ephemerides, and local simulation islands.
//!
//! Global positions come from authoritative sources: JPL approximate planetary
//! elements (Standish, valid 1800–2050; status=modeled) evaluated analytically,
//! and cached Horizons vectors (status=measured) for spacecraft and residuals.
//! Focused systems may awaken a local N-body island (leapfrog integrator).

use glam::{DMat3, DVec3};

use crate::frames::{AU, DAY};

pub const J2000: f64 = 2_451_545.0;
pub const GM_SUN: f64 = 1.327_124_400_18e20; // m^3/s^2

// ---------------------------------------------------------------- time

/// TT−UTC = 32.184 s + 37 leap seconds (constant since 2017-01-01; no leap
/// second is scheduled through the validity of this build). TDB−TT < 2 ms is
/// neglected. The clock is now explicitly TDB internally;
/// display converts to UTC at the boundary.
pub const TT_MINUS_UTC_S: f64 = 69.184;
pub const DELTA_T_DAYS: f64 = TT_MINUS_UTC_S / 86_400.0;

/// TT−UTC seconds: leap-second kernel when loaded (historically correct
/// back to 1972), else the post-2017 constant.
fn tt_minus_utc_s(jd_utc: f64) -> f64 {
    crate::spice::spice()
        .and_then(|s| s.lsk.as_ref())
        .map(|l| l.tt_minus_utc(jd_utc))
        .unwrap_or(TT_MINUS_UTC_S)
}

pub fn tdb_from_utc_jd(jd_utc: f64) -> f64 {
    jd_utc + tt_minus_utc_s(jd_utc) / 86_400.0
}
pub fn utc_from_tdb_jd(jd_tdb: f64) -> f64 {
    // ΔAT evaluated at TDB ≈ UTC — the boundary error is ≪ 1 s
    jd_tdb - tt_minus_utc_s(jd_tdb) / 86_400.0
}

/// Simulation clock. `jd` is a Julian date in TDB (ephemeris time).
#[derive(Clone)]
pub struct Clock {
    pub jd: f64,
    pub rate: f64, // simulated seconds per real second
    pub playing: bool,
}

impl Clock {
    pub fn now() -> Self {
        Self {
            jd: jd_now(),
            rate: 86_400.0,
            playing: false,
        }
    }
    pub fn tick(&mut self, real_dt: f64) {
        if self.playing {
            self.jd += real_dt * self.rate / DAY;
        }
    }
}

/// Current Julian date in TDB.
pub fn jd_now() -> f64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    tdb_from_utc_jd(2_440_587.5 + secs / 86_400.0)
}

/// Gregorian calendar → JD (Fliegel–Van Flandern), for noon of that date.
pub fn cal_to_jd(y: i32, m: i32, d: f64) -> f64 {
    let (yy, mm) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let a = yy / 100;
    let b = 2 - a + a / 4;
    (365.25 * (yy as f64 + 4716.0)).floor() + (30.6001 * (mm as f64 + 1.0)).floor() + d + b as f64
        - 1524.5
}

/// JD → (year, month, day, hour, minute).
pub fn jd_to_cal(jd: f64) -> (i32, u32, u32, u32, u32) {
    let z = (jd + 0.5).floor();
    let f = jd + 0.5 - z;
    let a = if z < 2_299_161.0 {
        z
    } else {
        let alpha = ((z - 1_867_216.25) / 36_524.25).floor();
        z + 1.0 + alpha - (alpha / 4.0).floor()
    };
    let b = a + 1524.0;
    let c = ((b - 122.1) / 365.25).floor();
    let d = (365.25 * c).floor();
    let e = ((b - d) / 30.6001).floor();
    let day = b - d - (30.6001 * e).floor() + f;
    let month = if e < 14.0 { e - 1.0 } else { e - 13.0 } as u32;
    let year = if month > 2 { c - 4716.0 } else { c - 4715.0 } as i32;
    let dd = day.floor();
    let hh = (day - dd) * 24.0;
    let h = hh.floor();
    let mi = ((hh - h) * 60.0).floor();
    (year, month, dd as u32, h as u32, mi as u32)
}

/// Format a TDB Julian date as a UTC calendar string.
pub fn fmt_jd(jd_tdb: f64) -> String {
    let (y, m, d, h, mi) = jd_to_cal(utc_from_tdb_jd(jd_tdb));
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mname = MON[((m as usize).max(1) - 1).min(11)];
    format!("{y} {mname} {d:02}  {h:02}:{mi:02} UTC")
}

// ------------------------------------------------- planetary elements

pub struct PlanetDef {
    pub name: &'static str,
    /// a(AU), e, I(deg), L(deg), long.peri(deg), long.node(deg)
    pub el: [f64; 6],
    /// per-century rates of the above
    pub rate: [f64; 6],
    pub radius_km: f64,
    pub gm: f64, // m^3/s^2
    pub obliquity_deg: f64,
    pub rot_hours: f64, // sidereal, negative = retrograde
    pub color: [f32; 3],
    pub color2: [f32; 3],
    pub rim: [f32; 3],
    pub kind: f32,
    pub flatten: f32,
}

/// JPL SSD "Approximate Positions of the Planets" Table 1 (Standish),
/// valid 1800–2050. Earth entry is the Earth–Moon barycenter.
pub const PLANETS: [PlanetDef; 9] = [
    PlanetDef {
        name: "Mercury",
        el: [
            0.387_099_27,
            0.205_635_93,
            7.004_979_02,
            252.250_323_50,
            77.457_796_28,
            48.330_765_93,
        ],
        rate: [
            0.000_000_37,
            0.000_019_06,
            -0.005_947_49,
            149_472.674_111_75,
            0.160_476_89,
            -0.125_340_81,
        ],
        radius_km: 2_439.7,
        gm: 2.2032e13,
        obliquity_deg: 0.03,
        rot_hours: 1_407.6,
        color: [0.62, 0.58, 0.54],
        color2: [0.38, 0.35, 0.33],
        rim: [0.0, 0.0, 0.0],
        kind: 1.0,
        flatten: 0.0,
    },
    PlanetDef {
        name: "Venus",
        el: [
            0.723_335_66,
            0.006_776_72,
            3.394_676_05,
            181.979_099_50,
            131.602_467_18,
            76.679_842_55,
        ],
        rate: [
            0.000_003_90,
            -0.000_041_07,
            -0.000_788_90,
            58_517.815_387_29,
            0.002_683_29,
            -0.277_694_18,
        ],
        radius_km: 6_051.8,
        gm: 3.248_59e14,
        obliquity_deg: 177.4,
        rot_hours: -5_832.5,
        color: [0.93, 0.85, 0.66],
        color2: [0.80, 0.70, 0.52],
        rim: [0.95, 0.85, 0.6],
        kind: 2.0,
        flatten: 0.0,
    },
    PlanetDef {
        name: "Earth",
        el: [
            1.000_002_61,
            0.016_711_23,
            -0.000_015_31,
            100.464_571_66,
            102.937_681_93,
            0.0,
        ],
        rate: [
            0.000_005_62,
            -0.000_043_92,
            -0.012_946_68,
            35_999.372_449_81,
            0.323_273_64,
            0.0,
        ],
        radius_km: 6_371.0,
        gm: 3.986_004_418e14,
        obliquity_deg: 23.44,
        rot_hours: 23.934_5,
        color: [0.09, 0.22, 0.45],
        color2: [0.2, 0.32, 0.2],
        rim: [0.45, 0.65, 1.0],
        kind: 4.0,
        flatten: 0.0034,
    },
    PlanetDef {
        name: "Mars",
        el: [
            1.523_710_34,
            0.093_394_10,
            1.849_691_42,
            -4.553_432_05,
            -23.943_629_59,
            49.559_538_91,
        ],
        rate: [
            0.000_018_47,
            0.000_078_82,
            -0.008_131_31,
            19_140.302_684_99,
            0.444_410_88,
            -0.292_573_43,
        ],
        radius_km: 3_389.5,
        gm: 4.282_837_5e13,
        obliquity_deg: 25.19,
        rot_hours: 24.622_9,
        color: [0.78, 0.42, 0.26],
        color2: [0.48, 0.26, 0.18],
        rim: [0.9, 0.55, 0.35],
        kind: 1.0,
        flatten: 0.0059,
    },
    PlanetDef {
        name: "Jupiter",
        el: [
            5.202_887_00,
            0.048_386_24,
            1.304_396_95,
            34.396_440_51,
            14.728_479_83,
            100.473_909_09,
        ],
        rate: [
            -0.000_116_07,
            -0.000_132_53,
            -0.001_837_14,
            3_034.746_127_75,
            0.212_526_68,
            0.204_691_06,
        ],
        radius_km: 69_911.0,
        gm: 1.266_865_34e17,
        obliquity_deg: 3.13,
        rot_hours: 9.925,
        color: [0.83, 0.72, 0.55],
        color2: [0.62, 0.45, 0.34],
        rim: [0.9, 0.8, 0.6],
        kind: 2.0,
        flatten: 0.065,
    },
    PlanetDef {
        name: "Saturn",
        el: [
            9.536_675_94,
            0.053_861_79,
            2.485_991_87,
            49.954_244_23,
            92.598_878_31,
            113.662_424_48,
        ],
        rate: [
            -0.001_250_60,
            -0.000_509_91,
            0.001_936_09,
            1_222.493_622_01,
            -0.418_972_16,
            -0.288_677_94,
        ],
        radius_km: 58_232.0,
        gm: 3.793_118_7e16,
        obliquity_deg: 26.73,
        rot_hours: 10.656,
        color: [0.91, 0.83, 0.63],
        color2: [0.75, 0.66, 0.47],
        rim: [0.95, 0.88, 0.65],
        kind: 2.0,
        flatten: 0.098,
    },
    PlanetDef {
        name: "Uranus",
        el: [
            19.189_164_64,
            0.047_257_44,
            0.772_637_83,
            313.238_104_51,
            170.954_276_30,
            74.016_925_03,
        ],
        rate: [
            -0.001_961_76,
            -0.000_043_97,
            -0.002_429_39,
            428.482_027_85,
            0.408_052_81,
            0.042_405_89,
        ],
        radius_km: 25_362.0,
        gm: 5.793_939e15,
        obliquity_deg: 97.8,
        rot_hours: -17.24,
        color: [0.62, 0.85, 0.88],
        color2: [0.5, 0.72, 0.78],
        rim: [0.7, 0.95, 1.0],
        kind: 2.0,
        flatten: 0.023,
    },
    PlanetDef {
        name: "Neptune",
        el: [
            30.069_922_76,
            0.008_590_48,
            1.770_043_47,
            -55.120_029_69,
            44.964_762_27,
            131.784_225_74,
        ],
        rate: [
            0.000_262_91,
            0.000_051_05,
            0.000_353_72,
            218.459_453_25,
            -0.322_414_64,
            -0.005_086_64,
        ],
        radius_km: 24_622.0,
        gm: 6.836_529e15,
        obliquity_deg: 28.3,
        rot_hours: 16.11,
        color: [0.32, 0.45, 0.92],
        color2: [0.2, 0.3, 0.7],
        rim: [0.5, 0.65, 1.0],
        kind: 2.0,
        flatten: 0.017,
    },
    PlanetDef {
        name: "Pluto",
        el: [
            39.482_116_75,
            0.248_827_30,
            17.140_012_06,
            238.929_038_33,
            224.068_916_29,
            110.303_936_84,
        ],
        rate: [
            -0.000_315_96,
            0.000_051_70,
            0.000_048_18,
            145.207_805_15,
            -0.040_629_42,
            -0.011_834_82,
        ],
        radius_km: 1_188.3,
        gm: 8.71e11,
        obliquity_deg: 122.5,
        rot_hours: -153.29,
        color: [0.78, 0.68, 0.58],
        color2: [0.55, 0.46, 0.4],
        rim: [0.0, 0.0, 0.0],
        kind: 5.0,
        flatten: 0.0,
    },
];

pub const SUN_RADIUS_KM: f64 = 695_700.0;

fn kepler_solve(m: f64, e: f64) -> f64 {
    let mut ea = if e < 0.8 { m } else { std::f64::consts::PI };
    for _ in 0..12 {
        let f = ea - e * ea.sin() - m;
        let fp = 1.0 - e * ea.cos();
        ea -= f / fp;
    }
    ea
}

/// Rotation from perifocal to ecliptic given node Ω, inclination i, arg.peri ω (radians).
fn perifocal_to_ecl(node: f64, inc: f64, argp: f64) -> DMat3 {
    DMat3::from_rotation_z(node) * DMat3::from_rotation_x(inc) * DMat3::from_rotation_z(argp)
}

/// The span Standish's linear rates are fitted to, in Julian centuries from J2000:
/// 1800 to 2050. Mars's eccentricity would pass 1 near the year 1.15 million.
pub const ELEMENTS_VALID_CENTURIES: (f64, f64) = (-2.0, 0.5);

/// A planet's orbit at `jd`: semi-major axis (m), eccentricity, and inclination, longitude
/// of perihelion and node (rad). Outside the table's span these hold at the nearest limit,
/// so an orbit stays a real ellipse at any date. Only the mean longitude keeps running.
struct OrbitShape {
    a: f64,
    e: f64,
    inc: f64,
    lp: f64,
    node: f64,
}

fn orbit_shape(p: &PlanetDef, jd: f64) -> OrbitShape {
    let t = ((jd - J2000) / 36_525.0).clamp(ELEMENTS_VALID_CENTURIES.0, ELEMENTS_VALID_CENTURIES.1);
    OrbitShape {
        a: (p.el[0] + p.rate[0] * t) * AU,
        e: p.el[1] + p.rate[1] * t,
        inc: (p.el[2] + p.rate[2] * t).to_radians(),
        lp: (p.el[4] + p.rate[4] * t).to_radians(),
        node: (p.el[5] + p.rate[5] * t).to_radians(),
    }
}

/// Mean anomaly at `jd` given the orbit's longitude of perihelion (rad).
fn mean_anomaly(p: &PlanetDef, jd: f64, lp: f64) -> f64 {
    let t = (jd - J2000) / 36_525.0;
    ((p.el[3] + p.rate[3] * t).to_radians() - lp) % std::f64::consts::TAU
}

/// Heliocentric position (m) of planet `i` at Julian date `jd`.
pub fn planet_pos(i: usize, jd: f64) -> DVec3 {
    let p = &PLANETS[i];
    let OrbitShape {
        a,
        e,
        inc,
        lp,
        node,
    } = orbit_shape(p, jd);
    let argp = lp - node;
    let ea = kepler_solve(mean_anomaly(p, jd, lp), e);
    let x = a * (ea.cos() - e);
    let y = a * (1.0 - e * e).sqrt() * ea.sin();
    perifocal_to_ecl(node, inc, argp) * DVec3::new(x, y, 0.0)
}

/// Velocity (m/s) by central finite difference of the element evaluation.
pub fn planet_vel(i: usize, jd: f64) -> DVec3 {
    let dt = 120.0; // seconds
    (planet_pos(i, jd + dt / DAY) - planet_pos(i, jd - dt / DAY)) / (2.0 * dt)
}

/// GM_moon / (GM_earth + GM_moon): Earth-center offset fraction from the EMB.
pub const MU_MOON_FRAC: f64 = 4.902_8e12 / (3.986_004_418e14 + 4.902_8e12);

/// The JPL table's "Earth" row is the Earth–Moon barycenter.
/// The Earth **center** is EMB minus the mass-weighted geocentric Moon vector.
pub fn earth_center(jd: f64) -> DVec3 {
    planet_pos(2, jd) - luna_rel(jd) * MU_MOON_FRAC
}

/// Barycenter-level NAIF ids for [`PLANETS`] (always in de440s).
pub const NAIF_IDS: [i32; 9] = [199, 299, 399, 4, 5, 6, 7, 8, 9];
/// True planet-center ids — resolved when a satellite kernel provides the
/// center-relative-barycenter segment (mar097: 499, jup365: 599, …).
pub const PLANET_CENTER_IDS: [i32; 9] = [199, 299, 399, 499, 599, 699, 799, 899, 999];
/// Satellite NAIF ids aligned with [`MOONS`].
pub const MOON_NAIF: [i32; 21] = [
    301, 401, 402, 501, 502, 503, 504, 601, 602, 603, 604, 605, 606, 608, 705, 701, 702, 703, 704,
    801, 901,
];

/// True when the SPICE kernels cover this epoch (the state source shown
/// in inspectors depends on it).
pub fn eph_is_spice(jd: f64) -> bool {
    crate::spice::spice().map(|s| s.covers(jd)).unwrap_or(false)
}

/// Heliocentric position of a body by index. Prefers the DE440s SPICE
/// kernel; falls back to the analytic elements outside coverage
/// or when kernels are absent (Earth resolved to its true center).
pub fn body_pos(i: usize, jd: f64) -> DVec3 {
    // Only inside de440s's coverage. Satellite kernels run past it and also carry a few
    // planets, but the labels say elements out there, and a mix of the two would put Earth
    // on one ephemeris and Venus on another.
    if let Some(s) = crate::spice::spice().filter(|s| s.covers(jd)) {
        // true planet center when a satellite kernel carries it, else the
        // system barycenter from de440s (offset ≤ ~10³ km)
        if let Some(p) = s.helio_ecl_m(PLANET_CENTER_IDS[i], jd) {
            return p;
        }
        if let Some(p) = s.helio_ecl_m(NAIF_IDS[i], jd) {
            return p;
        }
    }
    if i == 2 {
        earth_center(jd)
    } else {
        planet_pos(i, jd)
    }
}

/// True when this moon's state comes from a satellite SPK at this epoch.
pub fn moon_is_spice(mi: usize, jd: f64) -> bool {
    crate::spice::spice()
        .map(|s| s.has(MOON_NAIF[mi], jd))
        .unwrap_or(false)
}

/// Sampled full orbit ellipse (root frame) for engraving, at the osculating epoch.
pub fn planet_orbit(i: usize, jd: f64, n: usize) -> Vec<DVec3> {
    let OrbitShape {
        a,
        e,
        inc,
        lp,
        node,
    } = orbit_shape(&PLANETS[i], jd);
    let rot = perifocal_to_ecl(node, inc, lp - node);
    (0..n)
        .map(|k| {
            let ea = k as f64 / n as f64 * std::f64::consts::TAU;
            rot * DVec3::new(a * (ea.cos() - e), a * (1.0 - e * e).sqrt() * ea.sin(), 0.0)
        })
        .collect()
}

/// One point of the osculating orbit ellipse at eccentric anomaly `ea`.
pub fn planet_orbit_point(i: usize, jd: f64, ea: f64) -> DVec3 {
    let OrbitShape {
        a,
        e,
        inc,
        lp,
        node,
    } = orbit_shape(&PLANETS[i], jd);
    perifocal_to_ecl(node, inc, lp - node)
        * DVec3::new(a * (ea.cos() - e), a * (1.0 - e * e).sqrt() * ea.sin(), 0.0)
}

/// Eccentric anomaly of the planet on its osculating orbit at `jd`.
pub fn planet_eccentric_anomaly(i: usize, jd: f64) -> f64 {
    let p = &PLANETS[i];
    let shape = orbit_shape(p, jd);
    kepler_solve(mean_anomaly(p, jd, shape.lp), shape.e)
}

// ------------------------------------------------------------- moons

pub struct MoonDef {
    pub planet: usize,
    pub name: &'static str,
    pub a_km: f64,
    pub period_d: f64, // negative = retrograde
    pub radius_km: f64,
    pub e: f64,
    pub m0_deg: f64, // mean anomaly at J2000 (illustrative phase)
    pub color: [f32; 3],
    pub color2: [f32; 3],
    pub kind: f32,
}

/// Mean elements: JPL planetary satellite tables, circular-inclined
/// approximation (status=modeled; orbital plane approximated by the parent's
/// equatorial plane, phases illustrative except the Moon).
pub const MOONS: [MoonDef; 21] = [
    MoonDef {
        planet: 2,
        name: "Moon",
        a_km: 384_400.0,
        period_d: 27.321_661,
        radius_km: 1_737.4,
        e: 0.0549,
        m0_deg: 135.27,
        color: [0.62, 0.62, 0.6],
        color2: [0.42, 0.42, 0.42],
        kind: 5.0,
    },
    MoonDef {
        planet: 3,
        name: "Phobos",
        a_km: 9_376.0,
        period_d: 0.318_9,
        radius_km: 11.3,
        e: 0.0151,
        m0_deg: 30.0,
        color: [0.55, 0.5, 0.46],
        color2: [0.35, 0.32, 0.3],
        kind: 5.0,
    },
    MoonDef {
        planet: 3,
        name: "Deimos",
        a_km: 23_463.0,
        period_d: 1.262_4,
        radius_km: 6.2,
        e: 0.0002,
        m0_deg: 200.0,
        color: [0.58, 0.53, 0.48],
        color2: [0.38, 0.35, 0.32],
        kind: 5.0,
    },
    MoonDef {
        planet: 4,
        name: "Io",
        a_km: 421_800.0,
        period_d: 1.769_138,
        radius_km: 1_821.6,
        e: 0.0041,
        m0_deg: 20.0,
        color: [0.92, 0.82, 0.45],
        color2: [0.7, 0.55, 0.3],
        kind: 1.0,
    },
    MoonDef {
        planet: 4,
        name: "Europa",
        a_km: 671_100.0,
        period_d: 3.551_181,
        radius_km: 1_560.8,
        e: 0.0094,
        m0_deg: 140.0,
        color: [0.85, 0.82, 0.75],
        color2: [0.65, 0.6, 0.55],
        kind: 5.0,
    },
    MoonDef {
        planet: 4,
        name: "Ganymede",
        a_km: 1_070_400.0,
        period_d: 7.154_553,
        radius_km: 2_634.1,
        e: 0.0013,
        m0_deg: 260.0,
        color: [0.62, 0.58, 0.52],
        color2: [0.45, 0.42, 0.38],
        kind: 5.0,
    },
    MoonDef {
        planet: 4,
        name: "Callisto",
        a_km: 1_882_700.0,
        period_d: 16.689_017,
        radius_km: 2_410.3,
        e: 0.0074,
        m0_deg: 80.0,
        color: [0.5, 0.46, 0.42],
        color2: [0.32, 0.3, 0.28],
        kind: 5.0,
    },
    MoonDef {
        planet: 5,
        name: "Mimas",
        a_km: 185_540.0,
        period_d: 0.942_422,
        radius_km: 198.2,
        e: 0.0196,
        m0_deg: 10.0,
        color: [0.7, 0.7, 0.68],
        color2: [0.5, 0.5, 0.48],
        kind: 5.0,
    },
    MoonDef {
        planet: 5,
        name: "Enceladus",
        a_km: 238_040.0,
        period_d: 1.370_218,
        radius_km: 252.1,
        e: 0.0047,
        m0_deg: 90.0,
        color: [0.92, 0.94, 0.96],
        color2: [0.75, 0.78, 0.82],
        kind: 5.0,
    },
    MoonDef {
        planet: 5,
        name: "Tethys",
        a_km: 294_670.0,
        period_d: 1.887_802,
        radius_km: 531.1,
        e: 0.0001,
        m0_deg: 170.0,
        color: [0.78, 0.78, 0.76],
        color2: [0.58, 0.58, 0.56],
        kind: 5.0,
    },
    MoonDef {
        planet: 5,
        name: "Dione",
        a_km: 377_420.0,
        period_d: 2.736_915,
        radius_km: 561.4,
        e: 0.0022,
        m0_deg: 250.0,
        color: [0.75, 0.75, 0.73],
        color2: [0.55, 0.55, 0.53],
        kind: 5.0,
    },
    MoonDef {
        planet: 5,
        name: "Rhea",
        a_km: 527_070.0,
        period_d: 4.517_5,
        radius_km: 763.8,
        e: 0.001,
        m0_deg: 330.0,
        color: [0.72, 0.72, 0.7],
        color2: [0.52, 0.52, 0.5],
        kind: 5.0,
    },
    MoonDef {
        planet: 5,
        name: "Titan",
        a_km: 1_221_870.0,
        period_d: 15.945_421,
        radius_km: 2_574.7,
        e: 0.0288,
        m0_deg: 60.0,
        color: [0.88, 0.68, 0.4],
        color2: [0.7, 0.5, 0.3],
        kind: 2.0,
    },
    MoonDef {
        planet: 5,
        name: "Iapetus",
        a_km: 3_560_840.0,
        period_d: 79.330_183,
        radius_km: 734.5,
        e: 0.0293,
        m0_deg: 120.0,
        color: [0.66, 0.62, 0.55],
        color2: [0.25, 0.22, 0.2],
        kind: 5.0,
    },
    MoonDef {
        planet: 6,
        name: "Miranda",
        a_km: 129_900.0,
        period_d: 1.413_479,
        radius_km: 235.8,
        e: 0.0013,
        m0_deg: 15.0,
        color: [0.68, 0.7, 0.72],
        color2: [0.5, 0.52, 0.54],
        kind: 5.0,
    },
    MoonDef {
        planet: 6,
        name: "Ariel",
        a_km: 190_900.0,
        period_d: 2.520_379,
        radius_km: 578.9,
        e: 0.0012,
        m0_deg: 100.0,
        color: [0.7, 0.72, 0.74],
        color2: [0.52, 0.54, 0.56],
        kind: 5.0,
    },
    MoonDef {
        planet: 6,
        name: "Umbriel",
        a_km: 266_000.0,
        period_d: 4.144_177,
        radius_km: 584.7,
        e: 0.0039,
        m0_deg: 190.0,
        color: [0.5, 0.52, 0.55],
        color2: [0.35, 0.37, 0.4],
        kind: 5.0,
    },
    MoonDef {
        planet: 6,
        name: "Titania",
        a_km: 436_300.0,
        period_d: 8.705_872,
        radius_km: 788.9,
        e: 0.0011,
        m0_deg: 280.0,
        color: [0.65, 0.67, 0.7],
        color2: [0.48, 0.5, 0.52],
        kind: 5.0,
    },
    MoonDef {
        planet: 6,
        name: "Oberon",
        a_km: 583_500.0,
        period_d: 13.463_239,
        radius_km: 761.4,
        e: 0.0014,
        m0_deg: 340.0,
        color: [0.6, 0.6, 0.62],
        color2: [0.44, 0.44, 0.46],
        kind: 5.0,
    },
    // retrograde motion is encoded by the 156.9° inclination in MOON_INCL,
    // NOT by a negative period (never encode it twice)
    MoonDef {
        planet: 7,
        name: "Triton",
        a_km: 354_759.0,
        period_d: 5.876_854,
        radius_km: 1_353.4,
        e: 0.000_02,
        m0_deg: 50.0,
        color: [0.78, 0.8, 0.85],
        color2: [0.6, 0.62, 0.68],
        kind: 5.0,
    },
    MoonDef {
        planet: 8,
        name: "Charon",
        a_km: 19_591.0,
        period_d: 6.387_23,
        radius_km: 606.0,
        e: 0.0002,
        m0_deg: 220.0,
        color: [0.6, 0.58, 0.56],
        color2: [0.42, 0.4, 0.38],
        kind: 5.0,
    },
];

/// IAU 2015 rotation elements per planet (aligned with [`PLANETS`]):
/// pole α0 (deg), α̇ (deg/cy), δ0 (deg), δ̇ (deg/cy), W0 (deg), Ẇ (deg/day).
/// Small trigonometric series (Neptune's pole nutation, lunar libration) are
/// omitted — documented approximation, status=modeled.
pub const IAU_ROT: [[f64; 6]; 9] = [
    [
        281.010_3,
        -0.032_8,
        61.415_5,
        -0.004_9,
        329.598_8,
        6.138_510_8,
    ], // Mercury
    [272.76, 0.0, 67.16, 0.0, 160.20, -1.481_368_8], // Venus (retrograde)
    [0.0, -0.641, 90.0, -0.557, 190.147, 360.985_623_5], // Earth
    [
        317.269_202,
        -0.109_275,
        54.432_516,
        -0.058_271,
        176.049_863,
        350.891_982_443,
    ], // Mars
    [
        268.056_595,
        -0.006_499,
        64.495_303,
        0.002_413,
        284.95,
        870.536,
    ], // Jupiter (Sys III)
    [40.589, -0.036, 83.537, -0.004, 38.90, 810.793_902_4], // Saturn
    [257.311, 0.0, -15.175, 0.0, 203.81, -501.160_092_8], // Uranus (retrograde)
    [299.36, 0.0, 43.46, 0.0, 249.978, 541.139_775_7], // Neptune
    [132.993, 0.0, -6.163, 0.0, 302.695, 56.362_522_5], // Pluto
];

/// Body equatorial basis in the root frame at jd (S3): columns are the node
/// direction Q (body equator × ICRS equator), Q rotated 90° in the equator,
/// and the IAU pole. Moon orbits and rings live in this plane.
pub fn body_equator(pi: usize, jd: f64) -> DMat3 {
    let r = &IAU_ROT[pi];
    let t = (jd - J2000) / 36_525.0;
    let alpha = (r[0] + r[1] * t).to_radians();
    let delta = (r[2] + r[3] * t).to_radians();
    let m = crate::frames::ecl_from_icrs();
    let pole = m * DVec3::new(
        delta.cos() * alpha.cos(),
        delta.cos() * alpha.sin(),
        delta.sin(),
    );
    let z_icrs = m * DVec3::Z;
    let mut q = z_icrs.cross(pole);
    if q.length_squared() < 1e-12 {
        q = DVec3::X;
    }
    let q = q.normalize();
    DMat3::from_cols(q, pole.cross(q), pole)
}

/// Full body-fixed basis: x = prime meridian direction (node rotated by the
/// IAU angle W along the equator), z = pole. Surfaces (coastlines) use this.
pub fn body_basis(pi: usize, jd: f64) -> DMat3 {
    let eq = body_equator(pi, jd);
    let w = body_w(pi, jd);
    let (q, y, pole) = (eq.col(0), eq.col(1), eq.col(2));
    let x = q * w.cos() + y * w.sin();
    DMat3::from_cols(x, pole.cross(x), pole)
}

/// IAU prime-meridian angle W (radians) at jd.
pub fn body_w(pi: usize, jd: f64) -> f64 {
    let r = &IAU_ROT[pi];
    (r[4] + r[5] * (jd - J2000)).to_radians()
}

/// Spin axis unit vector in the root frame.
pub fn planet_axis(pi: usize, jd: f64) -> DVec3 {
    body_equator(pi, jd).col(2)
}

/// Inclination of each moon's orbit to its parent's equator (deg), aligned
/// with [`MOONS`]. Node longitudes are approximate (0 in the equator frame).
/// Triton's retrograde 157° is the one that matters most (S4).
pub const MOON_INCL: [f64; 21] = [
    0.0, 1.1, 1.8, 0.04, 0.47, 0.2, 0.2, 1.57, 0.01, 1.1, 0.03, 0.35, 0.35, 15.47, 4.34, 0.04,
    0.13, 0.08, 0.07, 156.9, 0.08,
];

/// Moon position relative to its planet (m, root frame): Kepler ellipse in
/// the parent's IAU equator, tilted by the moon's own inclination.
pub fn moon_rel(mi: usize, jd: f64) -> DVec3 {
    let m = &MOONS[mi];
    // real satellite ephemerides when a kernel covers this moon (Moon via
    // de440s 301; Galileans via jup365; Phobos/Deimos via mar097; …)
    if let Some(s) = crate::spice::spice() {
        if let Some(rel) = s.rel_ecl_m(MOON_NAIF[mi], PLANET_CENTER_IDS[m.planet], jd) {
            return rel;
        }
    }
    moon_rel_elements(mi, jd)
}

/// The tilt of a moon's orbit from its planet's IAU pole, as used to place it. Uranus's IAU
/// pole points against its spin, and its moons orbit with the spin, so their orbits are
/// nearly 180° from that pole. `MOON_INCL` stays what a card shows: the tilt from the equator.
pub fn moon_plane_tilt_deg(mi: usize) -> f64 {
    if MOONS[mi].planet == URANUS {
        180.0 - MOON_INCL[mi]
    } else {
        MOON_INCL[mi]
    }
}

const URANUS: usize = 6;

/// Where a moon is without any kernel: the lunar model, or mean elements.
fn moon_rel_elements(mi: usize, jd: f64) -> DVec3 {
    let m = &MOONS[mi];
    if m.name == "Moon" {
        return luna_rel(jd);
    }
    let n = std::f64::consts::TAU / (m.period_d * DAY);
    let ma = m.m0_deg.to_radians() + n * (jd - J2000) * DAY;
    let ea = kepler_solve(ma % std::f64::consts::TAU, m.e);
    let a = m.a_km * 1e3;
    let local = DVec3::new(
        a * (ea.cos() - m.e),
        a * (1.0 - m.e * m.e).sqrt() * ea.sin(),
        0.0,
    );
    body_equator(m.planet, jd)
        * (DMat3::from_rotation_x(moon_plane_tilt_deg(mi).to_radians()) * local)
}

/// Sampled moon orbit relative to planet. The Moon samples the same
/// precessing model as its position (line matches marker).
pub fn moon_orbit(mi: usize, jd: f64, n: usize) -> Vec<DVec3> {
    let m = &MOONS[mi];
    if m.name == "Moon" {
        let p = m.period_d;
        return (0..n)
            .map(|k| luna_rel(jd + p * k as f64 / n as f64))
            .collect();
    }
    let a = m.a_km * 1e3;
    let rot =
        body_equator(m.planet, jd) * DMat3::from_rotation_x(moon_plane_tilt_deg(mi).to_radians());
    (0..n)
        .map(|k| {
            let ea = k as f64 / n as f64 * std::f64::consts::TAU;
            rot * DVec3::new(
                a * (ea.cos() - m.e),
                a * (1.0 - m.e * m.e).sqrt() * ea.sin(),
                0.0,
            )
        })
        .collect()
}

/// Authoritative GMs (m³/s²) aligned with [`MOONS`] by index — JPL satellite
/// solutions (replaces radius/density guesses in islands).
pub const MOON_GM: [f64; 21] = [
    4.902_800_066e12, // Moon
    7.112e5,          // Phobos
    9.85e4,           // Deimos
    5.959_916e12,     // Io
    3.202_739e12,     // Europa
    9.887_834e12,     // Ganymede
    7.179_289e12,     // Callisto
    2.503e9,          // Mimas
    7.211e9,          // Enceladus
    4.121e10,         // Tethys
    7.311_3e10,       // Dione
    1.539_4e11,       // Rhea
    8.978_14e12,      // Titan
    1.205_1e11,       // Iapetus
    4.4e9,            // Miranda
    8.64e10,          // Ariel
    8.15e10,          // Umbriel
    2.282e11,         // Titania
    1.924e11,         // Oberon
    1.428_495e12,     // Triton
    1.058_8e11,       // Charon
];

/// Earth's Moon with precessing node (18.6 yr) and perigee (8.85 yr) —
/// a readable mid-fidelity model (status=modeled). It leaves out evection and the
/// other periodic terms: from 1950 to 2050 its direction is about 1° from DE440s on
/// average and 3.2° at worst, as the tests below hold it to, and worse outside that.
pub fn luna_rel(jd: f64) -> DVec3 {
    let d = jd - J2000;
    let a = 384_400.0e3;
    let e = 0.0549;
    let inc = 5.145_f64.to_radians();
    let node = (125.08_f64 - 0.052_953_8 * d).to_radians();
    // the rate of the argument of perigee: added to the node's, it gives the 8.85-year
    // period of the perigee itself
    let argp = (318.15_f64 + 0.164_357_3 * d).to_radians();
    let ma = (135.27_f64 + 13.064_993 * d).to_radians();
    let ea = kepler_solve(ma % std::f64::consts::TAU, e);
    let local = DVec3::new(a * (ea.cos() - e), a * (1.0 - e * e).sqrt() * ea.sin(), 0.0);
    perifocal_to_ecl(node, inc, argp) * local
}

#[cfg(test)]
mod iau_tests {
    use super::*;

    #[test]
    fn earth_subsolar_longitude_near_zero_at_utc_noon() {
        // At 12:00 UTC the subsolar point sits near Greenwich (± equation of
        // time, ~4°). This pins pole + prime meridian + time scales together.
        let jd = tdb_from_utc_jd(cal_to_jd(2026, 7, 11.5));
        let s = (-earth_center(jd)).normalize();
        let b = body_basis(2, jd);
        let lon = s.dot(b.col(1)).atan2(s.dot(b.col(0))).to_degrees();
        assert!(lon.abs() < 7.0, "subsolar longitude {lon}°");
    }

    #[test]
    fn earth_pole_matches_obliquity() {
        let pole = planet_axis(2, J2000);
        let lat = pole.z.asin().to_degrees();
        assert!((lat - 66.56).abs() < 0.1, "pole ecliptic latitude {lat}°");
    }

    #[test]
    fn triton_orbit_is_retrograde_in_neptune_frame() {
        let jd = J2000 + 5_000.0;
        let ti = MOONS.iter().position(|m| m.name == "Triton").unwrap();
        let r0 = moon_rel(ti, jd);
        let r1 = moon_rel(ti, jd + 0.01);
        let pole = planet_axis(7, jd);
        // angular momentum opposes the planet's spin pole
        assert!(r0.cross(r1).dot(pole) < 0.0);
    }
}

// -------------------------------------------------------- ring systems

pub struct RingDef {
    pub planet: usize,
    /// (inner km, outer km, relative density 0..1)
    pub zones: &'static [(f64, f64, f32)],
}

pub const RINGS: [RingDef; 2] = [
    RingDef {
        planet: 5,
        zones: &[
            (74_658.0, 92_000.0, 0.35),   // C
            (92_000.0, 117_580.0, 1.0),   // B
            (122_170.0, 136_775.0, 0.7),  // A (Cassini division gap before)
            (140_180.0, 140_680.0, 0.25), // F
        ],
    },
    RingDef {
        planet: 6,
        zones: &[(51_149.0, 51_650.0, 0.3)],
    }, // ε ring
];

// ------------------------------------------------------ mechanics

/// Hill sphere radius (m) of a secondary of gm2 orbiting gm1 at distance a.
pub fn hill_radius(a: f64, gm1: f64, gm2: f64) -> f64 {
    a * (gm2 / (3.0 * gm1)).powf(1.0 / 3.0)
}

/// Fluid Roche limit (m) for a satellite of density rho_s around a body of
/// radius r_p and density rho_p.
pub fn roche_limit(r_p: f64, rho_p: f64, rho_s: f64) -> f64 {
    2.44 * r_p * (rho_p / rho_s).powf(1.0 / 3.0)
}

/// Collinear Lagrange points L1..L3 plus L4/L5 for mass ratio mu = m2/(m1+m2),
/// in units of the separation, in the rotating frame (primary at -mu, secondary
/// at 1-mu, barycentric).
pub fn lagrange_points(mu: f64) -> [[f64; 2]; 5] {
    let omega_x = |x: f64| -> f64 {
        let r1 = (x + mu).abs().max(1e-9);
        let r2 = (x - 1.0 + mu).abs().max(1e-9);
        x - (1.0 - mu) * (x + mu) / (r1 * r1 * r1) - mu * (x - 1.0 + mu) / (r2 * r2 * r2)
    };
    let solve = |mut lo: f64, mut hi: f64| -> f64 {
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if omega_x(lo) * omega_x(mid) <= 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        0.5 * (lo + hi)
    };
    let l1 = solve(-mu + 1e-6 + (1.0 - mu) * 0.3, 1.0 - mu - 1e-6);
    let l2 = solve(1.0 - mu + 1e-6, 2.5);
    let l3 = solve(-2.5, -mu - 1e-6);
    [
        [l1, 0.0],
        [l2, 0.0],
        [l3, 0.0],
        [0.5 - mu, 0.866_025_4],
        [0.5 - mu, -0.866_025_4],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_scales_roundtrip_and_offset() {
        let utc = 2_461_233.0;
        let tdb = tdb_from_utc_jd(utc);
        assert!((utc_from_tdb_jd(tdb) - utc).abs() < 1e-12);
        // f64 ULP at JD ~2.46e6 is ~40 µs; 1 ms is far below any model error
        assert!(((tdb - utc) * 86_400.0 - 69.184).abs() < 1e-3);
    }

    #[test]
    fn calendar_roundtrip() {
        let jd = cal_to_jd(2026, 7, 11.5);
        let (y, m, d, h, _) = jd_to_cal(jd);
        assert_eq!((y, m, d, h), (2026, 7, 11, 12));
    }

    #[test]
    fn earth_orbit_scale_sane() {
        let r = planet_pos(2, J2000).length() / crate::frames::AU;
        assert!((0.97..1.03).contains(&r), "EMB radius {r} AU");
    }

    #[test]
    fn earth_center_offset_from_emb_is_moon_scale() {
        // Earth center sits ~4,670 km from the EMB (μ · a_moon)
        let jd = J2000 + 9_000.0;
        let off = (earth_center(jd) - planet_pos(2, jd)).length() / 1e3;
        assert!((3_800.0..5_100.0).contains(&off), "offset {off} km");
    }

    #[test]
    fn hermite_beats_linear_on_synthetic_orbit() {
        // circular Earth-like orbit sampled at 5-day cadence
        use glam::DVec3;
        let om = std::f64::consts::TAU / 365.25; // rad/day
        let r = crate::frames::AU;
        let state = |t: f64| {
            (
                DVec3::new(r * (om * t).cos(), r * (om * t).sin(), 0.0),
                DVec3::new(-r * om * (om * t).sin(), r * om * (om * t).cos(), 0.0),
            )
        };
        let samples: Vec<(f64, DVec3, DVec3)> = (0..20)
            .map(|k| {
                let t = k as f64 * 5.0;
                let (p, v) = state(t);
                (t, p, v)
            })
            .collect();
        let track = crate::data::Track { samples };
        let (exact, _) = state(2.5);
        let err_km = (track.pos_at(2.5).unwrap() - exact).length() / 1e3;
        assert!(err_km < 60.0, "hermite midpoint error {err_km} km");
        // outside coverage must refuse
        assert!(track.pos_at(-1.0).is_none());
        assert!(track.pos_at(1e9).is_none());
    }

    #[test]
    fn lagrange_points_earth_moon_sane() {
        let mu = 0.012_15;
        let l = lagrange_points(mu);
        // L1 ~0.836, L2 ~1.156 of the separation from the barycenter
        assert!((l[0][0] - 0.836).abs() < 0.01, "L1 = {}", l[0][0]);
        assert!((l[1][0] - 1.156).abs() < 0.01, "L2 = {}", l[1][0]);
    }
}

// ------------------------------------------------------ N-body island

#[derive(Clone)]
pub struct NBody {
    pub gm: Vec<f64>,
    pub pos: Vec<DVec3>,
    pub vel: Vec<DVec3>,
    pub label: String,
}

impl NBody {
    /// Each moon's orbital energy per unit mass about its planet, the body at `planet`: what a
    /// moon's orbit keeps constant, apart from tides from the Sun and the other moons. The
    /// system's total energy can't show this: the Sun's term swamps everything, and a large moon
    /// swamps a small one.
    pub fn moon_orbit_energies(&self, planet: usize) -> Vec<f64> {
        ((planet + 1)..self.pos.len())
            .map(|i| {
                let (dr, dv) = (
                    self.pos[i] - self.pos[planet],
                    self.vel[i] - self.vel[planet],
                );
                0.5 * dv.length_squared() - self.gm[planet] / dr.length().max(1.0)
            })
            .collect()
    }

    /// The largest relative change in any moon's orbital energy since `start`. `None` if
    /// there are no moons.
    pub fn moon_energy_drift(&self, planet: usize, start: &[f64]) -> Option<f64> {
        self.moon_orbit_energies(planet)
            .iter()
            .zip(start)
            .map(|(e, e0)| ((e - e0) / e0).abs())
            .reduce(f64::max)
    }

    /// Remove the total momentum so the island barycenter stays put
    pub fn zero_momentum(&mut self) {
        let gm_total: f64 = self.gm.iter().sum();
        if gm_total <= 0.0 {
            return;
        }
        let mut p = DVec3::ZERO;
        for i in 0..self.pos.len() {
            p += self.vel[i] * self.gm[i];
        }
        let v_com = p / gm_total;
        for v in &mut self.vel {
            *v -= v_com;
        }
    }

    /// Leapfrog (kick-drift-kick); dt seconds, substeps chosen by caller.
    pub fn step(&mut self, dt: f64) {
        let n = self.pos.len();
        let acc = |pos: &Vec<DVec3>, gm: &Vec<f64>| -> Vec<DVec3> {
            let mut a = vec![DVec3::ZERO; n];
            for i in 0..n {
                for j in 0..n {
                    if i == j {
                        continue;
                    }
                    let d = pos[j] - pos[i];
                    let r2 = d.length_squared().max(1.0);
                    a[i] += d * (gm[j] / (r2 * r2.sqrt()));
                }
            }
            a
        };
        let a0 = acc(&self.pos, &self.gm);
        for (i, a) in a0.iter().enumerate() {
            self.vel[i] += *a * (0.5 * dt);
            self.pos[i] += self.vel[i] * dt;
        }
        let a1 = acc(&self.pos, &self.gm);
        for (v, a) in self.vel.iter_mut().zip(&a1) {
            *v += *a * (0.5 * dt);
        }
    }
}

#[cfg(test)]
mod luna_tests {
    use super::*;

    /// (TDB Julian date, unit vector Earth to Moon, ecliptic J2000), read from JPL's
    /// DE440s through `SpiceSet::rel_ecl_m` and frozen so these tests need no data. These
    /// six epochs are where the double-counted node drift was found and measured.
    const DE440S_EPOCHS: [(f64, [f64; 3]); 6] = [
        (2451545.000, [-0.724585, -0.683267, 0.090126]), // 2000
        (2453371.250, [-0.912617, 0.402794, 0.069911]),  // 2005
        (2455197.500, [-0.226443, 0.973943, 0.012618]),  // 2010
        (2457023.750, [0.591268, 0.804623, -0.054635]),  // 2015
        (2458850.000, [0.985899, -0.142064, -0.088440]), // 2020
        (2461351.962, [-0.914581, -0.397666, -0.073505]), // 2026.85
    ];

    /// The same, every 915.37 days from 1950 to 2047, which lands on many lunar phases.
    const DE440S_SPREAD: [(f64, [f64; 3]); 40] = [
        (2433282.500, [0.466744, 0.881922, 0.066058]),
        (2434197.870, [-0.501935, -0.860247, -0.089648]),
        (2435113.240, [0.387730, 0.920677, 0.044948]),
        (2436028.610, [-0.271906, -0.961570, 0.038091]),
        (2436943.980, [0.319578, 0.943831, -0.083979]),
        (2437859.350, [-0.412340, -0.907302, 0.082329]),
        (2438774.720, [0.383938, 0.923025, -0.024821]),
        (2439690.090, [-0.220328, -0.973557, -0.060354]),
        (2440605.460, [0.177040, 0.980173, 0.088982]),
        (2441520.830, [-0.275758, -0.959204, -0.062335]),
        (2442436.200, [0.339697, 0.940534, -0.001416]),
        (2443351.570, [-0.212873, -0.974177, 0.075257]),
        (2444266.940, [0.074476, 0.993827, -0.082232]),
        (2445182.310, [-0.116881, -0.992639, 0.031725]),
        (2446097.680, [0.234450, 0.971562, 0.033175]),
        (2447013.050, [-0.204356, -0.975119, -0.085917]),
        (2447928.420, [0.031835, 0.997104, 0.069067]),
        (2448843.790, [0.027629, -0.999615, 0.002496]),
        (2449759.160, [0.081368, 0.994669, -0.063349]),
        (2450674.530, [-0.147856, -0.984795, 0.091197]),
        (2451589.900, [0.033216, 0.998022, -0.053372]),
        (2452505.270, [0.124216, -0.991716, -0.032704]),
        (2453420.640, [-0.085750, 0.992791, 0.083739]),
        (2454336.010, [-0.030257, -0.995855, -0.085771]),
        (2455251.380, [0.026863, 0.999088, 0.033200]),
        (2456166.750, [0.158944, -0.985738, 0.055301]),
        (2457082.120, [-0.227861, 0.969455, -0.090755]),
        (2457997.490, [0.128861, -0.989407, 0.066851]),
        (2458912.860, [-0.035167, 0.999367, -0.005453]),
        (2459828.230, [0.154965, -0.985339, -0.071361]),
        (2460743.600, [-0.315660, 0.944992, 0.085731]),
        (2461658.970, [0.292752, -0.955492, -0.036485]),
        (2462574.340, [-0.160215, 0.986667, -0.028634]),
        (2463489.710, [0.166089, -0.982493, 0.084391]),
        (2464405.080, [-0.341927, 0.936761, -0.074599]),
        (2465320.450, [0.424101, -0.905611, 0.002675]),
        (2466235.820, [-0.320707, 0.945233, 0.060677]),
        (2467151.190, [0.234842, -0.967704, -0.091637]),
        (2468066.560, [-0.334127, 0.940636, 0.059698]),
        (2468981.930, [0.498391, -0.866511, 0.027652]),
    ];

    fn off_by_deg(jd: f64, want: [f64; 3]) -> f64 {
        let got = luna_rel(jd).normalize();
        got.dot(DVec3::from(want).normalize())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    #[test]
    fn the_lunar_model_is_near_the_kernel_at_the_measured_epochs() {
        for (jd, want) in DE440S_EPOCHS {
            let off = off_by_deg(jd, want);
            assert!(off < 1.5, "JD {jd}: {off:.2} deg from DE440s");
        }
    }

    #[test]
    fn the_lunar_model_is_about_a_degree_off_across_a_century() {
        let off: Vec<f64> = DE440S_SPREAD
            .iter()
            .map(|(jd, want)| off_by_deg(*jd, *want))
            .collect();
        let mean = off.iter().sum::<f64>() / off.len() as f64;
        let worst = off.iter().cloned().fold(0.0, f64::max);
        assert!(mean < 1.2, "mean {mean:.2} deg from DE440s");
        assert!(worst < 3.2, "worst {worst:.2} deg from DE440s");
    }

    /// The perigee's direction, found from the model itself: where the distance is least
    /// in one anomalistic month. It must come round in 8.85 years, not the 6.0 years the
    /// double-counted node drift gave.
    #[test]
    fn the_perigee_circles_in_8_85_years() {
        let perigee_longitude = |jd0: f64| {
            let closest = (0..2800)
                .map(|k| jd0 + k as f64 * 0.01)
                .min_by(|a, b| luna_rel(*a).length().total_cmp(&luna_rel(*b).length()))
                .unwrap();
            let p = luna_rel(closest);
            p.y.atan2(p.x).to_degrees()
        };
        // over one year the perigee advances 360/8.85 = 40.7°; 6 years would give 60°
        let moved =
            (perigee_longitude(J2000 + 365.25) - perigee_longitude(J2000)).rem_euclid(360.0);
        assert!((moved - 40.7).abs() < 3.0, "{moved}");
    }

    /// Where kernels are present, the frozen numbers must still be what they say.
    #[test]
    fn the_frozen_directions_are_the_kernels() {
        let Some(s) = crate::spice::spice() else {
            return;
        };
        for (jd, want) in DE440S_EPOCHS.iter().chain(DE440S_SPREAD.iter()) {
            let k = s
                .rel_ecl_m(301, 399, *jd)
                .expect("de440s covers it")
                .normalize();
            let got = k
                .dot(DVec3::from(*want).normalize())
                .clamp(-1.0, 1.0)
                .acos();
            // the dates are rounded to 0.001 day, about 0.007° of the Moon's motion
            assert!(got.to_degrees() < 0.02, "JD {jd}: {}", got.to_degrees());
        }
    }

    /// `pantheon-web/js/eph.js` carries a copy of this model for the 3D window.
    #[test]
    fn the_browser_copy_of_the_lunar_model_is_the_same() {
        let js = include_str!("../pantheon-web/js/eph.js");
        for line in [
            "const node = (125.08 - 0.0529538 * d) * DEG;",
            "const argp = (318.15 + 0.1643573 * d) * DEG;",
            "const ma = (135.27 + 13.064993 * d) * DEG;",
        ] {
            assert!(js.contains(line), "eph.js has drifted: {line}");
        }
    }
}

#[cfg(test)]
mod far_date_tests {
    use super::*;

    /// Julian dates far beyond anything the elements were fitted to, both ways. The first
    /// failure this guards against was Mars's eccentricity passing 1 near the year 1.15 million.
    fn far_dates() -> Vec<f64> {
        let mut out = vec![J2000];
        for years in [
            1e3, 1e4, 1e5, 1e6, 1.15e6, 1.2e6, 1e7, 1e8, 1e9, 1e10, 2.2e12,
        ] {
            out.push(J2000 + years * 365.25);
            out.push(J2000 - years * 365.25);
        }
        out
    }

    #[test]
    fn orbits_stay_real_ellipses_at_any_date() {
        for jd in far_dates() {
            for p in &PLANETS {
                let s = orbit_shape(p, jd);
                assert!(s.a > 0.0 && (0.0..1.0).contains(&s.e), "{} at {jd}", p.name);
                assert!(s.inc.is_finite() && s.lp.is_finite() && s.node.is_finite());
            }
        }
    }

    #[test]
    fn every_position_is_finite_at_any_date() {
        for jd in far_dates() {
            for i in 0..PLANETS.len() {
                assert!(planet_pos(i, jd).is_finite(), "{} at {jd}", PLANETS[i].name);
                assert!(body_pos(i, jd).is_finite(), "{} at {jd}", PLANETS[i].name);
                assert!(planet_vel(i, jd).is_finite(), "{} at {jd}", PLANETS[i].name);
                assert!(planet_orbit_point(i, jd, 1.0).is_finite());
                assert!(planet_eccentric_anomaly(i, jd).is_finite());
                assert!(planet_orbit(i, jd, 8).iter().all(|p| p.is_finite()));
            }
            for mi in 0..MOONS.len() {
                assert!(moon_rel(mi, jd).is_finite(), "{} at {jd}", MOONS[mi].name);
            }
        }
    }

    /// Inside the span the table is fitted to, holding the elements changes nothing.
    #[test]
    fn the_limits_do_not_touch_the_valid_span() {
        for year in [1800.0, 1900.0, 2000.0, 2026.0, 2050.0] {
            let jd = J2000 + (year - 2000.0) * 365.25;
            let t: f64 = (jd - J2000) / 36_525.0;
            for p in &PLANETS {
                let s = orbit_shape(p, jd);
                assert!(
                    (s.e - (p.el[1] + p.rate[1] * t)).abs() < 1e-12,
                    "{}",
                    p.name
                );
                assert!(
                    (s.a - (p.el[0] + p.rate[0] * t) * AU).abs() < 1.0,
                    "{}",
                    p.name
                );
            }
        }
    }

    /// Past de440s, every planet is on elements, as the labels say. Without kernels this
    /// holds trivially.
    #[test]
    fn planets_past_de440s_are_on_elements_as_labelled() {
        let jd = J2000 + 226.0 * 365.25;
        assert!(!eph_is_spice(jd));
        assert_eq!(body_pos(2, jd), earth_center(jd));
        for i in [0, 1, 3, 4, 5, 6, 7, 8] {
            assert_eq!(body_pos(i, jd), planet_pos(i, jd), "{}", PLANETS[i].name);
        }
    }

    #[test]
    fn a_held_planet_keeps_orbiting() {
        let jd = J2000 + 1e6 * 365.25;
        let (a, b) = (planet_pos(3, jd), planet_pos(3, jd + 300.0));
        assert!((a - b).length() > 1e10, "Mars should have moved");
    }

    #[test]
    fn dates_format_at_any_date() {
        for jd in far_dates() {
            assert!(!fmt_jd(jd).is_empty());
        }
    }

    /// The browser copy of the elements must hold at the same limits.
    #[test]
    fn the_browser_copy_holds_the_elements_too() {
        let js = include_str!("../pantheon-web/js/eph.js");
        let (lo, hi) = ELEMENTS_VALID_CENTURIES;
        assert!(
            js.contains(&format!(
                "Math.min(Math.max((jd - J2000_JD) / 36525.0, {lo:.1}), {hi:.1})"
            )),
            "eph.js has drifted from ELEMENTS_VALID_CENTURIES"
        );
    }
}

#[cfg(test)]
mod moon_sense_tests {
    use super::*;

    /// Orbit normals (r × v, ecliptic J2000) from JPL Horizons for 2026-11-04: planetocentric
    /// state vectors, read from the API and frozen so this runs with no data.
    const HORIZONS_NORMALS: [(&str, [f64; 3]); 21] = [
        ("Moon", [-0.0485, -0.0733, 0.9961]),
        ("Phobos", [0.4294, -0.0527, 0.9016]),
        ("Deimos", [0.4042, -0.0598, 0.9127]),
        ("Io", [-0.0143, -0.0362, 0.9992]),
        ("Europa", [-0.0201, -0.0300, 0.9993]),
        ("Ganymede", [-0.0146, -0.0382, 0.9992]),
        ("Callisto", [-0.0134, -0.0313, 0.9994]),
        ("Mimas", [0.0659, 0.4801, 0.8747]),
        ("Enceladus", [0.0854, 0.4623, 0.8826]),
        ("Tethys", [0.0675, 0.4685, 0.8809]),
        ("Dione", [0.0852, 0.4621, 0.8827]),
        ("Rhea", [0.0826, 0.4665, 0.8807]),
        ("Titan", [0.0881, 0.4565, 0.8854]),
        ("Iapetus", [0.1920, 0.2199, 0.9564]),
        ("Miranda", [0.2866, 0.9482, -0.1374]),
        ("Ariel", [0.2117, 0.9681, -0.1343]),
        ("Umbriel", [0.2107, 0.9683, -0.1342]),
        ("Titania", [0.2120, 0.9679, -0.1351]),
        ("Oberon", [0.2109, 0.9678, -0.1375]),
        ("Triton", [-0.5279, 0.5684, -0.6310]),
        ("Charon", [-0.6781, 0.6237, -0.3889]),
    ];

    fn model_normal(mi: usize, jd: f64) -> DVec3 {
        let h = 0.01;
        let r = moon_rel_elements(mi, jd);
        let v = moon_rel_elements(mi, jd + h) - moon_rel_elements(mi, jd - h);
        r.cross(v).normalize()
    }

    /// Every moon goes round the way JPL says, on its mean elements, which is all the
    /// moons of Uranus ever have and every moon has on a fresh clone.
    #[test]
    fn every_moon_orbits_the_way_horizons_says() {
        let jd = cal_to_jd(2026, 11, 4.0);
        for (mi, (name, want)) in HORIZONS_NORMALS.iter().enumerate() {
            assert_eq!(MOONS[mi].name, *name, "the table's order");
            let dot = model_normal(mi, jd).dot(DVec3::from(*want));
            println!("{name:10} model normal . Horizons normal = {dot:+.3}");
            assert!(dot > 0.0, "{name} orbits backwards: {dot:+.3}");
        }
    }

    /// The moons of Uranus also lie in the right plane, which is what makes their sense
    /// more than a coin toss.
    #[test]
    fn the_moons_of_uranus_lie_in_their_real_plane() {
        let jd = cal_to_jd(2026, 11, 4.0);
        for (mi, (name, want)) in HORIZONS_NORMALS.iter().enumerate() {
            if MOONS[mi].planet == URANUS {
                let dot = model_normal(mi, jd).dot(DVec3::from(*want));
                assert!(dot > 0.99, "{name}: {dot:+.3}");
            }
        }
    }

    #[test]
    fn the_browser_copy_uses_the_same_tilt() {
        let js = include_str!("../pantheon-web/js/eph.js");
        assert!(js.contains("m.incl = m.parent === URANUS ? 180.0 - MOON_INCL[i] : MOON_INCL[i];"));
        assert!(js.contains("const URANUS = 6;"));
    }

    #[test]
    fn cards_still_show_the_tilt_from_the_equator() {
        for (mi, m) in MOONS.iter().enumerate() {
            if m.planet == URANUS {
                assert!(
                    MOON_INCL[mi] < 90.0,
                    "{} must not read as retrograde",
                    m.name
                );
            }
        }
    }
}

#[cfg(test)]
mod energy_tests {
    use super::*;

    /// A system set up as the app sets one up: Sun, planet, then its moons.
    fn system(pi: usize, jd: f64) -> (NBody, f64) {
        let moons: Vec<usize> = (0..MOONS.len())
            .filter(|m| MOONS[*m].planet == pi)
            .collect();
        let dt = 60.0 / DAY;
        let at = |t: f64| -> Vec<DVec3> {
            let planet = planet_pos(pi, t);
            let mut out = vec![DVec3::ZERO, planet];
            out.extend(moons.iter().map(|m| planet + moon_rel_elements(*m, t)));
            out
        };
        let (before, after, pos) = (at(jd - dt), at(jd + dt), at(jd));
        let vel = before
            .iter()
            .zip(&after)
            .map(|(a, b)| (*b - *a) / (2.0 * 60.0))
            .collect();
        let mut gm = vec![GM_SUN, PLANETS[pi].gm];
        gm.extend(moons.iter().map(|m| MOON_GM[*m]));
        let mut s = NBody {
            gm,
            pos,
            vel,
            label: String::new(),
        };
        s.zero_momentum();
        let shortest = moons
            .iter()
            .map(|m| MOONS[*m].period_d.abs())
            .fold(f64::MAX, f64::min);
        // the app's substep
        (s, shortest * DAY / 600.0)
    }

    /// The largest value of the diagnostic while running `spans` shortest periods.
    fn drift(pi: usize, step_factor: f64, spans: f64) -> f64 {
        let jd = cal_to_jd(2026, 11, 4.5);
        let (mut s, substep) = system(pi, jd);
        let e0 = s.moon_orbit_energies(1);
        let h = substep * step_factor;
        let steps = (spans * 600.0 / step_factor) as usize;
        let mut worst = 0.0f64;
        for _ in 0..steps {
            s.step(h);
            worst = worst.max(s.moon_energy_drift(1, &e0).unwrap());
        }
        worst
    }

    #[test]
    fn the_energy_drift_sees_a_step_that_is_too_large() {
        for pi in [2usize, 3, 4, 5, 6, 7, 8] {
            let normal = drift(pi, 1.0, 20.0);
            let coarse = drift(pi, 100.0, 20.0);
            let name = PLANETS[pi].name;
            println!(
                "{name:8} normal step {:9.3}%, 100x step {:12.3}%",
                normal * 100.0,
                coarse * 100.0
            );
            assert!(normal < 0.03, "{name}: {normal} at the normal step");
            assert!(coarse > 0.05, "{name}: {coarse} at a 100x step");
            assert!(coarse > 5.0 * normal, "{name}: {coarse} vs {normal}");
        }
    }

    #[test]
    fn the_energy_drift_sees_one_moon_thrown_out() {
        for pi in [2usize, 3, 4, 5] {
            let (mut s, _) = system(pi, cal_to_jd(2026, 11, 4.5));
            let e0 = s.moon_orbit_energies(1);
            let (planet_v, k) = (s.vel[1], 2);
            s.vel[k] = planet_v + (s.vel[k] - planet_v) * 2.0;
            let change = s.moon_energy_drift(1, &e0).unwrap();
            println!(
                "{:8} doubling one moon's speed reads {change:.3}",
                PLANETS[pi].name
            );
            assert!(change > 1.0, "{}: only {change}", PLANETS[pi].name);
        }
    }

    #[test]
    fn a_system_with_no_moons_has_no_drift() {
        let (s, _) = system(0, cal_to_jd(2026, 11, 4.5));
        assert!(s.moon_energy_drift(1, &s.moon_orbit_energies(1)).is_none());
    }
}
