//! Hierarchical coordinate frames, high-precision orthographic atlas camera,
//! and the view-basis ladder (ecliptic → galactic → supergalactic) that
//! reorients the projection plane as the zoom crosses scale bands.
//!
//! All positions are f64 meters in heliocentric ecliptic J2000 ("root frame").
//! Semantic child frames (planets, moons, exosystems) resolve into root before
//! projection; f64 keeps every band sub-pixel exact.

use glam::{DMat3, DQuat, DVec3};

pub const AU: f64 = 1.495_978_707e11;
pub const PC: f64 = 3.085_677_581_491_367e16;
pub const LY: f64 = 9.460_730_472_580_8e15;
pub const KPC: f64 = 1e3 * PC;
pub const MPC: f64 = 1e6 * PC;
pub const DAY: f64 = 86_400.0;

fn unit_radec(ra_deg: f64, dec_deg: f64) -> DVec3 {
    let (ra, dec) = (ra_deg.to_radians(), dec_deg.to_radians());
    DVec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin())
}

/// Rotation taking ICRS/equatorial vectors to ecliptic J2000: Rx(−ε), so the
/// north celestial pole maps to (0, +sin ε, +cos ε). (AUDIT FND-001: an extra
/// `.transpose()` here previously mirrored every catalog latitude.)
pub fn ecl_from_icrs() -> DMat3 {
    let eps = 23.439_291_1_f64.to_radians();
    DMat3::from_cols(
        DVec3::new(1.0, 0.0, 0.0),
        DVec3::new(0.0, eps.cos(), -eps.sin()),
        DVec3::new(0.0, eps.sin(), eps.cos()),
    )
}

/// Convert an ICRS direction + distance into root-frame (ecliptic) meters.
pub fn radec_to_ecl(ra_deg: f64, dec_deg: f64, dist_m: f64) -> DVec3 {
    ecl_from_icrs() * (unit_radec(ra_deg, dec_deg) * dist_m)
}

/// Galactic basis expressed in the root (ecliptic) frame: x toward Sgr A*,
/// z toward the north galactic pole.
pub struct GalacticFrame {
    pub center: DVec3, // Sgr A* position, root frame
    pub x: DVec3,
    pub y: DVec3,
    pub z: DVec3,
}

pub fn galactic_frame() -> GalacticFrame {
    let m = ecl_from_icrs();
    let zg = m * unit_radec(192.859_508, 27.128_336); // north galactic pole
    let xg0 = m * unit_radec(266.405_100, -28.936_175); // galactic center dir
    let yg = zg.cross(xg0).normalize();
    let xg = yg.cross(zg).normalize();
    GalacticFrame { center: xg0 * (8178.0 * PC), x: xg, y: yg, z: zg }
}

/// Supergalactic basis in the root frame (de Vaucouleurs): derived from the
/// galactic frame; SG north pole at galactic l=47.37°, b=+6.32°.
pub fn supergalactic_basis(gal: &GalacticFrame) -> (DVec3, DVec3, DVec3) {
    let gdir = |l_deg: f64, b_deg: f64| -> DVec3 {
        let (l, b) = (l_deg.to_radians(), b_deg.to_radians());
        gal.x * (b.cos() * l.cos()) + gal.y * (b.cos() * l.sin()) + gal.z * b.sin()
    };
    let sgz = gdir(47.37, 6.32);
    let sgx0 = gdir(137.37, 0.0);
    let sgy = sgz.cross(sgx0).normalize();
    let sgx = sgy.cross(sgz).normalize();
    (sgx, sgy, sgz)
}

pub fn smoothstep(lo: f64, hi: f64, x: f64) -> f64 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Layer visibility window on log10(view width in meters), with feathering.
pub fn band(lw: f64, lo: f64, hi: f64, feather: f64) -> f64 {
    smoothstep(lo, lo + feather, lw) * (1.0 - smoothstep(hi - feather, hi, lw))
}

#[derive(Clone)]
pub struct CamAnim {
    pub t: f64,
    pub dur: f64,
    pub c0: DVec3,
    pub c1: DVec3,
    pub lm0: f64,
    pub lm1: f64,
    pub bump: f64, // extra log10-mpp lift at midpoint for long lateral hops
}

#[derive(Clone)]
pub struct Camera {
    pub center: DVec3,
    pub mpp: f64,
    pub tilt: f64,
    pub anim: Option<CamAnim>,
}

pub const MPP_MIN: f64 = 0.02;
pub const MPP_MAX: f64 = 2.0e22; // ~ 1e25 m across a 1600 px window

impl Camera {
    pub fn new() -> Self {
        Self { center: DVec3::ZERO, mpp: 2.0e9, tilt: 0.0, anim: None }
    }

    pub fn log_width(&self, screen_w: f64) -> f64 {
        (self.mpp * screen_w).log10()
    }

    /// Fly to a target; `screen_w` px used to decide the mid-zoom-out bump.
    pub fn fly_to(&mut self, center: DVec3, mpp: f64, screen_w: f64, dur: f64) {
        let lm0 = self.mpp.log10();
        let lm1 = mpp.clamp(MPP_MIN, MPP_MAX).log10();
        let dist = (center - self.center).length();
        let need = (dist / (0.30 * screen_w)).max(1e-30).log10();
        let bump = (need - lm0.max(lm1)).max(0.0);
        self.anim = Some(CamAnim {
            t: 0.0,
            dur: dur.max(0.05),
            c0: self.center,
            c1: center,
            lm0,
            lm1,
            bump,
        });
    }

    /// Advance animation; returns true while animating.
    pub fn tick(&mut self, dt: f64) -> bool {
        let Some(a) = &mut self.anim else { return false };
        a.t += dt;
        let s = (a.t / a.dur).clamp(0.0, 1.0);
        let e = s * s * (3.0 - 2.0 * s);
        let lm = a.lm0 + (a.lm1 - a.lm0) * e + a.bump * 4.0 * e * (1.0 - e);
        self.mpp = 10f64.powf(lm);
        self.center = a.c0 + (a.c1 - a.c0) * e;
        if s >= 1.0 {
            self.center = a.c1;
            self.mpp = 10f64.powf(a.lm1);
            self.anim = None;
            return false;
        }
        true
    }
}

/// A resolved projection for one frame: world (root, m) → screen px.
pub struct Proj {
    pub right: DVec3,
    pub up: DVec3,
    pub out: DVec3,
    pub center: DVec3,
    pub mpp: f64,
    pub w: f64,
    pub h: f64,
}

impl Proj {
    /// Build the projection for the camera at this scale: the basis slerps
    /// ecliptic → galactic (log W 18.3..19.3) → supergalactic (22.3..23.2),
    /// then user tilt rotates about the screen x-axis.
    pub fn new(cam: &Camera, w: f64, h: f64, gal: &GalacticFrame) -> Self {
        let lw = cam.log_width(w);
        let q_ecl = DQuat::IDENTITY;
        let m_gal = DMat3::from_cols(gal.x, gal.y, gal.z); // columns = view rows^T
        let q_gal = DQuat::from_mat3(&m_gal);
        let (sx, sy, sz) = supergalactic_basis(gal);
        let q_sg = DQuat::from_mat3(&DMat3::from_cols(sx, sy, sz));

        let t1 = smoothstep(18.3, 19.3, lw);
        let t2 = smoothstep(22.3, 23.2, lw);
        let mut q = q_ecl.slerp(q_gal, t1);
        q = q.slerp(q_sg, t2);
        let m = DMat3::from_quat(q);
        let (mut right, mut up, mut out) = (m.col(0), m.col(1), m.col(2));
        if cam.tilt.abs() > 1e-6 {
            let rot = DQuat::from_axis_angle(right, -cam.tilt);
            up = rot * up;
            out = rot * out;
        }
        Self { right, up, out, center: cam.center, mpp: cam.mpp, w, h }
    }

    pub fn screen(&self, world: DVec3) -> [f64; 3] {
        let rel = world - self.center;
        let vx = rel.dot(self.right);
        let vy = rel.dot(self.up);
        let vz = rel.dot(self.out);
        [self.w * 0.5 + vx / self.mpp, self.h * 0.5 - vy / self.mpp, vz]
    }

    pub fn px(&self, world: DVec3) -> [f32; 2] {
        let s = self.screen(world);
        [s[0] as f32, s[1] as f32]
    }

    /// World position of a screen point on the camera plane (depth 0).
    pub fn unproject(&self, sx: f64, sy: f64) -> DVec3 {
        self.center
            + self.right * ((sx - self.w * 0.5) * self.mpp)
            + self.up * ((self.h * 0.5 - sy) * self.mpp)
    }

    /// Affine map screen px → 2D coordinates on an arbitrary plane, in `unit`
    /// meters. Returns None when the plane is near edge-on (caller fades).
    pub fn plane_affine(
        &self,
        origin: DVec3,
        ax: DVec3,
        ay: DVec3,
        unit: f64,
    ) -> Option<([f32; 3], [f32; 3], f64)> {
        let n = ax.cross(ay).normalize();
        let dn = self.out.dot(n);
        let incidence = dn.abs();
        if incidence < 0.08 {
            return None;
        }
        let coord = |sx: f64, sy: f64| -> (f64, f64) {
            let base = self.unproject(sx, sy);
            let t = (origin - base).dot(n) / dn;
            let p = base + self.out * t - origin;
            (p.dot(ax) / unit, p.dot(ay) / unit)
        };
        let (u0, v0) = coord(0.0, 0.0);
        let (u1, v1) = coord(1.0, 0.0);
        let (u2, v2) = coord(0.0, 1.0);
        Some((
            [(u1 - u0) as f32, (u2 - u0) as f32, u0 as f32],
            [(v1 - v0) as f32, (v2 - v0) as f32, v0 as f32],
            incidence,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ecl_lonlat(v: DVec3) -> (f64, f64) {
        let lon = v.y.atan2(v.x).to_degrees().rem_euclid(360.0);
        let lat = (v.z / v.length()).asin().to_degrees();
        (lon, lat)
    }

    #[test]
    fn ecl_matrix_is_proper_rotation() {
        let m = ecl_from_icrs();
        let det = m.determinant();
        assert!((det - 1.0).abs() < 1e-12, "det = {det}");
        let mt = m * m.transpose();
        for c in 0..3 {
            for r in 0..3 {
                let want = if c == r { 1.0 } else { 0.0 };
                assert!((mt.col(c)[r] - want).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn north_celestial_pole_golden() {
        // NCP must land at ecliptic latitude 90−ε ≈ 66.56°, longitude 90°.
        let v = ecl_from_icrs() * DVec3::Z;
        let (lon, lat) = ecl_lonlat(v);
        assert!((lat - 66.560_71).abs() < 0.01, "lat = {lat}");
        assert!((lon - 90.0).abs() < 0.01, "lon = {lon}");
    }

    #[test]
    fn galactic_center_direction_golden() {
        // Sgr A* (ICRS 266.405°, −28.936°) lies at ecliptic λ≈266.85°, β≈−5.54°.
        let v = radec_to_ecl(266.405_100, -28.936_175, 1.0);
        let (lon, lat) = ecl_lonlat(v);
        assert!((lon - 266.85).abs() < 0.2, "lon = {lon}");
        assert!((lat + 5.54).abs() < 0.2, "lat = {lat}");
    }

    #[test]
    fn north_galactic_pole_golden() {
        // NGP (ICRS 192.859°, +27.128°) lies at ecliptic β ≈ +29.81°.
        let v = radec_to_ecl(192.859_508, 27.128_336, 1.0);
        let (_, lat) = ecl_lonlat(v);
        assert!((lat - 29.81).abs() < 0.2, "lat = {lat}");
    }

    #[test]
    fn galactic_frame_is_orthonormal_and_consistent() {
        let g = galactic_frame();
        assert!(g.x.dot(g.y).abs() < 1e-10);
        assert!(g.y.dot(g.z).abs() < 1e-10);
        assert!((g.x.cross(g.y) - g.z).length() < 1e-10);
        // center direction agrees with the x axis to well under a degree
        let d = g.center.normalize().dot(g.x);
        assert!(d > 0.9999, "center/x misalignment: cos = {d}");
    }
}

/// Human-readable length with adaptive units.
pub fn fmt_len(m: f64) -> String {
    let a = m.abs();
    if a < 1e4 {
        format!("{m:.0} m")
    } else if a < 0.05 * AU {
        format!("{:.0} km", m / 1e3)
    } else if a < 0.1 * LY {
        let v = m / AU;
        if v < 100.0 {
            format!("{v:.2} AU")
        } else {
            format!("{v:.0} AU")
        }
    } else if a < 0.8 * PC {
        format!("{:.2} ly", m / LY)
    } else if a < 1e3 * PC {
        let v = m / PC;
        if v < 100.0 {
            format!("{v:.1} pc")
        } else {
            format!("{v:.0} pc")
        }
    } else if a < 1e6 * PC {
        format!("{:.1} kpc", m / KPC)
    } else {
        format!("{:.1} Mpc", m / MPC)
    }
}
