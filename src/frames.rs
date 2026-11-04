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
    /// the zoom-and-pan path (van Wijk & Nuij 2003): pull back just enough that
    /// the on-screen motion stays readable, however far the hop
    path: ZoomPan,
    /// user tilt and yaw glide with the flight (shortest arc for yaw)
    pub tilt0: f64,
    pub tilt1: f64,
    pub yaw0: f64,
    pub yaw1: f64,
}

const PATH_RHO: f64 = 1.3;

#[derive(Clone, Copy)]
struct ZoomPan {
    w0: f64,
    r0: f64,
    /// total path length; 0 means start and end coincide
    s_total: f64,
    /// nonzero when there is no lateral move: pure exponential zoom
    pure_k: f64,
    u1: f64,
}

impl ZoomPan {
    fn new(w0: f64, w1: f64, u1: f64) -> Self {
        let rho = PATH_RHO;
        if u1 < 1e-9 * w0.min(w1) {
            let s = (w1 / w0).ln().abs() / rho;
            let k = if w1 >= w0 { 1.0 } else { -1.0 };
            return Self { w0, r0: 0.0, s_total: s, pure_k: k, u1 };
        }
        let r2 = rho * rho;
        // b = (w1^2 - w0^2 +- rho^4 u^2) / (2 w rho^2 u), written in ratios so it
        // stays finite across 20 orders of magnitude
        let b0 = (w1 / w0) * (w1 / u1) / (2.0 * r2) - (w0 / u1) / (2.0 * r2) + r2 * (u1 / w0) / 2.0;
        let b1 = (w1 / u1) / (2.0 * r2) - (w0 / w1) * (w0 / u1) / (2.0 * r2) - r2 * (u1 / w1) / 2.0;
        let r0 = -b0.asinh();
        let r1 = -b1.asinh();
        Self { w0, r0, s_total: ((r1 - r0) / rho).max(0.0), pure_k: 0.0, u1 }
    }

    /// (lateral travel fraction 0..=1, view width) at path position s.
    fn at(&self, s: f64) -> (f64, f64) {
        let rho = PATH_RHO;
        if self.pure_k != 0.0 {
            return (0.0, self.w0 * (self.pure_k * rho * s).exp());
        }
        let c = (rho * s + self.r0).cosh();
        // cosh(r0) tanh(rho s + r0) - sinh(r0), rewritten without cancellation
        let u = self.w0 / (rho * rho) * (rho * s).sinh() / c;
        let w = self.w0 * self.r0.cosh() / c;
        ((u / self.u1).clamp(0.0, 1.0), w)
    }
}

/// FRAMEGRAPH-003: the camera is attached to a frame. `center` is a LOCAL
/// offset from `frame_origin` (root coords of the attached frame's origin).
/// With the default root attachment (`frame_origin == 0`) everything behaves
/// exactly as before; inside an exoplanet pocket the origin rebases to the
/// host, so pan/zoom precision is that of local ~AU-scale numbers instead of
/// 1e17 m absolutes (which quantize at ~80 m).
#[derive(Clone)]
pub struct Camera {
    /// LOCAL center within the attached frame
    pub center: DVec3,
    /// root-frame origin of the attached frame
    pub frame_origin: DVec3,
    pub mpp: f64,
    /// user tilt about the screen x axis, radians (0 = looking straight down)
    pub tilt: f64,
    /// user rotation of the map about its own normal, radians
    pub yaw: f64,
    /// presentation offset added close to a body: three-quarter lit, north up.
    /// Fades to zero as the view widens, so maps stay plan-view.
    pub pres_tilt: f64,
    pub pres_yaw: f64,
    pub anim: Option<CamAnim>,
}

pub const MPP_MIN: f64 = 4.0;
pub const MPP_MAX: f64 = 2.0e22; // ~ 1e25 m across a 1600 px window

impl Camera {
    pub fn new() -> Self {
        Self {
            center: DVec3::ZERO,
            frame_origin: DVec3::ZERO,
            mpp: 2.0e9,
            tilt: 0.0,
            yaw: 0.0,
            pres_tilt: 0.0,
            pres_yaw: 0.0,
            anim: None,
        }
    }

    /// Absolute (root-frame) camera center, for coarse logic — distance
    /// checks, history, serialization. Interaction math stays local.
    pub fn center_root(&self) -> DVec3 {
        self.frame_origin + self.center
    }

    /// Attach to a new frame origin, preserving the absolute viewpoint.
    /// A one-time ~ulp(1e17) rounding is accepted here; all *subsequent*
    /// interaction happens in local coordinates at full precision.
    pub fn rebase(&mut self, new_origin: DVec3) {
        let delta = self.frame_origin - new_origin;
        self.center += delta;
        if let Some(a) = &mut self.anim {
            a.c0 += delta;
            a.c1 += delta;
        }
        self.frame_origin = new_origin;
    }

    pub fn log_width(&self, screen_w: f64) -> f64 {
        (self.mpp * screen_w).log10()
    }

    /// Fly to an absolute (root-frame) target. The animation runs in local
    /// coordinates; `screen_w` px converts metres per pixel to a view width.
    pub fn fly_to(&mut self, center: DVec3, mpp: f64, screen_w: f64, dur: f64) {
        let target_local = center - self.frame_origin;
        let lm0 = self.mpp.log10();
        let lm1 = mpp.clamp(MPP_MIN, MPP_MAX).log10();
        let dist = (target_local - self.center).length();
        let path = ZoomPan::new(10f64.powf(lm0) * screen_w, 10f64.powf(lm1) * screen_w, dist);
        self.anim = Some(CamAnim {
            t: 0.0,
            dur: dur.max(0.05),
            c0: self.center,
            c1: target_local,
            lm0,
            lm1,
            path,
            tilt0: self.tilt,
            tilt1: self.tilt,
            yaw0: self.yaw,
            yaw1: self.yaw,
        });
    }

    /// A pleasant duration for a hop to this view, in seconds.
    pub fn flight_seconds(&self, center: DVec3, mpp: f64, screen_w: f64) -> f64 {
        let dist = (center - self.frame_origin - self.center).length();
        let s = ZoomPan::new(self.mpp * screen_w, mpp.clamp(MPP_MIN, MPP_MAX) * screen_w, dist).s_total;
        (0.75 + 0.1 * s).clamp(0.75, 4.5)
    }

    /// Same flight, also gliding the user tilt and yaw to new values.
    pub fn fly_to_view(&mut self, center: DVec3, mpp: f64, tilt: f64, yaw: f64, screen_w: f64, dur: f64) {
        self.fly_to(center, mpp, screen_w, dur);
        if let Some(a) = &mut self.anim {
            a.tilt1 = tilt;
            a.yaw1 = shortest_arc(a.yaw0, yaw);
        }
    }

    /// Advance animation; returns true while animating.
    pub fn tick(&mut self, dt: f64, screen_w: f64) -> bool {
        let Some(a) = &mut self.anim else { return false };
        a.t += dt;
        let s = (a.t / a.dur).clamp(0.0, 1.0);
        let e = s * s * (3.0 - 2.0 * s);
        let (frac, width) = a.path.at(a.path.s_total * e);
        self.mpp = (width / screen_w).max(1e-6);
        self.center = a.c0 + (a.c1 - a.c0) * frac;
        self.tilt = a.tilt0 + (a.tilt1 - a.tilt0) * e;
        self.yaw = a.yaw0 + (a.yaw1 - a.yaw0) * e;
        if s >= 1.0 {
            self.center = a.c1;
            self.mpp = 10f64.powf(a.lm1);
            self.tilt = a.tilt1;
            self.yaw = a.yaw1;
            self.anim = None;
            return false;
        }
        true
    }
}

/// `to`, shifted by whole turns so it is the nearest equivalent angle to `from`.
pub fn shortest_arc(from: f64, to: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    from + (to - from + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI
}

/// A resolved projection: world (root, m) or frame-local (m) → screen px.
pub struct Proj {
    pub right: DVec3,
    pub up: DVec3,
    pub out: DVec3,
    /// LOCAL camera center within the attached frame
    pub center: DVec3,
    /// root origin of the attached frame
    pub origin: DVec3,
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
        let yaw = cam.yaw + cam.pres_yaw;
        if yaw.abs() > 1e-9 {
            let rot = DQuat::from_axis_angle(out, yaw);
            right = rot * right;
            up = rot * up;
        }
        // positive tilt swings the camera toward the bottom of the screen, so the
        // top of the map recedes. Past 90 degrees the camera is under the plane.
        let tilt = (cam.tilt + cam.pres_tilt).clamp(-2.5, 2.5);
        if tilt.abs() > 1e-6 {
            let rot = DQuat::from_axis_angle(right, tilt);
            up = rot * up;
            out = rot * out;
        }
        Self {
            right,
            up,
            out,
            center: cam.center,
            origin: cam.frame_origin,
            mpp: cam.mpp,
            w,
            h,
        }
    }

    pub fn screen(&self, world: DVec3) -> [f64; 3] {
        // subtract the frame origin first: near content cancels cleanly
        let rel = (world - self.origin) - self.center;
        let vx = rel.dot(self.right);
        let vy = rel.dot(self.up);
        let vz = rel.dot(self.out);
        [self.w * 0.5 + vx / self.mpp, self.h * 0.5 - vy / self.mpp, vz]
    }

    pub fn px(&self, world: DVec3) -> [f32; 2] {
        let s = self.screen(world);
        [s[0] as f32, s[1] as f32]
    }

    /// Frame-local position → screen px at FULL precision (never touches
    /// root-magnitude numbers). Layers living in the camera's frame use this.
    pub fn px_local(&self, local: DVec3) -> [f32; 2] {
        let rel = local - self.center;
        let vx = rel.dot(self.right);
        let vy = rel.dot(self.up);
        [
            (self.w * 0.5 + vx / self.mpp) as f32,
            (self.h * 0.5 - vy / self.mpp) as f32,
        ]
    }

    pub fn screen_local_z(&self, local: DVec3) -> f64 {
        (local - self.center).dot(self.out)
    }

    /// Root-frame world position of a screen point on the camera plane.
    pub fn unproject(&self, sx: f64, sy: f64) -> DVec3 {
        self.origin + self.unproject_local(sx, sy)
    }

    /// Frame-local position of a screen point (full local precision).
    pub fn unproject_local(&self, sx: f64, sy: f64) -> DVec3 {
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
    fn local_frame_projection_survives_cosmic_origins() {
        // FRAMEGRAPH-003: camera attached to a frame ~12 pc from the root.
        // Local-path projection must keep centimeter precision where a
        // root-space subtraction quantizes at ~2^-52 · 1e17 ≈ 30 m.
        let mut cam = Camera::new();
        cam.frame_origin = DVec3::new(3.7e17, -1.2e17, 5.0e16);
        cam.center = DVec3::new(1.0e11, 2.0e10, 0.0);
        cam.mpp = 0.01; // 1 cm per pixel
        let gal = galactic_frame();
        let proj = Proj::new(&cam, 1600.0, 950.0, &gal);
        let p = cam.center + DVec3::new(0.05, 0.0, 0.0); // 5 cm to the right
        let px = proj.px_local(p);
        assert!((px[0] as f64 - 805.0).abs() < 1e-3, "px = {}", px[0]);
        // absolute round trip stays consistent at coarse precision
        let root = proj.unproject(800.0, 475.0);
        assert!((root - cam.center_root()).length() < 100.0);
        // rebase preserves the absolute viewpoint
        let before = cam.center_root();
        cam.rebase(DVec3::ZERO);
        assert!((cam.center_root() - before).length() < 100.0);
        assert!(cam.frame_origin.length() < 1e-9);
    }

    #[test]
    fn yaw_then_tilt_puts_camera_at_requested_ecliptic_direction() {
        let mut cam = Camera::new();
        let lon = 2.1f64;
        let elev = 0.4f64;
        for elev in [elev, -elev] {
            cam.tilt = std::f64::consts::FRAC_PI_2 - elev;
            cam.yaw = lon + std::f64::consts::FRAC_PI_2;
            let proj = Proj::new(&cam, 1600.0, 900.0, &galactic_frame());
            let want = DVec3::new(elev.cos() * lon.cos(), elev.cos() * lon.sin(), elev.sin());
            assert!((proj.out - want).length() < 1e-9, "out = {:?}", proj.out);
            // ecliptic north stays on the upper half of the screen, from either side
            assert!(proj.up.z > 0.0);
        }
    }

    #[test]
    fn flight_lands_exactly_and_stays_finite_across_all_scales() {
        // Earth close-up to the cosmic web and back: 18 orders of magnitude
        let far = DVec3::new(3.0e24, -1.0e24, 5.0e23);
        let mut cam = Camera::new();
        cam.center = DVec3::new(1.5e11, 2.0e9, 0.0);
        cam.mpp = 2.0e4;
        for (target, mpp) in [(far, 5.0e21), (DVec3::new(1.5e11, 2.0e9, 0.0), 2.0e4)] {
            cam.fly_to(target, mpp, 1600.0, 2.0);
            let mut widest = 0.0f64;
            let mut prev = cam.center;
            for _ in 0..200 {
                cam.tick(0.01, 1600.0);
                assert!(cam.mpp.is_finite() && cam.mpp > 0.0, "mpp {}", cam.mpp);
                assert!(cam.center.is_finite());
                widest = widest.max(cam.mpp);
                prev = cam.center;
            }
            let _ = prev;
            assert!(cam.anim.is_none());
            assert_eq!(cam.center, target);
            assert!((cam.mpp - mpp).abs() / mpp < 1e-9);
            // it pulled back at least to the coarser end of the hop
            assert!(widest >= mpp * 0.99);
        }
    }

    #[test]
    fn flight_without_lateral_motion_is_a_pure_zoom() {
        let mut cam = Camera::new();
        cam.center = DVec3::new(1.0e11, 0.0, 0.0);
        cam.mpp = 1.0e5;
        cam.fly_to(cam.center, 1.0e9, 1600.0, 1.0);
        let c = cam.center;
        let mut last = cam.mpp;
        for _ in 0..100 {
            cam.tick(0.0101, 1600.0);
            assert_eq!(cam.center, c);
            assert!(cam.mpp >= last * 0.9999, "zoom must be monotonic");
            last = cam.mpp;
        }
        assert!((cam.mpp - 1.0e9).abs() / 1.0e9 < 1e-9);
    }

    #[test]
    fn shortest_arc_never_turns_the_long_way() {
        let tau = std::f64::consts::TAU;
        assert!((shortest_arc(0.1, tau - 0.1) - (-0.1)).abs() < 1e-12);
        assert!((shortest_arc(3.0, 3.2) - 3.2).abs() < 1e-12);
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
