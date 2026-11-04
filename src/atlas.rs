//! The atlas: composes every scale band into one continuous zoomable scene.
//! Layers own log-scale visibility windows and cross-fade; geometry is
//! projected CPU-side in f64 and emitted as screen-space vector primitives.

use std::borrow::Cow;

use glam::{DMat3, DVec3};

use crate::data::{Prov, Universe};
use crate::engine::draw::{
    CanvasUniforms, DiscInst, DrawList, GalaxyInst, RingInst, RingMap, StrokeStyle, KIND_STAR,
};
use crate::engine::textures::{TexId, TexSet, TexStatus};
use crate::frames::{band, smoothstep, Camera, Proj, AU, KPC, MPC, PC};
use crate::sim::{
    self, hill_radius, lagrange_points, moon_rel, planet_axis, Clock, NBody, GM_SUN, J2000, MOONS,
    PLANETS, RINGS, SUN_RADIUS_KM,
};
use crate::theme;

pub const R_SUN_M: f64 = SUN_RADIUS_KM * 1e3;
pub const R_EARTH_M: f64 = 6.371e6;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ObjectRef {
    Sun,
    Planet(usize),
    Moon(usize),
    Craft(usize),
    Star(usize),
    ExoHost(usize),
    ExoPlanet(usize, usize),
    Galaxy(usize),
    Node(usize),
}

#[derive(Clone)]
pub struct Modes {
    pub living: bool,
    pub trails: bool,
    pub gravity: bool,
    pub truth: bool,
    pub system_view: bool,
    pub ladder: bool,
    pub labels: bool,
    pub orbits: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            living: false,
            trails: true,
            gravity: false,
            truth: false,
            system_view: false,
            ladder: true,
            labels: true,
            orbits: true,
        }
    }
}

/// An awakened local simulation island (focused system, N-body).
pub struct Island {
    pub sim: NBody,
    pub ids: Vec<ObjectRef>,
    pub anchor: ObjectRef,
    pub trails: Vec<Vec<DVec3>>,
    pub jd: f64,
    /// initial scaled energy, for the drift diagnostic
    pub e0: f64,
}

pub struct Label {
    pub pos: [f32; 2],
    pub text: String,
    pub size: f32,
    pub color: [f32; 4],
    pub priority: i32,
    /// clicking this label selects the object it names
    pub obj: Option<ObjectRef>,
}

pub struct Pickable {
    pub pos: [f32; 2],
    pub r: f32,
    pub obj: ObjectRef,
}

pub struct FrameOut {
    pub uni: CanvasUniforms,
    pub labels: Vec<Label>,
    pub picks: Vec<Pickable>,
    pub band_name: String,
    /// what this view is built from, in one honest sentence
    pub caption: (Cow<'static, str>, Prov),
    /// the biggest body on screen and how its surface was made
    largest: Option<(f32, SurfaceSource)>,
    /// Evidence actually drawn this frame (layer-granular) —
    /// the truth legend is generated from this, never from a static list
    pub evidence: Vec<(crate::evidence::Ev, &'static str)>,
}

/// Record that a layer drew content backed by this evidence.
fn seen(out: &mut FrameOut, e: crate::evidence::Ev, what: &'static str) {
    if !out.evidence.iter().any(|(a, b)| *a == e && *b == what) {
        out.evidence.push((e, what));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SurfaceSource {
    Sun,
    Map,
    Procedural(PositionSource),
}

/// What a body's position on screen really rests on.
#[derive(Clone, Copy, PartialEq)]
enum PositionSource {
    Kernel,
    PlanetElements,
    /// our Moon's precessing ellipse
    LunarModel,
    MoonElements,
}

fn note_surface(out: &mut FrameOut, r_px: f32, src: SurfaceSource) {
    if out.largest.map(|(r, _)| r_px > r).unwrap_or(true) {
        out.largest = Some((r_px, src));
    }
}

pub struct BuildIn<'a> {
    pub u: &'a Universe,
    pub cam: &'a Camera,
    pub clock: &'a Clock,
    pub modes: &'a Modes,
    pub focus: Option<ObjectRef>,
    /// Active exoplanet pocket scene — independent of selection (closing a card
    /// must not destroy the world around the camera).
    pub scene_exo: Option<usize>,
    pub island: Option<&'a Island>,
    pub w: f64,
    pub h: f64,
    pub anim_t: f64,
    pub ppp: f32,
    pub tex: &'a TexStatus,
}

// ------------------------------------------------------------ object model

/// Deterministic local basis for an exoplanet system: the ecliptic plane at
/// the host, so systems read face-on in the top-down atlas projection. True
/// orbital orientation is unknown for most systems; this convention is marked
/// illustrative in cards and will be replaced when node/inclination data or a
/// real frame graph lands.
pub fn exo_plane(_u: &Universe, _ei: usize) -> (DVec3, DVec3) {
    (DVec3::X, DVec3::Y)
}

/// World-space offset of an exoplanet from its host (planets
/// now have real positions — reticle, follow, dive, and paths all agree).
/// Kepler-solved on the same conic the orbit engraving uses; phase is
/// illustrative (no epoch of periastron in the default columns).
pub fn exo_planet_rel(u: &Universe, ei: usize, k: usize, jd: f64) -> Option<DVec3> {
    let s = &u.exos[ei];
    let p = s.planets.get(k)?;
    let (a_au, _) = s.a_au_of(p)?;
    let ecc = p.e.unwrap_or(0.0).clamp(0.0, 0.95);
    let ma = p
        .per_d
        .map(|per| (jd - J2000) / per * std::f64::consts::TAU)
        .unwrap_or(k as f64 * 1.7);
    let mut ea = ma % std::f64::consts::TAU;
    for _ in 0..10 {
        ea -= (ea - ecc * ea.sin() - ma % std::f64::consts::TAU) / (1.0 - ecc * ea.cos());
    }
    let a = a_au * AU;
    let (x, y) = (
        a * (ea.cos() - ecc),
        a * (1.0 - ecc * ecc).sqrt() * ea.sin(),
    );
    let (e1, e2) = exo_plane(u, ei);
    Some(e1 * x + e2 * y)
}

/// Position resolution now routes through the frame graph:
/// hierarchical composition instead of special-case formulas, with a
/// parity test in entity.rs guarding equivalence. Simulation islands
/// still override their members.
pub fn obj_pos(u: &Universe, obj: ObjectRef, jd: f64, island: Option<&Island>) -> DVec3 {
    if let Some(isl) = island {
        if let Some(k) = isl.ids.iter().position(|o| *o == obj) {
            return isl.sim.pos[k];
        }
    }
    use crate::entity::Provider;
    let g = u.graph();
    match obj {
        ObjectRef::Sun => DVec3::ZERO,
        ObjectRef::Planet(i) => g.origin_in_root(u, g.planet(i), jd),
        ObjectRef::Moon(mi) => g.origin_in_root(u, g.moon(mi), jd),
        ObjectRef::Craft(i) => g.origin_in_root(u, g.craft_frame(i), jd),
        ObjectRef::Star(i) => g.leaf_in_root(u, Provider::Star(i), jd),
        ObjectRef::ExoHost(i) => g.leaf_in_root(u, Provider::ExoHost(i), jd),
        ObjectRef::ExoPlanet(i, k) => g.leaf_in_root(u, Provider::ExoPlanetOrbit(i, k), jd),
        ObjectRef::Galaxy(i) => u.galaxies[i].pos,
        ObjectRef::Node(i) => u.nodes[i].pos,
    }
}

pub fn obj_name(u: &Universe, obj: ObjectRef) -> String {
    match obj {
        ObjectRef::Sun => "Sun".into(),
        ObjectRef::Planet(i) => PLANETS[i].name.into(),
        ObjectRef::Moon(mi) => MOONS[mi].name.into(),
        ObjectRef::Craft(i) => u.craft[i].name.into(),
        ObjectRef::Star(i) => u.stars[i]
            .name
            .map(String::from)
            .unwrap_or_else(|| format!("Gaia star · {:.1} pc", u.stars[i].dist_pc)),
        ObjectRef::ExoHost(i) => u.exos[i].host.clone(),
        ObjectRef::ExoPlanet(i, k) => u.exos[i].planets[k].name.clone(),
        ObjectRef::Galaxy(i) => u.galaxies[i].name.into(),
        ObjectRef::Node(i) => u.nodes[i].name.into(),
    }
}

/// Outer radius of a planet's rings (m), if it has any.
pub fn ring_outer_m(pi: usize) -> Option<f64> {
    RINGS.iter().find(|r| r.planet == pi).and_then(|r| {
        r.zones
            .iter()
            .map(|z| z.1 * 1e3)
            .fold(None, |a: Option<f64>, b| {
                Some(a.map_or(b, |x: f64| x.max(b)))
            })
    })
}

/// Physical radius of a solid body (m), rings included where it has them.
pub fn body_frame_radius(obj: ObjectRef) -> Option<f64> {
    match obj {
        ObjectRef::Sun => Some(R_SUN_M),
        ObjectRef::Planet(i) => {
            let body = PLANETS[i].radius_km * 1e3;
            // a faint thin ring system shouldn't push the planet away
            Some(if PLANETS[i].name == "Saturn" {
                ring_outer_m(i).unwrap_or(body)
            } else {
                body
            })
        }
        ObjectRef::Moon(mi) => Some(MOONS[mi].radius_km * 1e3),
        _ => None,
    }
}

/// Suggested view half-extent (m) when flying to an object: bodies are framed
/// large enough to see, not lost among their moons.
pub fn obj_half_extent(u: &Universe, obj: ObjectRef) -> f64 {
    match obj {
        ObjectRef::Sun | ObjectRef::Planet(_) | ObjectRef::Moon(_) => {
            body_frame_radius(obj).unwrap_or(AU)
        }
        ObjectRef::Craft(_) => 6.0 * AU,
        ObjectRef::Star(_) => 4.0 * AU,
        ObjectRef::ExoHost(i) => {
            let s = &u.exos[i];
            let mut ext = 0.0f64;
            for p in &s.planets {
                if let Some((a, _)) = s.a_au_of(p) {
                    ext = ext.max(a);
                }
            }
            if ext <= 0.0 {
                ext = 1.0;
            }
            ext * 1.6 * AU
        }
        ObjectRef::ExoPlanet(i, k) => {
            let s = &u.exos[i];
            s.planets[k]
                .rade
                .map(|r| r * R_EARTH_M * 2.0)
                .unwrap_or(0.05 * AU)
        }
        ObjectRef::Galaxy(i) => u.galaxies[i].size_kpc * KPC * 0.75,
        ObjectRef::Node(i) => u.nodes[i].size_mpc * MPC * 2.2,
    }
}

/// View half-extent that takes in a planet's moon system, if it has one.
pub fn moon_system_extent(pi: usize) -> Option<f64> {
    let outer = MOONS
        .iter()
        .filter(|m| m.planet == pi)
        .map(|m| m.a_km * 1e3)
        .fold(0.0f64, f64::max);
    (outer > 0.0).then_some(outer * 1.15)
}

/// A pleasant viewpoint for a sunlit body: the sun about 35 degrees to one side
/// so the terminator shows, north up, seen from above the orbital plane. Bodies
/// with rings get enough elevation for the rings to open. Returns (tilt, yaw).
pub fn presentation_angles(
    u: &Universe,
    obj: ObjectRef,
    jd: f64,
    isl: Option<&Island>,
) -> (f64, f64) {
    use std::f64::consts::FRAC_PI_2;
    let pos = obj_pos(u, obj, jd, isl);
    let to_sun = (-pos).normalize_or_zero();
    let mut toward = to_sun;
    if let ObjectRef::Moon(mi) = obj {
        // moons: lean toward the parent, so the familiar face is the one on show
        let parent = obj_pos(u, ObjectRef::Planet(MOONS[mi].planet), jd, isl);
        let to_parent = (parent - pos).normalize_or_zero();
        toward = (to_sun + to_parent * 0.7).normalize_or_zero();
    }
    if matches!(obj, ObjectRef::Sun) {
        // the Sun has no phase; a low view keeps its poles off-centre
        return (1.3, 0.0);
    }
    let lon = toward.y.atan2(toward.x) - 0.62;
    let mut elev = 22f64.to_radians();
    if let ObjectRef::Planet(i) = obj {
        if ring_outer_m(i).is_some() {
            // rings: open them, and stay on their sunlit face where there is one
            let pole = planet_axis(i, jd);
            let sun_side = to_sun.dot(pole);
            for try_deg in [
                22.0f64, 30.0, 38.0, 46.0, 54.0, -22.0, -30.0, -38.0, -46.0, -54.0,
            ] {
                elev = try_deg.to_radians();
                let c = DVec3::new(elev.cos() * lon.cos(), elev.cos() * lon.sin(), elev.sin());
                let open = c.dot(pole);
                if open.abs() > 0.28 && (sun_side.abs() < 0.015 || open * sun_side > 0.0) {
                    break;
                }
            }
        }
    }
    (FRAC_PI_2 - elev, lon + FRAC_PI_2)
}

// -------------------------------------------------------------- helpers

fn ink(base: [f32; 3], a: f32, truth: bool, prov: Prov) -> [f32; 4] {
    if truth {
        let c = prov.rgb();
        [c[0], c[1], c[2], a]
    } else {
        [base[0], base[1], base[2], a]
    }
}

fn project_poly(proj: &Proj, pts: &[DVec3]) -> Vec<[f32; 2]> {
    pts.iter().map(|p| proj.px(*p)).collect()
}

fn on_screen(p: [f32; 2], w: f64, h: f64, m: f32) -> bool {
    p[0] > -m && p[1] > -m && p[0] < w as f32 + m && p[1] < h as f32 + m
}

fn view_dir(proj: &Proj, world_dir: DVec3) -> [f32; 3] {
    [
        world_dir.dot(proj.right) as f32,
        world_dir.dot(proj.up) as f32,
        world_dir.dot(proj.out) as f32,
    ]
}

/// View-z → NDC depth, bounded for any scale: near (+z toward viewer) maps
/// small. atan compresses the unbounded range into (0, 1).
fn zmap(z_view: f64, view_w: f64) -> f32 {
    (0.5 - (z_view / view_w.max(1e-6)).atan() / std::f64::consts::PI) as f32
}

/// A body's frame as view-space axes.
fn view_axes(proj: &Proj, m: DMat3) -> ([f32; 3], [f32; 3], [f32; 3]) {
    (
        view_dir(proj, m.col(0)),
        view_dir(proj, m.col(1)),
        view_dir(proj, m.col(2)),
    )
}

/// Any right-handed frame around a pole (for bodies with no known prime meridian).
fn frame_about(pole: DVec3) -> DMat3 {
    let z = pole.normalize_or_zero();
    let seed = if z.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    let x = seed.cross(z).normalize_or_zero();
    DMat3::from_cols(x, z.cross(x), z)
}

/// IAU solar pole and rotation (Archinal et al. 2018): alpha 286.13, delta 63.87,
/// W = 84.176 + 14.1844 d.
fn sun_basis(jd: f64) -> DMat3 {
    let (alpha, delta) = (286.13f64.to_radians(), 63.87f64.to_radians());
    let m = crate::frames::ecl_from_icrs();
    let pole = m * DVec3::new(
        delta.cos() * alpha.cos(),
        delta.cos() * alpha.sin(),
        delta.sin(),
    );
    let q = (m * DVec3::Z).cross(pole).normalize_or_zero();
    let y = pole.cross(q);
    let w = (84.176 + 14.184_4 * (jd - J2000)).to_radians();
    let x = q * w.cos() + y * w.sin();
    DMat3::from_cols(x, pole.cross(x), pole)
}

// ------------------------------------------------- adaptive curve tessellation

/// Tessellate a parametric curve to sub-pixel chord error wherever it is on
/// screen, in f64 so a curve whose radius is millions of pixels neither facets
/// nor jitters, and clip it to the viewport before anything becomes f32.
struct Curve<'a> {
    proj: &'a Proj,
    f: &'a dyn Fn(f64) -> DVec3,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    runs: Vec<Vec<(f64, f64, f64)>>,
    /// subdivisions left for this curve, so no input can make one frame take forever
    budget: u32,
}

/// Far more subdivisions than any real curve on any screen needs.
const CURVE_BUDGET: u32 = 200_000;

fn finite2(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

impl Curve<'_> {
    fn scr(&self, t: f64) -> [f64; 2] {
        let s = self.proj.screen((self.f)(t));
        [s[0], s[1]]
    }

    fn seg(&mut self, t0: f64, p0: [f64; 2], t1: f64, p1: [f64; 2], depth: u32) {
        // NaN compares false with everything, so without this it subdivides to full depth
        if !finite2(p0) || !finite2(p1) {
            return;
        }
        let tm = 0.5 * (t0 + t1);
        let pm = self.scr(tm);
        if !finite2(pm) {
            return;
        }
        let dx = pm[0] - 0.5 * (p0[0] + p1[0]);
        let dy = pm[1] - 0.5 * (p0[1] + p1[1]);
        let err = (dx * dx + dy * dy).sqrt();
        let pad = err * 2.0;
        let xmin = p0[0].min(p1[0]).min(pm[0]) - pad;
        let xmax = p0[0].max(p1[0]).max(pm[0]) + pad;
        let ymin = p0[1].min(p1[1]).min(pm[1]) - pad;
        let ymax = p0[1].max(p1[1]).max(pm[1]) + pad;
        if xmax < self.x0 || xmin > self.x1 || ymax < self.y0 || ymin > self.y1 {
            return; // fully outside; the next visible piece starts a new run
        }
        if depth >= 16 || err < 0.16 || self.budget == 0 {
            self.emit(t0, p0, t1, p1);
            return;
        }
        self.budget -= 1;
        self.seg(t0, p0, tm, pm, depth + 1);
        self.seg(tm, pm, t1, p1, depth + 1);
    }

    /// Liang-Barsky clip, then append, keeping runs contiguous.
    fn emit(&mut self, t0: f64, p0: [f64; 2], t1: f64, p1: [f64; 2]) {
        let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
        let (mut u0, mut u1) = (0.0f64, 1.0f64);
        for (p, q) in [
            (-dx, p0[0] - self.x0),
            (dx, self.x1 - p0[0]),
            (-dy, p0[1] - self.y0),
            (dy, self.y1 - p0[1]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    u0 = u0.max(r);
                } else {
                    u1 = u1.min(r);
                }
            }
        }
        if u0 > u1 {
            return;
        }
        let a = (p0[0] + dx * u0, p0[1] + dy * u0, t0 + (t1 - t0) * u0);
        let b = (p0[0] + dx * u1, p0[1] + dy * u1, t0 + (t1 - t0) * u1);
        let joined = self
            .runs
            .last()
            .and_then(|r| r.last())
            .map(|l| (l.0 - a.0).abs() < 1e-6 && (l.1 - a.1).abs() < 1e-6)
            .unwrap_or(false);
        if joined {
            self.runs.last_mut().unwrap().push(b);
        } else {
            self.runs.push(vec![a, b]);
        }
    }
}

/// Draw a parametric curve over t0..t1 with a per-parameter alpha.
#[allow(clippy::too_many_arguments)]
fn draw_curve(
    inp: &BuildIn,
    proj: &Proj,
    dl: &mut DrawList,
    st: StrokeStyle,
    f: &dyn Fn(f64) -> DVec3,
    alpha: &dyn Fn(f64) -> f32,
    t0: f64,
    t1: f64,
    seeds: usize,
) {
    let m = 8.0 + (st.width + st.glow * 3.0) as f64;
    let mut c = Curve {
        proj,
        f,
        x0: -m,
        y0: -m,
        x1: inp.w + m,
        y1: inp.h + m,
        runs: Vec::new(),
        budget: CURVE_BUDGET,
    };
    let mut prev_t = t0;
    let mut prev_p = c.scr(t0);
    for k in 1..=seeds.max(1) {
        let t = t0 + (t1 - t0) * k as f64 / seeds.max(1) as f64;
        let p = c.scr(t);
        c.seg(prev_t, prev_p, t, p, 0);
        prev_t = t;
        prev_p = p;
    }
    for run in &c.runs {
        let pts: Vec<[f32; 2]> = run.iter().map(|p| [p.0 as f32, p.1 as f32]).collect();
        let alphas: Vec<f32> = run.iter().map(|p| alpha(p.2)).collect();
        dl.polyline_alpha(&pts, false, st, &alphas);
    }
}

fn seeded(i: usize, k: usize) -> f64 {
    let mut x = (i as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(k as u64 * 0x632B_E5AB);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x & 0xFFFF_FFFF) as f64 / 4_294_967_296.0
}

// --------------------------------------------------------- layer registry

/// Layers declare their scale window and gate instead
/// of living as a hardcoded call sequence. `lw` is log10(view width in m);
/// windows here are coarse skips — each layer still feathers internally.
pub struct LayerDef {
    pub name: &'static str,
    pub lw: (f64, f64),
    pub gate: fn(&BuildIn) -> bool,
    pub run: fn(&BuildIn, &Proj, f64, &mut DrawList, &mut FrameOut),
}

fn gate_always(_: &BuildIn) -> bool {
    true
}
/// solar-system layers yield to an active exoplanet pocket
fn gate_no_exo(i: &BuildIn) -> bool {
    exo_active_system(i).is_none()
}
fn gate_gravity(i: &BuildIn) -> bool {
    i.modes.gravity
}
fn gravity_run(inp: &BuildIn, proj: &Proj, _lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    gravity_layer(inp, proj, dl, out);
}

pub const LAYERS: &[LayerDef] = &[
    LayerDef {
        name: "scale rings",
        lw: (12.4, f64::MAX),
        gate: gate_always,
        run: ruler_layer,
    },
    LayerDef {
        name: "sky",
        lw: (f64::MIN, 17.8),
        gate: gate_always,
        run: sky_layer,
    },
    LayerDef {
        name: "solar system",
        lw: (5.0, 17.5),
        gate: gate_no_exo,
        run: solar_layer,
    },
    LayerDef {
        name: "belts",
        lw: (10.4, 17.4),
        gate: gate_no_exo,
        run: belts_layer,
    },
    LayerDef {
        name: "moon systems",
        lw: (f64::MIN, 11.4),
        gate: gate_no_exo,
        run: moons_layer,
    },
    LayerDef {
        name: "heliosphere",
        lw: (11.8, 17.1),
        gate: gate_no_exo,
        run: heliosphere_layer,
    },
    LayerDef {
        name: "star field",
        lw: (15.8, 20.2),
        gate: gate_always,
        run: stars_layer,
    },
    LayerDef {
        name: "galaxy",
        lw: (18.0, 23.5),
        gate: gate_always,
        run: galaxy_layer,
    },
    LayerDef {
        name: "local group",
        lw: (19.8, 25.0),
        gate: gate_always,
        run: localgroup_layer,
    },
    LayerDef {
        name: "cosmic web",
        lw: (22.8, 26.5),
        gate: gate_always,
        run: cosmic_layer,
    },
    LayerDef {
        name: "gravity view",
        lw: (f64::MIN, f64::MAX),
        gate: gate_gravity,
        run: gravity_run,
    },
];

// ------------------------------------------------------------------ build

pub fn build(inp: &BuildIn, dl: &mut DrawList) -> FrameOut {
    let (u, cam, jd) = (inp.u, inp.cam, inp.clock.jd);
    let truth = inp.modes.truth;
    let proj = Proj::new(cam, inp.w, inp.h, &u.gal);
    let lw = cam.log_width(inp.w);
    dl.screen_w = inp.w as f32;
    dl.screen_h = inp.h as f32;

    let mut out = FrameOut {
        uni: CanvasUniforms::default(),
        labels: Vec::new(),
        picks: Vec::new(),
        band_name: band_name(lw).into(),
        caption: band_caption(lw, inp),
        largest: None,
        evidence: Vec::new(),
    };
    out.uni.size = [inp.w as f32, inp.h as f32, inp.anim_t as f32, inp.ppp];

    // ---- fullscreen procedural planes
    let dim = if truth { 0.35 } else { 1.0 };
    let w_gal = band(lw, 19.0, 23.0, 0.9) * dim;
    let w_dust = band(lw, 17.9, 22.8, 0.8) * dim;
    if w_gal > 0.001 || w_dust > 0.001 {
        if let Some((ra, rb, inc)) = proj.plane_affine(u.gal.center, u.gal.x, u.gal.y, KPC) {
            let fade = smoothstep(0.08, 0.24, inc);
            out.uni.fog_a = [ra[0], ra[1], ra[2], (w_gal * fade) as f32];
            out.uni.fog_b = [rb[0], rb[1], rb[2], (w_dust * fade) as f32];
            let sol = -u.gal.center;
            out.uni.sun_g = [
                (sol.dot(u.gal.x) / KPC) as f32,
                (sol.dot(u.gal.y) / KPC) as f32,
                // where the dense Gaia sample ends, so the dust starts outside it. A sliver
                // above zero keeps the shader's fade valid if the catalog isn't loaded.
                (u.gaia_reach_pc / 1000.0).max(1e-3) as f32,
                0.0,
            ];
        }
    }
    let w_cos = band(lw, 23.0, 26.1, 0.9) * dim.max(0.3);
    let w_deep = band(lw, 20.6, 25.7, 0.9);
    if w_cos > 0.001 {
        let (sx, sy, _sz) = crate::frames::supergalactic_basis(&u.gal);
        if let Some((ra, rb, inc)) = proj.plane_affine(DVec3::ZERO, sx, sy, 10.0 * MPC) {
            let fade = smoothstep(0.08, 0.24, inc);
            out.uni.cos_a = [ra[0], ra[1], ra[2], (w_cos * fade) as f32];
            out.uni.cos_b = [rb[0], rb[1], rb[2], 0.0];
            for (k, n) in u.nodes.iter().take(16).enumerate() {
                out.uni.nodes[k] = [
                    (n.pos.dot(sx) / (10.0 * MPC)) as f32,
                    (n.pos.dot(sy) / (10.0 * MPC)) as f32,
                    ((n.size_mpc * 0.05).max(0.08)) as f32,
                    ((n.size_mpc / 6.0).sqrt().clamp(0.5, 1.2)) as f32,
                ];
            }
        }
    }
    // the background field lies on a plane facing the camera, so it stays
    // readable while the map itself turns between frames
    if w_deep > 0.001 {
        if let Some((ra, rb, _)) = proj.plane_affine(DVec3::ZERO, proj.right, proj.up, 10.0 * MPC) {
            out.uni.deep_a = [ra[0], ra[1], ra[2], w_deep as f32];
            out.uni.deep_b = [rb[0], rb[1], rb[2], 0.0];
        }
    }

    // ---- exosystem pocket (worlds within worlds; scene-gated, not scale-gated)
    if let Some(ei) = exo_active_system(inp) {
        exo_layer(inp, &proj, lw, ei, dl, &mut out);
        out.band_name = u.exos[ei].host.clone();
        out.caption = (
            "Periods and sizes from the NASA Exoplanet Archive. Orbit orientation and phase are illustrative.".into(),
            Prov::Measured,
        );
    }

    // scale names assume you're at home; say so honestly when you've wandered off
    let away = cam.center_root().length();
    if exo_active_system(inp).is_none() && lw < 19.6 && away > 1.2 * cam.mpp * inp.w {
        if away > 1.0e14 {
            out.band_name = "Interstellar space".into();
            out.caption = (
                "Nothing in this atlas lies here at this scale. Follow the arrow back to the Sun."
                    .into(),
                Prov::Procedural,
            );
        } else if lw < 15.0 {
            out.band_name = "Solar System".into();
        }
    }
    if let Some(ObjectRef::Star(_)) = inp.focus {
        if lw < 15.6 {
            out.caption = (
                "Its position is from Gaia DR3. Gaia gives no radius here, so the marker is schematic.".into(),
                Prov::Measured,
            );
        }
    }

    // ---- registered layers: declared scale windows + gates
    for layer in LAYERS {
        if lw >= layer.lw.0 && lw <= layer.lw.1 && (layer.gate)(inp) {
            (layer.run)(inp, &proj, lw, dl, &mut out);
        }
    }

    if let (Some((r_px, src)), true) = (out.largest, lw < 10.4) {
        if r_px > 12.0 && exo_active_system(inp).is_none() {
            let (text, prov) = body_caption(src);
            out.caption = (text.into(), prov);
        }
    }

    // ---- island trails
    if let Some(isl) = inp.island {
        for (k, trail) in isl.trails.iter().enumerate() {
            if trail.len() > 2 {
                let pts = project_poly(&proj, trail);
                let c = ink([0.55, 0.9, 0.75], 0.5, truth, Prov::Modeled);
                dl.polyline(&pts, false, StrokeStyle::new(c, 1.0).glow(1.2));
            }
            let p = proj.px(isl.sim.pos[k]);
            if on_screen(p, inp.w, inp.h, 20.0) {
                dl.point(p, 1.6, 5.0, 0.0, [0.55, 0.95, 0.75, 0.8]);
            }
        }
    }

    // focused Gaia star gets a local schematic scene so a dive never lands in
    // a void
    if let Some(ObjectRef::Star(si)) = inp.focus {
        focused_star_layer(inp, &proj, lw, si, dl, &mut out);
    }

    // back-to-front for correct edge blending (depth buffer handles occlusion;
    // disc_z now stores mapped NDC depth, so far = larger = first)
    if dl.discs.len() == dl.disc_z.len() && dl.discs.len() > 1 {
        let mut idx: Vec<usize> = (0..dl.discs.len()).collect();
        idx.sort_by(|&a, &b| {
            dl.disc_z[b]
                .partial_cmp(&dl.disc_z[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        dl.discs = idx.iter().map(|&i| dl.discs[i]).collect();
        dl.disc_tex = idx.iter().map(|&i| dl.disc_tex[i]).collect();
    }

    // the kernels cover 1850-2150, so inside that the elements' own range doesn't matter
    let jd_ok = (1800.0..=2050.0).contains(&jd_year(jd)) || sim::eph_is_spice(jd);
    if !jd_ok {
        out.labels.push(Label {
            pos: [(inp.w / 2.0) as f32, 90.0],
            text: "This date is outside 1800-2050: planet positions are extrapolated".into(),
            size: 13.0,
            color: theme::rgba(Prov::Inferred.rgb(), 0.9),
            priority: 100,
            obj: None,
        });
    }
    out
}

fn jd_year(jd: f64) -> f64 {
    2000.0 + (jd - J2000) / 365.25
}

pub fn band_name(lw: f64) -> &'static str {
    match lw {
        x if x < 8.3 => "Planet",
        x if x < 10.4 => "Planet and moons",
        x if x < 12.3 => "Inner planets",
        x if x < 13.6 => "Outer planets",
        x if x < 15.0 => "Heliosphere",
        x if x < 16.6 => "Oort cloud",
        x if x < 18.0 => "Nearest stars",
        x if x < 19.6 => "Stellar neighbourhood",
        x if x < 22.3 => "Milky Way",
        x if x < 23.6 => "Local Group",
        _ => "Cosmic web",
    }
}

/// One sentence, always on screen, saying what this view is made of.
fn band_caption(lw: f64, inp: &BuildIn) -> (Cow<'static, str>, Prov) {
    match lw {
        x if x < 13.6 => {
            if sim::eph_is_spice(inp.clock.jd) {
                ("Planet positions from JPL kernels. Asteroid and Kuiper belts are illustrative samples.".into(), Prov::Modeled)
            } else {
                ("Planet positions from approximate orbital elements, not JPL kernels. Asteroid and Kuiper belts are illustrative samples.".into(), Prov::Modeled)
            }
        }
        x if x < 15.0 => {
            if inp.u.craft.is_empty() {
                ("Spacecraft tracks aren't downloaded. The heliosphere's shape is illustrative.".into(), Prov::Illustrative)
            } else {
                ("Spacecraft paths are JPL trajectory solutions. The heliosphere's shape is illustrative.".into(), Prov::Measured)
            }
        }
        x if x < 16.6 => ("No one has seen the Oort cloud. This shell is an illustration of a theoretical structure.".into(), Prov::Illustrative),
        x if x < 18.0 => ("Star positions, colours and brightness from Gaia DR3.".into(), Prov::Measured),
        x if x < 19.6 => {
            let reach = inp.u.gaia_reach_pc;
            let text = if reach > 0.0 {
                format!("Gaia DR3 covers stars out to {reach:.0} pc, then only naked-eye ones. The dust beyond them is illustrative.").into()
            } else {
                "The dust here is illustrative.".into()
            };
            (text, Prov::Procedural)
        }
        x if x < 22.3 => ("An illustration of the Milky Way. Only the Sun's and the centre's positions are measured.".into(), Prov::Procedural),
        x if x < 23.6 => ("Galaxy positions and sizes from a compiled catalog. Their looks and the background are illustrative.".into(), Prov::Illustrative),
        _ => ("Cluster positions from a compiled catalog. The filaments are procedural, not a survey map.".into(), Prov::Procedural),
    }
}

/// Close to a body, the sentence is about that body's surface.
fn body_caption(src: SurfaceSource) -> (&'static str, Prov) {
    use PositionSource::*;
    match src {
        SurfaceSource::Sun => ("The surface is a tuned map. Its motion, prominences and corona are illustrative.", Prov::Illustrative),
        SurfaceSource::Map => ("The surface is a map built from NASA imagery, with tuned colour and some gaps filled. Atmosphere glow is illustrative.", Prov::Inferred),
        SurfaceSource::Procedural(Kernel) => ("The surface pattern is procedural: no map of this body is on this computer. Its position comes from a JPL kernel.", Prov::Procedural),
        SurfaceSource::Procedural(PlanetElements) => ("The surface pattern is procedural: no map of this body is on this computer. Its position comes from approximate orbital elements, not a JPL kernel.", Prov::Procedural),
        SurfaceSource::Procedural(LunarModel) => ("The surface pattern is procedural: no map of this body is on this computer. Its position comes from a simple lunar model, about a degree off, not a JPL kernel.", Prov::Procedural),
        SurfaceSource::Procedural(MoonElements) => ("The surface pattern is procedural: no map of this body is on this computer. No kernel covers it at this date, so its place along its orbit is chosen for legibility and isn't where it really is.", Prov::Procedural),
    }
}

// ------------------------------------------------------------ ruler rings

fn ruler_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let anchor = if lw < 19.0 {
        DVec3::ZERO
    } else if lw < 23.0 {
        inp.u.gal.center
    } else {
        DVec3::ZERO
    };
    let view_w = inp.cam.mpp * inp.w;
    // round numbers in the natural unit of this scale
    let (unit_m, unit_name) = if view_w < 0.6 * crate::frames::LY {
        (AU, "AU")
    } else if view_w < 3.0 * KPC {
        (PC, "pc")
    } else if view_w < 0.5 * MPC {
        (KPC, "kpc")
    } else {
        (MPC, "Mpc")
    };
    let target = view_w * 0.30 / unit_m;
    let k = target.log10().floor();
    let base = 10f64.powf(k);
    let nice = [1.0, 2.0, 5.0]
        .iter()
        .map(|m| m * base)
        .min_by(|a, b| (a - target).abs().partial_cmp(&(b - target).abs()).unwrap())
        .unwrap();
    let c = proj.px(anchor);
    let fade = smoothstep(12.4, 12.9, lw) as f32;
    let r_px = (nice * unit_m / inp.cam.mpp) as f32;
    if (34.0..(inp.w as f32 * 1.25)).contains(&r_px) {
        dl.circle(
            c,
            r_px,
            StrokeStyle::new(theme::rgba(theme::INK_FAINT, 0.13 * fade), 1.0),
        );
        let ang = 0.62f32;
        let txt = if nice >= 1.0 {
            format!("{} {unit_name}", nice.round())
        } else {
            format!("{nice} {unit_name}")
        };
        out.labels.push(Label {
            pos: [c[0] + r_px * ang.cos(), c[1] - r_px * ang.sin()],
            text: txt,
            size: 11.0,
            color: theme::rgba(theme::INK_FAINT, 0.8 * fade),
            priority: -5,
            obj: None,
        });
    }
}

// --------------------------------------------------------------- the real sky

/// Horizontal field of view of the sky backdrop. The atlas camera is orthographic
/// and the sky is at infinity, so it needs a nominal lens of its own.
const SKY_TAN_HALF_FOV: f64 = 0.62;

fn sky_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let w_sky = 1.0 - smoothstep(16.3, 17.7, lw);
    if w_sky <= 0.001 {
        return;
    }
    let gal = &inp.u.gal;
    let in_gal = |v: DVec3| {
        [
            v.dot(gal.x) as f32,
            v.dot(gal.y) as f32,
            v.dot(gal.z) as f32,
        ]
    };
    let k = SKY_TAN_HALF_FOV / (inp.w * 0.5);
    let (r, up, fwd) = (in_gal(proj.right), in_gal(proj.up), in_gal(-proj.out));
    // a photograph of the sky is true only near where it was taken
    let near_sun = inp.cam.center_root().length() < 60.0 * PC;
    if near_sun {
        dl.want(TexId::Sky);
    }
    let has_map = if near_sun {
        inp.tex.fade(TexId::Sky)
    } else {
        0.0
    };
    out.uni.sky_r = [r[0], r[1], r[2], (w_sky as f32) * has_map.max(0.7)];
    out.uni.sky_u = [up[0], up[1], up[2], k as f32];
    out.uni.sky_f = [
        fwd[0],
        fwd[1],
        fwd[2],
        if has_map > 0.0 { 1.0 } else { 0.0 },
    ];
    if has_map > 0.0 {
        seen(out, crate::evidence::Ev::SkyPanorama, "sky panorama");
    } else {
        seen(out, crate::evidence::Ev::SkyProcedural, "sky");
    }
    if !near_sun {
        return;
    }

    // naked-eye stars by true direction: the real constellations, crisp above
    // the panorama. They occupy the sky, so they never move with pan or zoom.
    seen(
        out,
        crate::evidence::Ev::GaiaDr3,
        "bright stars by direction",
    );
    let truth = inp.modes.truth;
    let (cx, cy) = (inp.w * 0.5, inp.h * 0.5);
    for s in &inp.u.stars {
        if s.gmag > 6.3 {
            continue;
        }
        let d = s.pos.normalize_or_zero();
        let z = -d.dot(proj.out);
        if z < 0.3 {
            continue;
        }
        let x = cx + d.dot(proj.right) / z / k;
        let y = cy - d.dot(proj.up) / z / k;
        if x < -10.0 || y < -10.0 || x > inp.w + 10.0 || y > inp.h + 10.0 {
            continue;
        }
        let m = s.gmag;
        let core = (1.5 - 0.17 * m).clamp(0.5, 1.9);
        let bright = (1.9 - 0.3 * m).clamp(0.1, 3.4) * w_sky as f32;
        let rgb = crate::data::star_rgb(s.bp_rp);
        let c = if truth {
            let p = Prov::Measured.rgb();
            [p[0], p[1], p[2], bright]
        } else {
            [rgb[0], rgb[1], rgb[2], bright]
        };
        let spike = if m < 0.4 { 0.32 } else { 0.0 };
        dl.point([x as f32, y as f32], core, core * 2.2, spike, c);
    }
    let _ = lw;
}

// ------------------------------------------------------------ solar layer

fn solar_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, cam, jd, truth) = (inp.u, inp.cam, inp.clock.jd, inp.modes.truth);
    let w_solar = band(lw, 5.0, 17.4, 0.7);
    if w_solar <= 0.001 {
        return;
    }
    let jd_ok = (1800.0..=2050.0).contains(&jd_year(jd));
    let orbit_prov = if jd_ok { Prov::Modeled } else { Prov::Inferred };
    use crate::evidence::Ev;
    seen(
        out,
        if sim::eph_is_spice(jd) {
            Ev::De440
        } else {
            Ev::ApproxElements
        },
        "planet states",
    );
    if inp.modes.orbits {
        seen(out, Ev::ApproxElements, "orbit lines");
    }

    sun_body(inp, proj, lw, w_solar, dl, out);

    for (i, p) in PLANETS.iter().enumerate() {
        let pos = obj_pos(u, ObjectRef::Planet(i), jd, inp.island);
        let a_px = (p.el[0] * AU / cam.mpp) as f32;
        let px = proj.px(pos);
        // orbit line: adaptive and clipped, so it stays smooth at any zoom. The
        // ink is brightest at the planet and fades around the arc behind it, so
        // the line itself shows the direction of travel. The osculating ellipse
        // is nudged through the body's actual position (ephemeris vs elements).
        if inp.modes.orbits && a_px > 7.0 && (p.radius_km * 1e3 / cam.mpp) < 200.0 {
            let selected = inp.focus == Some(ObjectRef::Planet(i));
            let ea_now = sim::planet_eccentric_anomaly(i, jd);
            let offset = pos - sim::planet_orbit_point(i, jd, ea_now);
            let f = |ea: f64| {
                let w = 0.5 + 0.5 * (ea - ea_now).cos();
                sim::planet_orbit_point(i, jd, ea) + offset * (w * w)
            };
            let tau = std::f64::consts::TAU;
            let grade = |ea: f64| {
                let behind = ((ea_now - ea).rem_euclid(tau) / tau) as f32;
                0.2 + 0.8 * (1.0 - behind) * (1.0 - behind)
            };
            let body_px = (p.radius_km * 1e3 / cam.mpp) as f32;
            let close = 1.0 - ((body_px - 40.0) / 160.0).clamp(0.0, 1.0);
            let fade = ((a_px - 7.0) / 18.0).clamp(0.0, 1.0)
                * (0.3 + 0.7 * (1800.0 / a_px).min(1.0))
                * close;
            let base = [
                theme::INK_GOLD[0] * 0.7 + p.color[0] * 0.3,
                theme::INK_GOLD[1] * 0.7 + p.color[1] * 0.3,
                theme::INK_GOLD[2] * 0.7 + p.color[2] * 0.3,
            ];
            let gain = if selected { 0.95 } else { 0.58 };
            let mut st = StrokeStyle::new(
                ink(base, gain * fade * w_solar as f32, truth, orbit_prov),
                if selected { 1.3 } else { 1.0 },
            )
            .glow(1.1);
            if !jd_ok {
                st = st.dashed(14.0, 0.55);
            }
            draw_curve(inp, proj, dl, st, &f, &grade, ea_now, ea_now + tau, 64);
        }
        // measured Horizons arc: an audit overlay, shown in Truth mode only
        if truth && inp.modes.trails && lw < 13.8 {
            seen(out, Ev::HorizonsVectors, "ephemeris arcs");
            if let Some(Some(t)) = u.planet_truth.get(i) {
                if t.covers(jd, 30.0) && a_px > 40.0 {
                    let pts: Vec<[f32; 2]> = t
                        .samples
                        .iter()
                        .step_by(2)
                        .map(|(_, w, _)| proj.px(*w))
                        .collect();
                    dl.polyline(
                        &pts,
                        false,
                        StrokeStyle::new(theme::rgba(Prov::Measured.rgb(), 0.35), 1.0)
                            .dashed(6.0, 0.5),
                    );
                }
            }
        }
        let r_px = (p.radius_km * 1e3 / cam.mpp) as f32;
        let ring_reach = ring_outer_m(i)
            .map(|m| (m / cam.mpp) as f32)
            .unwrap_or(r_px);
        if !on_screen(px, inp.w, inp.h, ring_reach.max(r_px * 1.3) + 40.0) {
            continue;
        }
        let vis = (a_px > 5.0 || r_px > 0.5) && w_solar > 0.05;
        if !vis {
            continue;
        }
        if r_px > 1.7 {
            draw_planet_disc(inp, proj, i, pos, px, r_px, dl, out);
        } else {
            let s = (1.5 + 0.9 * (1.0 - (i as f32 / 9.0))).min(2.4);
            let c = ink(p.color, 1.3, truth, Prov::Modeled);
            dl.point(px, s, s * 1.8, 0.0, c);
        }
        let pick_r = if p.name == "Saturn" { ring_reach } else { r_px };
        out.picks.push(Pickable {
            pos: px,
            r: pick_r.max(9.0),
            obj: ObjectRef::Planet(i),
        });
        if a_px > 26.0 {
            out.labels.push(Label {
                pos: [px[0], px[1] - pick_r.max(4.0) - 11.0],
                text: p.name.into(),
                size: 12.5,
                color: theme::rgba(theme::INK_WHITE, 0.85),
                priority: 20 - i as i32,
                obj: Some(ObjectRef::Planet(i)),
            });
        }
    }

    // spacecraft
    if lw > 10.0 {
        craft_layer(inp, proj, lw, dl, out);
    }
}

fn sun_body(
    inp: &BuildIn,
    proj: &Proj,
    lw: f64,
    w_solar: f64,
    dl: &mut DrawList,
    out: &mut FrameOut,
) {
    let cam = inp.cam;
    let jd = inp.clock.jd;
    let sun_px = proj.px(DVec3::ZERO);
    let r = (R_SUN_M / cam.mpp) as f32;
    if !on_screen(sun_px, inp.w, inp.h, r * 3.8 + 200.0) {
        return;
    }
    let zd = zmap(proj.screen(DVec3::ZERO)[2], cam.mpp * inp.w);
    if r > 1.2 {
        let m = sun_basis(jd);
        let (bx, by, bz) = view_axes(proj, m);
        let tex = TexId::Sun;
        if r > 6.0 {
            dl.want(tex);
        }
        let fade = if inp.tex.on_disk(tex) {
            inp.tex.fade(tex)
        } else {
            0.0
        };
        // a body close-up keeps surface detail; a distant Sun blazes
        let emission = 1.45 + 6.5 * (1.0 - smoothstep(6.0, 150.0, r as f64)) as f32;
        let mut d = DiscInst::plain(sun_px, [r, 0.0, KIND_STAR, 0.13], zd);
        d.sun = [0.0, 0.0, 1.0, 1.0];
        d.axis = [bz[0], bz[1], bz[2], 0.0];
        d.bx = [bx[0], bx[1], bx[2], fade];
        d.by = [by[0], by[1], by[2], 0.0];
        d.ca = [1.0, 0.78, 0.46, 1.0];
        d.cb = [1.0, 0.6, 0.3, 1.0];
        d.flags = [0.0, 0.0, 0.0, emission];
        dl.disc(d, zd, [fade.gt(&0.0).then_some(tex), None, None]);
        note_surface(out, r, SurfaceSource::Sun);
        seen(
            out,
            crate::evidence::Ev::CoronaIllustrative,
            "solar corona and granulation motion",
        );
        if fade > 0.0 {
            seen(
                out,
                crate::evidence::Ev::SurfaceImagery,
                "solar surface map",
            );
        }
    }
    // glare: fades out as the disc resolves, so there is no size pop
    let glare = 1.0 - smoothstep(4.0, 90.0, r as f64) as f32;
    if glare > 0.01 {
        let far = smoothstep(15.0, 17.2, lw) as f32;
        let core = (1.8 - 0.7 * far).max(1.1);
        let a = (2.8 - 1.4 * far) * glare * w_solar as f32;
        dl.point_z(
            sun_px,
            core.max(r * 0.8),
            13.0 - 7.0 * far,
            0.22 * (1.0 - far),
            [1.0, 0.84, 0.58, a],
            zd - 0.001,
        );
    }
    out.picks.push(Pickable {
        pos: sun_px,
        r: r.max(10.0),
        obj: ObjectRef::Sun,
    });
    if lw > 10.5 && lw < 16.6 {
        out.labels.push(Label {
            pos: [sun_px[0], sun_px[1] - r.max(8.0) - 12.0],
            text: "Sun".into(),
            size: 13.0,
            color: theme::rgba([1.0, 0.88, 0.62], 0.9),
            priority: 30,
            obj: Some(ObjectRef::Sun),
        });
    }
}

/// Which surface map belongs to a planet (whether or not it's on disk).
pub fn planet_tex(name: &str) -> Option<TexId> {
    Some(match name {
        "Mercury" => TexId::Mercury,
        "Venus" => TexId::Venus,
        "Earth" => TexId::EarthDay,
        "Mars" => TexId::Mars,
        "Jupiter" => TexId::Jupiter,
        "Saturn" => TexId::Saturn,
        "Uranus" => TexId::Uranus,
        "Neptune" => TexId::Neptune,
        _ => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_planet_disc(
    inp: &BuildIn,
    proj: &Proj,
    i: usize,
    pos: DVec3,
    px: [f32; 2],
    r_px: f32,
    dl: &mut DrawList,
    out: &mut FrameOut,
) {
    use crate::evidence::Ev;
    let p = &PLANETS[i];
    let jd = inp.clock.jd;
    let sun_dir = view_dir(proj, (-pos).normalize_or_zero());
    let (bx, by, bz) = view_axes(proj, sim::body_basis(i, jd));
    let seed = match p.name {
        "Jupiter" => 0.72,
        "Saturn" => 0.31,
        _ => 0.11 + i as f32 * 0.07,
    };
    // per-body surface character: ca.a = polar caps, cb.a = detail
    let (cap, det) = match p.name {
        "Mars" => (0.9, 1.15),
        "Mercury" => (0.0, 1.7),
        "Venus" => (0.0, 0.45),
        "Jupiter" => (0.0, 1.25),
        "Saturn" => (0.0, 1.0),
        "Uranus" => (0.0, 0.35),
        "Neptune" => (0.0, 0.8),
        "Pluto" => (0.0, 1.3),
        _ => (0.0, 1.0),
    };
    // atmosphere colour, strength, height in radii (exaggerated to be visible)
    let (rim, rim_a, atm_h): ([f32; 3], f32, f32) = match p.name {
        "Earth" => ([0.30, 0.55, 1.0], 1.0, 0.030),
        "Venus" => ([1.0, 0.84, 0.58], 0.8, 0.032),
        "Mars" => ([0.95, 0.60, 0.40], 0.42, 0.016),
        "Jupiter" => ([0.86, 0.78, 0.66], 0.30, 0.012),
        "Saturn" => ([0.92, 0.84, 0.64], 0.28, 0.012),
        "Uranus" => ([0.62, 0.90, 0.96], 0.55, 0.022),
        "Neptune" => ([0.45, 0.62, 1.0], 0.55, 0.022),
        _ => ([0.0; 3], 0.0, 0.0),
    };
    let relief = match p.name {
        "Mercury" => 0.5,
        "Mars" => 0.2,
        _ => 0.0,
    };
    let zd = zmap(proj.screen(pos)[2], inp.cam.mpp * inp.w);

    // imagery, requested only once a body is big enough to show it
    let tex = planet_tex(p.name);
    if r_px > 3.0 {
        if let Some(t) = tex {
            dl.want(t);
        }
    }
    let fade = tex
        .filter(|t| inp.tex.on_disk(*t))
        .map(|t| inp.tex.fade(t))
        .unwrap_or(0.0);
    let is_earth = p.kind == 4.0;
    let has_night = is_earth && r_px > 10.0 && inp.tex.on_disk(TexId::EarthNight);
    let has_clouds = is_earth && r_px > 10.0 && inp.tex.on_disk(TexId::EarthClouds);
    if has_night {
        dl.want(TexId::EarthNight);
    }
    if has_clouds {
        dl.want(TexId::EarthClouds);
    }
    let night_on = has_night && inp.tex.fade(TexId::EarthNight) > 0.0;
    let clouds_on = has_clouds && inp.tex.fade(TexId::EarthClouds) > 0.0;
    if fade > 0.0 {
        note_surface(out, r_px, SurfaceSource::Map);
        seen(out, Ev::SurfaceImagery, "surface maps");
    } else {
        let position = if sim::eph_is_spice(jd) {
            PositionSource::Kernel
        } else {
            PositionSource::PlanetElements
        };
        note_surface(out, r_px, SurfaceSource::Procedural(position));
        seen(out, Ev::ProceduralSurface, "surface patterns");
    }

    // rings
    let ring_def = RINGS.iter().find(|r| r.planet == i);
    let mut ring_fx = (0.0f32, 0.0f32, 0.0f32);
    let mut tex_c = if clouds_on {
        Some(TexId::EarthClouds)
    } else {
        None
    };
    if let Some(ring) = ring_def {
        let inner = ring.zones.iter().map(|z| z.0).fold(f64::MAX, f64::min);
        let outer = ring.zones.iter().map(|z| z.1).fold(0.0f64, f64::max);
        let base = (p.radius_km) as f32;
        let (ri, ro) = (inner as f32 / base, outer as f32 / base);
        let min_px = if p.name == "Saturn" { 2.2 } else { 28.0 };
        if r_px > min_px {
            let (map, tex_id) = if p.name == "Saturn" && inp.tex.on_disk(TexId::SaturnRing) {
                dl.want(TexId::SaturnRing);
                if inp.tex.fade(TexId::SaturnRing) > 0.0 {
                    (RingMap::File(TexId::SaturnRing), Some(TexId::SaturnRing))
                } else {
                    (RingMap::Zones(ring_zone_index(i)), None)
                }
            } else {
                (RingMap::Zones(ring_zone_index(i)), None)
            };
            let brightness = if p.name == "Saturn" { 1.55 } else { 0.5 };
            let inst = RingInst {
                center: px,
                r: [r_px, ri, ro, 0.0],
                pole: [bz[0], bz[1], bz[2], zd],
                sun: [sun_dir[0], sun_dir[1], sun_dir[2], brightness],
            };
            dl.rings_far.push(inst);
            let mut near = inst;
            near.r[3] = 1.0;
            near.pole[3] = zd - 0.0006;
            dl.rings_near.push(near);
            dl.ring_tex.push(map);
            seen(out, Ev::RingProfile, "ring radial profile");
            if tex_id.is_some() {
                ring_fx = (ri, ro, 1.0);
                tex_c = tex_id;
            }
        }
    }

    let mut d = DiscInst::plain(px, [r_px, p.flatten, p.kind, seed], zd);
    d.sun = [sun_dir[0], sun_dir[1], sun_dir[2], 0.05];
    d.axis = [bz[0], bz[1], bz[2], sim::body_w(i, jd) as f32];
    d.bx = [bx[0], bx[1], bx[2], fade];
    // clouds drift gently against the ground: one snapshot, not today's weather
    let cloud_off =
        (((jd - J2000) * 0.06) % std::f64::consts::TAU) as f32 + inp.anim_t as f32 * 0.0012;
    d.by = [by[0], by[1], by[2], cloud_off];
    d.ca = [p.color[0], p.color[1], p.color[2], cap];
    d.cb = [p.color2[0], p.color2[1], p.color2[2], det];
    d.rim = [rim[0], rim[1], rim[2], rim_a];
    d.fx = [atm_h, ring_fx.0, ring_fx.1, relief];
    d.flags = [
        if night_on { 1.0 } else { 0.0 },
        if clouds_on { 1.0 } else { 0.0 },
        ring_fx.2,
        0.0,
    ];
    let set: TexSet = [
        (fade > 0.0).then_some(tex).flatten(),
        night_on.then_some(TexId::EarthNight),
        tex_c,
    ];
    dl.disc(d, zd, set);
    if rim_a > 0.0 {
        seen(out, Ev::AtmosphereGlow, "atmosphere glow");
    }

    // mask the point-to-disc transition with a fading glow
    if r_px < 3.2 {
        let a = ((3.2 - r_px) / 2.0).clamp(0.0, 1.0) * 1.2;
        dl.point_z(
            px,
            r_px.max(1.6),
            3.2,
            0.0,
            [p.color[0], p.color[1], p.color[2], a],
            zd - 0.001,
        );
    }
    // Earth: vector overlays. With real imagery the coastlines become an audit
    // overlay for Truth mode; without it they carry the map.
    if is_earth && r_px > 26.0 {
        earth_vector_layer(inp, proj, pos, r_px, fade, night_on, dl, out);
    }
}

fn ring_zone_index(pi: usize) -> usize {
    RINGS.iter().position(|r| r.planet == pi).unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn earth_vector_layer(
    inp: &BuildIn,
    proj: &Proj,
    pos: DVec3,
    r_px: f32,
    imagery_fade: f32,
    night_map: bool,
    dl: &mut DrawList,
    out: &mut FrameOut,
) {
    let truth = inp.modes.truth;
    let jd = inp.clock.jd;
    // IAU body-fixed basis: x = prime meridian — geographic longitude is exact
    // now (S3), no alignment fudge
    let b = sim::body_basis(2, jd);
    let r_m = PLANETS[2].radius_km * 1e3 * 1.001;
    let to_world = |lon: f64, lat: f64| -> DVec3 {
        let (lo, la) = (lon.to_radians(), lat.to_radians());
        let local = b.col(0) * (la.cos() * lo.cos())
            + b.col(1) * (la.cos() * lo.sin())
            + b.col(2) * la.sin();
        pos + local * r_m
    };
    let front = |w: DVec3| -> bool { (w - pos).dot(proj.out) > 0.0 };
    let coast_alpha = if truth {
        0.85
    } else {
        0.62 * (1.0 - imagery_fade)
    };
    if coast_alpha > 0.01 {
        seen(out, crate::evidence::Ev::Coastlines, "Earth coastlines");
        dl.set_over(true);
        let c = ink([0.55, 0.85, 0.62], coast_alpha, truth, Prov::Measured);
        for line in &inp.u.coast {
            let mut run: Vec<[f32; 2]> = Vec::new();
            for pt in line {
                let w = to_world(pt[0] as f64, pt[1] as f64);
                if front(w) {
                    run.push(proj.px(w));
                } else if run.len() > 1 {
                    dl.polyline(&run, false, StrokeStyle::new(c, 1.0));
                    run.clear();
                } else {
                    run.clear();
                }
            }
            if run.len() > 1 {
                dl.polyline(&run, false, StrokeStyle::new(c, 1.0));
            }
        }
        dl.set_over(false);
    }
    // graticule: a reference for Truth mode only
    if truth {
        dl.set_over(true);
        let g = ink(theme::INK_FAINT, 0.4, truth, Prov::Illustrative);
        for lat in [-60.0f64, -30.0, 0.0, 30.0, 60.0] {
            let mut run: Vec<[f32; 2]> = Vec::new();
            for s in 0..=96 {
                let lon = s as f64 / 96.0 * 360.0;
                let w = to_world(lon, lat);
                if front(w) {
                    run.push(proj.px(w));
                } else {
                    if run.len() > 1 {
                        dl.polyline(&run, false, StrokeStyle::new(g, 0.7));
                    }
                    run.clear();
                }
            }
            if run.len() > 1 {
                dl.polyline(&run, false, StrokeStyle::new(g, 0.7));
            }
        }
        dl.set_over(false);
    }

    // No night map: synthetic land-masked lights (labeled illustrative).
    if !night_map && imagery_fade < 0.5 && r_px > 60.0 && !inp.u.night_lights.is_empty() {
        seen(out, crate::evidence::Ev::LandLights, "night lights");
        let zd = zmap(proj.screen(pos)[2], inp.cam.mpp * inp.w) - 0.0008;
        let sun = (-pos).normalize_or_zero();
        let scale = (r_px / 600.0).clamp(0.6, 1.6);
        for l in &inp.u.night_lights {
            let w = to_world(l[0] as f64, l[1] as f64);
            let dir = (w - pos) / r_m;
            if dir.dot(proj.out) <= 0.04 {
                continue; // back hemisphere
            }
            let night = -dir.dot(sun); // 0 at terminator → 1 at midnight
            if night < 0.02 {
                continue;
            }
            let a = 0.6 * ((night - 0.02) / 0.16).clamp(0.0, 1.0) as f32;
            dl.point_z(
                proj.px(w),
                0.85 * scale,
                2.4 * scale,
                0.0,
                [1.0, 0.78, 0.46, a],
                zd,
            );
        }
    }
}

// ------------------------------------------------------------ moons layer

fn moons_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, cam, jd, truth) = (inp.u, inp.cam, inp.clock.jd, inp.modes.truth);
    if lw > 11.4 {
        return;
    }
    let mut any_spice = false;
    let mut any_mean = false;
    for (mi, m) in MOONS.iter().enumerate() {
        let a_px = (m.a_km * 1e3 / cam.mpp) as f32;
        // the ORBIT is only worth drawing in a size window, but the BODY must
        // never be culled by its own orbit size (diving to a moon previously
        // showed empty space, found via the luna shot)
        if a_px < 5.0 {
            continue;
        }
        let ppos = obj_pos(u, ObjectRef::Planet(m.planet), jd, inp.island);
        let ppx = proj.px(ppos);
        if !on_screen(ppx, inp.w, inp.h, a_px + 200.0) {
            continue;
        }
        if sim::moon_is_spice(mi, jd) {
            any_spice = true;
        } else {
            any_mean = true;
        }
        let rel = if let Some(isl) = inp.island {
            obj_pos(u, ObjectRef::Moon(mi), jd, Some(isl)) - ppos
        } else {
            moon_rel(mi, jd)
        };
        let mpos = ppos + rel;
        let mpx = proj.px(mpos);
        // orbit: one period of the same ephemeris that places the moon, so the
        // line passes exactly through the body, at any zoom
        let is_luna = m.name == "Moon";
        if inp.modes.orbits {
            let fade = ((a_px - 5.0) / 30.0).clamp(0.1, 1.0);
            let period = m.period_d.abs();
            let f = |t: f64| ppos + moon_rel(mi, jd + t * period);
            let grade = |t: f64| {
                let ahead = t as f32;
                0.25 + 0.75 * ahead * ahead
            };
            let selected = inp.focus == Some(ObjectRef::Moon(mi));
            let c = ink(
                theme::INK_GOLD_DIM,
                (if selected { 0.95 } else { 0.62 }) * fade,
                truth,
                Prov::Modeled,
            );
            draw_curve(
                inp,
                proj,
                dl,
                StrokeStyle::new(c, 0.95).glow(0.9),
                &f,
                &grade,
                -1.0,
                0.0,
                40,
            );
        }
        // measured Moon arc from Horizons (geocentric): Truth-mode audit overlay
        if truth && is_luna && inp.modes.trails {
            if let Some(t) = &u.moon_truth {
                if t.covers(jd, 5.0) {
                    let pts: Vec<[f32; 2]> = t
                        .samples
                        .iter()
                        .map(|(_, rel, _)| proj.px(ppos + *rel))
                        .collect();
                    dl.polyline(
                        &pts,
                        false,
                        StrokeStyle::new(theme::rgba(Prov::Measured.rgb(), 0.4), 1.0)
                            .dashed(5.0, 0.55),
                    );
                }
            }
        }
        let r_px = (m.radius_km * 1e3 / cam.mpp) as f32;
        if r_px > 1.7 {
            let sun_dir = view_dir(proj, (-mpos).normalize_or_zero());
            let mdet = match m.name {
                "Moon" => 1.6,
                "Io" => 1.4,
                "Callisto" | "Mimas" => 1.3,
                "Europa" | "Enceladus" => 0.5,
                _ => 0.9,
            };
            let zd = zmap(proj.screen(mpos)[2], cam.mpp * inp.w);
            // moons keep one face to the parent (mean tidal lock, no libration)
            let pole = planet_axis(m.planet, jd);
            let pole = if is_luna {
                (DVec3::Z + pole * 0.05).normalize()
            } else {
                pole
            };
            let to_parent = (-rel).normalize_or_zero();
            let x = (to_parent - pole * to_parent.dot(pole)).normalize_or_zero();
            let (bx, by, bz) = view_axes(proj, DMat3::from_cols(x, pole.cross(x), pole));
            let tex = if is_luna { Some(TexId::Moon) } else { None };
            if r_px > 3.0 {
                if let Some(t) = tex {
                    dl.want(t);
                }
            }
            let fade = tex
                .filter(|t| inp.tex.on_disk(*t))
                .map(|t| inp.tex.fade(t))
                .unwrap_or(0.0);
            if fade > 0.0 {
                note_surface(out, r_px, SurfaceSource::Map);
                seen(out, crate::evidence::Ev::SurfaceImagery, "surface maps");
            } else {
                let position = if sim::moon_is_spice(mi, jd) {
                    PositionSource::Kernel
                } else if is_luna {
                    PositionSource::LunarModel
                } else {
                    PositionSource::MoonElements
                };
                note_surface(out, r_px, SurfaceSource::Procedural(position));
                seen(
                    out,
                    crate::evidence::Ev::ProceduralSurface,
                    "surface patterns",
                );
            }
            let mut d = DiscInst::plain(mpx, [r_px, 0.0, m.kind, 0.23 + mi as f32 * 0.041], zd);
            d.sun = [sun_dir[0], sun_dir[1], sun_dir[2], 0.05];
            d.axis = [bz[0], bz[1], bz[2], 0.0];
            d.bx = [bx[0], bx[1], bx[2], fade];
            d.by = [by[0], by[1], by[2], 0.0];
            d.ca = [m.color[0], m.color[1], m.color[2], 0.0];
            d.cb = [m.color2[0], m.color2[1], m.color2[2], mdet];
            if m.name == "Titan" {
                d.rim = [0.86, 0.60, 0.30, 0.75];
                d.fx[0] = 0.05;
            }
            d.fx[3] = if is_luna { 0.7 } else { 0.0 };
            dl.disc(d, zd, [(fade > 0.0).then_some(TexId::Moon), None, None]);
            if r_px < 3.2 {
                let a = ((3.2 - r_px) / 2.0).clamp(0.0, 1.0) * 1.1;
                dl.point_z(
                    mpx,
                    r_px.max(1.4),
                    2.6,
                    0.0,
                    [m.color[0], m.color[1], m.color[2], a],
                    zd - 0.001,
                );
            }
        } else {
            dl.point(mpx, 1.2, 2.0, 0.0, ink(m.color, 1.25, truth, Prov::Modeled));
        }
        out.picks.push(Pickable {
            pos: mpx,
            r: r_px.max(8.0),
            obj: ObjectRef::Moon(mi),
        });
        if a_px > 60.0 {
            out.labels.push(Label {
                pos: [mpx[0], mpx[1] - r_px.max(3.0) - 10.0],
                text: m.name.into(),
                size: 11.5,
                color: theme::rgba(theme::INK_WHITE, 0.72),
                priority: 8,
                obj: Some(ObjectRef::Moon(mi)),
            });
        }
    }
    if any_spice {
        seen(out, crate::evidence::Ev::SatelliteSpk, "moon states");
    }
    if any_mean {
        seen(
            out,
            crate::evidence::Ev::MoonMeanElements,
            "moon states (no kernel)",
        );
    }
}

// ----------------------------------------------------------- particle belts

/// Illustrative small bodies: seeded, on near-circular orbits, moving at their
/// Keplerian rate when time runs. They show where belts are, not which rocks.
#[allow(clippy::too_many_arguments)]
fn belt(
    inp: &BuildIn,
    proj: &Proj,
    dl: &mut DrawList,
    n: usize,
    seed: usize,
    radius: &dyn Fn(usize) -> Option<f64>,
    thickness: f64,
    alpha: f32,
    rgb: [f32; 3],
) {
    let truth = inp.modes.truth;
    let days = inp.clock.jd - J2000;
    for k in 0..n {
        let Some(r_au) = radius(k) else { continue };
        let th0 = seeded(k, seed + 2) * std::f64::consts::TAU;
        let rate = std::f64::consts::TAU / (365.25 * r_au.powf(1.5));
        let th = th0 + rate * days;
        let z = (seeded(k, seed + 3) - 0.5) * thickness * r_au * AU;
        let r = r_au * AU;
        let p = proj.px(DVec3::new(r * th.cos(), r * th.sin(), z));
        if on_screen(p, inp.w, inp.h, 3.0) {
            let a = alpha * (0.4 + 0.9 * seeded(k, seed + 4) as f32);
            dl.point(p, 0.55, 0.9, 0.0, ink(rgb, a, truth, Prov::Illustrative));
        }
    }
}

fn belts_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let aw = band(lw, 10.5, 13.3, 0.5) * ((2.75 * AU / inp.cam.mpp) / 140.0).clamp(0.08, 1.0);
    if aw > 0.01 {
        seen(
            out,
            crate::evidence::Ev::GlyphIllustrative,
            "asteroid belt sample",
        );
        // the Kirkwood gaps (resonances with Jupiter) are real; the rocks are not
        let radius = |k: usize| {
            let r = 2.1 + 1.2 * seeded(k, 11);
            let gap = [2.50, 2.82, 2.95, 3.27]
                .iter()
                .any(|g| (r - g).abs() < 0.035);
            (!gap).then_some(r)
        };
        belt(
            inp,
            proj,
            dl,
            3200,
            11,
            &radius,
            0.28,
            0.55 * aw as f32,
            [0.74, 0.68, 0.58],
        );
        // Jupiter's Trojans, clustered 60 degrees ahead of and behind the planet
        let jpos = obj_pos(inp.u, ObjectRef::Planet(4), inp.clock.jd, inp.island);
        let jlon = jpos.y.atan2(jpos.x);
        for k in 0..700 {
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let lon = jlon + side * 1.047 + (seeded(k, 21) - 0.5) * 0.72;
            let r = (5.2 + (seeded(k, 22) - 0.5) * 0.7) * AU;
            let z = (seeded(k, 23) - 0.5) * 0.4 * r;
            let p = proj.px(DVec3::new(r * lon.cos(), r * lon.sin(), z));
            if on_screen(p, inp.w, inp.h, 3.0) {
                let a = 0.5 * aw as f32 * (0.4 + 0.8 * seeded(k, 24) as f32);
                dl.point(
                    p,
                    0.55,
                    0.9,
                    0.0,
                    ink([0.70, 0.62, 0.52], a, inp.modes.truth, Prov::Illustrative),
                );
            }
        }
    }
    let kw = band(lw, 12.2, 15.2, 0.5) * ((44.0 * AU / inp.cam.mpp) / 240.0).clamp(0.08, 1.0);
    if kw > 0.01 {
        seen(
            out,
            crate::evidence::Ev::GlyphIllustrative,
            "Kuiper belt sample",
        );
        let radius = |k: usize| {
            // plutinos pile up at the 3:2 resonance; the cold belt lies at 42-48 AU
            Some(if k.is_multiple_of(4) {
                39.4 + (seeded(k, 31) - 0.5) * 1.1
            } else {
                41.5 + 6.5 * seeded(k, 31)
            })
        };
        belt(
            inp,
            proj,
            dl,
            3200,
            31,
            &radius,
            0.18,
            0.62 * kw as f32,
            [0.62, 0.70, 0.84],
        );
    }
    if lw > 12.3 && lw < 14.2 {
        let a = (band(lw, 12.3, 14.2, 0.3) * 0.5) as f32;
        let r_px = (44.0 * AU / inp.cam.mpp) as f32;
        let c = proj.px(DVec3::ZERO);
        out.labels.push(Label {
            pos: [c[0], c[1] + r_px],
            text: "Kuiper belt".into(),
            size: 11.5,
            color: theme::rgba([0.62, 0.70, 0.84], a * 1.5),
            priority: 3,
            obj: None,
        });
    }
    if lw > 11.0 && lw < 12.6 {
        let a = band(lw, 11.0, 12.6, 0.25) as f32;
        let r_px = (2.75 * AU / inp.cam.mpp) as f32;
        let c = proj.px(DVec3::ZERO);
        out.labels.push(Label {
            pos: [c[0], c[1] + r_px],
            text: "Asteroid belt".into(),
            size: 11.5,
            color: theme::rgba([0.76, 0.70, 0.60], 0.75 * a),
            priority: 3,
            obj: None,
        });
    }
}

// ------------------------------------------------------- heliosphere layer

fn heliosphere_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let truth = inp.modes.truth;
    let w = band(lw, 12.6, 16.2, 0.6);
    let sun_px = proj.px(DVec3::ZERO);
    let px_per_au = (AU / inp.cam.mpp) as f32;
    // interstellar flow arrives from ecliptic longitude 255, latitude 5 (IBEX);
    // the tail streams the other way. The glow's shape is a sketch of that.
    let upstream = {
        let (lon, lat) = (255.4f64.to_radians(), 5.2f64.to_radians());
        DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin())
    };
    let down = -upstream;
    let (dx, dy) = (down.dot(proj.right), -down.dot(proj.up));
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let w_helio = band(lw, 12.4, 15.4, 0.6);
    out.uni.helio = [
        sun_px[0],
        sun_px[1],
        px_per_au,
        (w_helio * if truth { 0.35 } else { 1.0 }) as f32,
    ];
    let w_oort = band(lw, 14.5, 17.5, 0.7);
    out.uni.helio_b = [
        (dx / len) as f32,
        (dy / len) as f32,
        (w_oort * if truth { 0.35 } else { 1.0 }) as f32,
        0.0,
    ];
    if w <= 0.01 {
        return;
    }
    seen(
        out,
        crate::evidence::Ev::GlyphIllustrative,
        "heliosphere and Oort cloud",
    );
    for (r_au, name, dash) in [
        (90.0, "Termination shock", 9.0),
        (123.0, "Heliopause", 14.0),
    ] {
        let r_px = (r_au * AU / inp.cam.mpp) as f32;
        if r_px > 60.0 && r_px < 4_000.0 {
            let c = ink(
                [0.5, 0.62, 0.85],
                0.32 * w as f32,
                truth,
                Prov::Illustrative,
            );
            dl.circle(sun_px, r_px, StrokeStyle::new(c, 1.0).dashed(dash, 0.55));
            out.labels.push(Label {
                pos: [sun_px[0] + r_px * 0.71, sun_px[1] - r_px * 0.71],
                text: name.into(),
                size: 11.0,
                color: theme::rgba([0.58, 0.70, 0.92], 0.75),
                priority: 4,
                obj: None,
            });
        }
    }
    // Oort cloud: a soft shell in the band shader plus a sprinkling of comets
    let ow = band(lw, 14.6, 17.2, 0.5);
    if ow > 0.01 {
        for k in 0..6000 {
            let r = (2_000.0 + 98_000.0 * seeded(k, 5).powf(1.5)) * AU;
            let th = seeded(k, 6) * std::f64::consts::TAU;
            let sinph = 2.0 * seeded(k, 7) - 1.0;
            let cosph = (1.0 - sinph * sinph).sqrt();
            let p = proj.px(DVec3::new(
                r * cosph * th.cos(),
                r * cosph * th.sin(),
                r * sinph,
            ));
            if on_screen(p, inp.w, inp.h, 4.0) {
                let a = 0.5 * ow as f32 * (0.3 + 0.9 * seeded(k, 8) as f32);
                dl.point(
                    p,
                    0.55,
                    0.9,
                    0.0,
                    ink([0.58, 0.66, 0.84], a, truth, Prov::Illustrative),
                );
            }
        }
    }
}

fn craft_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, jd, truth) = (inp.u, inp.clock.jd, inp.modes.truth);
    let w = band(lw, 10.0, 16.2, 0.6);
    if w <= 0.01 || u.craft.is_empty() {
        return;
    }
    seen(
        out,
        crate::evidence::Ev::HorizonsVectors,
        "spacecraft trajectories",
    );
    for (ci, c) in u.craft.iter().enumerate() {
        let (jd0, jd1) = c.track.span();
        let selected = inp.focus == Some(ObjectRef::Craft(ci));
        if inp.modes.trails && jd > jd0 {
            let upto = jd.min(jd1);
            // resampled through the Hermite track, so looping paths stay smooth;
            // Parker's rosette around the Sun is kept quiet unless it's selected
            let quiet = c.name == "Parker Solar Probe" && !selected;
            let a = (if selected {
                0.85
            } else if quiet {
                0.05
            } else {
                0.24
            }) * w as f32;
            let mut st = StrokeStyle::new(ink(c.color, a, truth, Prov::Measured), 1.0);
            if !quiet {
                st = st.glow(0.9);
            }
            let f = |t: f64| {
                c.track
                    .pos_at(jd0 + (upto - jd0) * t)
                    .unwrap_or(DVec3::ZERO)
            };
            let one = |_t: f64| 1.0f32;
            let seeds = (c.track.samples.len() / 3).clamp(48, 320);
            draw_curve(inp, proj, dl, st, &f, &one, 0.0, 1.0, seeds);
        }
        if jd >= jd0 {
            // No marker outside trajectory coverage. Cassini
            // keeps a dimmed memorial at its final state (mission ended there).
            let past_horizon = jd > jd1;
            if past_horizon && c.name != "Cassini" {
                continue;
            }
            let pos = obj_pos(u, ObjectRef::Craft(ci), jd, None);
            let px = proj.px(pos);
            if on_screen(px, inp.w, inp.h, 40.0) {
                let ended = past_horizon && c.name == "Cassini";
                let a = if past_horizon { 0.4 } else { 1.4 };
                dl.point(
                    px,
                    1.4,
                    3.2,
                    0.0,
                    ink(c.color, a * w as f32, truth, Prov::Measured),
                );
                out.picks.push(Pickable {
                    pos: px,
                    r: 9.0,
                    obj: ObjectRef::Craft(ci),
                });
                out.labels.push(Label {
                    pos: [px[0] + 8.0, px[1] - 10.0],
                    text: if ended {
                        format!("{} (ended 2017)", c.name)
                    } else {
                        c.name.into()
                    },
                    size: 11.0,
                    color: theme::rgba(c.color, 0.85 * w as f32),
                    priority: 10,
                    obj: Some(ObjectRef::Craft(ci)),
                });
            }
        }
    }
}

// ------------------------------------------------------------- stars layer

fn stars_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, truth) = (inp.u, inp.modes.truth);
    let w = band(lw, 15.9, 19.9, 0.8);
    if w <= 0.005 {
        return;
    }
    seen(out, crate::evidence::Ev::GaiaDr3, "star field");
    // as the catalog's dense ball shrinks on screen, faint stars recede so it
    // does not collapse into a white blob
    let crowd = (1.0 - (lw as f32 - 17.5) * 0.6).clamp(0.09, 1.0);
    let spike_w = 1.0 - smoothstep(17.7, 18.5, lw) as f32;
    let margin = 24.0f32;
    for (si, s) in u.stars.iter().enumerate() {
        let pos = s.pos_at(inp.clock.jd); // one epoch-aware provider
        let px = proj.px(pos);
        if !on_screen(px, inp.w, inp.h, margin) {
            continue;
        }
        let m = s.absmag;
        let core = (1.6 - 0.09 * m).clamp(0.7, 3.1) * (0.72 + 0.28 * crowd);
        let bright = (2.9 - 0.2 * m).clamp(0.36, 6.0)
            * w as f32
            * (crowd + (m < 1.0) as i32 as f32 * 0.5 * crowd);
        let rgb = crate::data::star_rgb(s.bp_rp);
        let c = if truth {
            let p = Prov::Measured.rgb();
            [
                p[0] * 0.6 + rgb[0] * 0.4,
                p[1] * 0.6 + rgb[1] * 0.4,
                p[2] * 0.6 + rgb[2] * 0.4,
                bright,
            ]
        } else {
            [rgb[0], rgb[1], rgb[2], bright]
        };
        // only the true luminaries get a diffraction cross
        let spike = if m < -1.0 { 0.4 * spike_w } else { 0.0 };
        let glow = core * (1.5 + (1.6 - m * 0.16).clamp(0.0, 1.6));
        dl.point(px, core, glow, spike, c);
        if core > 1.2 || s.name.is_some() {
            out.picks.push(Pickable {
                pos: px,
                r: core.max(7.0),
                obj: ObjectRef::Star(si),
            });
        }
        if let Some(name) = s.name {
            if lw > 16.6 {
                out.labels.push(Label {
                    pos: [px[0], px[1] - core - 10.0],
                    text: name.into(),
                    size: 11.5,
                    color: theme::rgba(theme::INK_WHITE, 0.72 * w as f32),
                    priority: (10.0 - m) as i32,
                    obj: Some(ObjectRef::Star(si)),
                });
            }
        }
    }
    // You are here
    let sun_px = proj.px(DVec3::ZERO);
    if lw > 16.6 && on_screen(sun_px, inp.w, inp.h, 20.0) {
        out.labels.push(Label {
            pos: [sun_px[0], sun_px[1] - 16.0],
            text: "You are here".into(),
            size: 12.5,
            color: theme::rgba(theme::INK_GOLD, 0.95 * w as f32),
            priority: 60,
            obj: Some(ObjectRef::Sun),
        });
        dl.circle(
            sun_px,
            7.0,
            StrokeStyle::new(theme::rgba(theme::INK_GOLD, 0.75 * w as f32), 1.0),
        );
    }
    // where measurement stops: honest and, at this scale, useful. Shown from when the
    // ring fits the view until it has shrunk to a small mark.
    let reach = inp.u.gaia_reach_pc;
    let reach_lw = (reach.max(1.0) * PC).log10();
    let edge_w = band(lw, reach_lw - 0.2, reach_lw + 1.3, 0.35) as f32;
    if reach > 0.0 && edge_w > 0.01 {
        let r_px = (reach * PC / inp.cam.mpp) as f32;
        if r_px > 24.0 {
            dl.circle(
                sun_px,
                r_px,
                StrokeStyle::new(theme::rgba(theme::INK_BLUE, 0.22 * edge_w), 1.0).dashed(9.0, 0.5),
            );
            out.labels.push(Label {
                pos: [sun_px[0] - r_px * 0.71, sun_px[1] + r_px * 0.71],
                text: format!("Gaia's dense sample ends here ({reach:.0} pc); beyond it, naked-eye stars only"),
                size: 11.0,
                color: theme::rgba(theme::INK_BLUE, 0.8 * edge_w),
                priority: 6,
                obj: None,
            });
        }
    }
    // exoplanet hosts: small rings you can dive into, ranked rather than a wall
    // of identical circles: multi-planet systems first, then near centre
    let pw = band(lw, 15.9, 18.8, 0.6);
    if pw > 0.02 {
        seen(out, crate::evidence::Ev::ExoArchive, "exoplanet hosts");
        let (cx, cy) = (inp.w as f32 * 0.5, inp.h as f32 * 0.5);
        let mut cands: Vec<(f32, usize, [f32; 2])> = Vec::new();
        for (ei, e) in u.exos.iter().enumerate() {
            if e.dist_pc.is_none() || e.planets.is_empty() {
                continue;
            }
            let px = proj.px(e.pos);
            if !on_screen(px, inp.w, inp.h, 10.0) {
                continue;
            }
            let d2 = (px[0] - cx).powi(2) + (px[1] - cy).powi(2);
            let rank = d2 / (1.0 + e.planets.len() as f32 * 2.0);
            cands.push((rank, ei, px));
        }
        cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (k, (_, ei, px)) in cands.iter().take(40).enumerate() {
            let e = &u.exos[*ei];
            let fade = if k < 16 { 1.0 } else { 0.45 };
            let c = ink(
                theme::INK_GOLD,
                0.62 * pw as f32 * fade,
                truth,
                Prov::Measured,
            );
            dl.circle(*px, 5.0, StrokeStyle::new(c, 0.9));
            out.picks.push(Pickable {
                pos: *px,
                r: 8.0,
                obj: ObjectRef::ExoHost(*ei),
            });
            if lw < 17.6 && k < 14 {
                out.labels.push(Label {
                    pos: [px[0], px[1] + 14.0],
                    text: format!("{} ({})", e.host, e.planets.len()),
                    size: 10.5,
                    color: theme::rgba(theme::INK_GOLD, 0.78 * pw as f32),
                    priority: 5 + e.planets.len() as i32,
                    obj: Some(ObjectRef::ExoHost(*ei)),
                });
            }
        }
    }
}

// ------------------------------------------------------------ galaxy layer

/// Radii along the Sun's azimuth (kpc) of the stylized arms in shaders/band.wgsl.
const ARM_NAMES: [(&str, f64); 4] = [
    ("Scutum-Centaurus Arm", 5.0),
    ("Sagittarius Arm", 7.0),
    ("Perseus Arm", 9.8),
    ("Outer Arm", 13.7),
];

fn galaxy_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let u = inp.u;
    let w = band(lw, 18.9, 23.3, 0.7);
    if w <= 0.01 {
        return;
    }
    seen(
        out,
        crate::evidence::Ev::GalaxyFog,
        "Milky Way illustration",
    );
    let gc_px = proj.px(u.gal.center);
    // Sun marker
    let sun_px = proj.px(DVec3::ZERO);
    if on_screen(sun_px, inp.w, inp.h, 60.0) && w > 0.15 && lw < 22.8 {
        dl.circle(
            sun_px,
            6.0,
            StrokeStyle::new(theme::rgba(theme::INK_GOLD, 0.9 * w as f32), 1.1),
        );
        dl.point(
            sun_px,
            1.2,
            3.0,
            0.0,
            theme::rgba(theme::INK_GOLD, 1.5 * w as f32),
        );
        out.labels.push(Label {
            pos: [sun_px[0], sun_px[1] - 16.0],
            text: "You are here".into(),
            size: 12.5,
            color: theme::rgba(theme::INK_GOLD, 0.95 * w as f32),
            priority: 50,
            obj: Some(ObjectRef::Sun),
        });
    }
    if on_screen(gc_px, inp.w, inp.h, 60.0) && w > 0.15 && lw < 22.6 {
        out.labels.push(Label {
            pos: [gc_px[0], gc_px[1] + 18.0],
            text: "Galactic centre".into(),
            size: 12.0,
            color: theme::rgba([1.0, 0.92, 0.76], 0.9 * w as f32),
            priority: 45,
            obj: None,
        });
    }
    // arm names, placed along each stylized arm at a fixed azimuth
    let aw = band(lw, 20.4, 22.2, 0.3) as f32;
    if aw > 0.01 && !inp.modes.truth {
        for (k, (name, r0)) in ARM_NAMES.iter().enumerate() {
            let az = -1.1 + k as f64 * 0.55;
            let r = r0 * (0.2126 * az).exp();
            // azimuth from the Sun's direction: galactocentric (-x, y) in the plane
            let x = -r * az.cos();
            let y = r * az.sin();
            let p = proj.px(u.gal.center + u.gal.x * (x * KPC) + u.gal.y * (y * KPC));
            out.labels.push(Label {
                pos: p,
                text: (*name).into(),
                size: 11.5,
                color: theme::rgba([0.70, 0.78, 1.0], 0.75 * aw),
                priority: 7,
                obj: None,
            });
        }
    }
}

/// Measured orientations: position angle and inclination in degrees.
fn galaxy_orientation(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "Andromeda (M31)" => (37.7, 77.5),
        "Triangulum (M33)" => (23.0, 55.0),
        "Large Magellanic Cloud" => (170.0, 34.0),
        "Small Magellanic Cloud" => (45.0, 70.0),
        _ => return None,
    })
}

/// The disc plane of a galaxy from its sky position, position angle and
/// inclination, as two unit vectors in the root frame.
fn galaxy_plane(pos: DVec3, pa_deg: f64, inc_deg: f64) -> (DVec3, DVec3) {
    let s = pos.normalize_or_zero();
    let pole_icrs = crate::frames::ecl_from_icrs() * DVec3::Z;
    let east = pole_icrs.cross(s).normalize_or_zero();
    let north = s.cross(east);
    let (pa, inc) = (pa_deg.to_radians(), inc_deg.to_radians());
    let major = north * pa.cos() + east * pa.sin();
    let minor_sky = -north * pa.sin() + east * pa.cos();
    let minor = minor_sky * inc.cos() + s * inc.sin();
    (major, minor)
}

/// Emit a galaxy sprite for a disc in the plane (ax, ay), or a face-on one.
#[allow(clippy::too_many_arguments)]
fn push_galaxy(
    proj: &Proj,
    dl: &mut DrawList,
    px: [f32; 2],
    radius_m: f64,
    plane: Option<(DVec3, DVec3)>,
    kind: f32,
    seed: f32,
    tint: [f32; 4],
    mpp: f64,
) {
    let r_px = (radius_m / mpp) as f32;
    let (a, b): ([f32; 2], [f32; 2]) = match plane {
        Some((ax, ay)) => (
            [
                (ax.dot(proj.right) * radius_m / mpp) as f32,
                (-ax.dot(proj.up) * radius_m / mpp) as f32,
            ],
            [
                (ay.dot(proj.right) * radius_m / mpp) as f32,
                (-ay.dot(proj.up) * radius_m / mpp) as f32,
            ],
        ),
        None => {
            let ang = seed * std::f32::consts::TAU;
            let (c, s) = (ang.cos(), ang.sin());
            ([r_px * c, r_px * s], [-r_px * s * 0.72, r_px * c * 0.72])
        }
    };
    // an edge-on disc still has thickness
    let mut bb = b;
    let la = (a[0] * a[0] + a[1] * a[1]).sqrt().max(1e-3);
    let cross = a[0] * b[1] - a[1] * b[0];
    let min_cross = la * la * 0.09;
    if cross.abs() < min_cross {
        let sign = if cross >= 0.0 { 1.0 } else { -1.0 };
        let perp = [-a[1] / la, a[0] / la];
        let add = (min_cross - cross.abs()) / la * sign;
        bb = [b[0] + perp[0] * add, b[1] + perp[1] * add];
    }
    let det = a[0] * bb[1] - a[1] * bb[0];
    if det.abs() < 1e-5 {
        return;
    }
    // inverse of [a b]: screen px -> galaxy plane in units of its radius
    let m = [bb[1] / det, -bb[0] / det, -a[1] / det, a[0] / det];
    let quad = la
        .max((bb[0] * bb[0] + bb[1] * bb[1]).sqrt())
        .max(r_px * 0.3)
        * 1.06
        + 2.0;
    dl.galaxies.push(GalaxyInst {
        center: px,
        m,
        shape: [r_px.max(0.6), kind, seed, quad],
        tint,
    });
}

fn draw_galaxy_glyph(
    inp: &BuildIn,
    proj: &Proj,
    gi: usize,
    px: [f32; 2],
    dl: &mut DrawList,
    out: &mut FrameOut,
    w: f64,
) {
    let g = &inp.u.galaxies[gi];
    let truth = inp.modes.truth;
    let r_px = ((g.size_kpc * 0.5 * KPC) / inp.cam.mpp) as f32;
    let plane = galaxy_orientation(g.name).map(|(pa, inc)| galaxy_plane(g.pos, pa, inc));
    let irregular = matches!(
        g.name,
        "Large Magellanic Cloud"
            | "Small Magellanic Cloud"
            | "NGC 6822"
            | "IC 10"
            | "WLM"
            | "IC 1613"
            | "Sextans A"
            | "Sextans B"
            | "NGC 3109"
            | "Leo A"
            | "Pegasus dIrr"
    );
    let kind = if g.spiral {
        0.0
    } else if irregular {
        1.0
    } else {
        2.0
    };
    let seed = 0.13 + gi as f32 * 0.137;
    let tint = if truth {
        let c = Prov::Illustrative.rgb();
        [c[0], c[1], c[2], 0.7 * w as f32]
    } else if g.spiral {
        [0.60, 0.72, 1.0, 1.9 * w as f32]
    } else {
        // dwarfs are faint: bright enough to find when tiny, quiet when resolved
        let quiet = smoothstep(4.0, 22.0, r_px as f64) as f32;
        [0.72, 0.78, 1.0, (1.7 - 1.3 * quiet) * w as f32]
    };
    push_galaxy(
        proj,
        dl,
        px,
        g.size_kpc * 0.5 * KPC,
        plane,
        kind,
        seed,
        tint,
        inp.cam.mpp,
    );
    // something to see and click, however small on screen
    if r_px < 2.5 {
        dl.point(px, 0.9, 1.8, 0.0, [0.86, 0.86, 1.0, 1.1 * w as f32]);
    }
    let pick = r_px.max(9.0);
    out.picks.push(Pickable {
        pos: px,
        r: pick,
        obj: ObjectRef::Galaxy(gi),
    });
    out.labels.push(Label {
        pos: [px[0], px[1] - r_px.max(4.0) - 10.0],
        text: g.name.into(),
        size: 11.5,
        color: theme::rgba(theme::INK_WHITE, 0.78 * w as f32),
        priority: if g.spiral { 30 } else { 6 },
        obj: Some(ObjectRef::Galaxy(gi)),
    });
}

// -------------------------------------------------------- local group layer

fn localgroup_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let u = inp.u;
    let w = band(lw, 20.2, 24.9, 0.6);
    if w <= 0.02 {
        return;
    }
    seen(
        out,
        crate::evidence::Ev::CompiledCatalog,
        "Local Group positions",
    );
    seen(
        out,
        crate::evidence::Ev::GlyphIllustrative,
        "galaxy appearance, background galaxies",
    );
    for (gi, g) in u.galaxies.iter().enumerate() {
        // nearby satellites belong to the galaxy band's own fade window too
        let gw = if g.dist_kpc <= 500.0 {
            w
        } else {
            band(lw, 21.4, 24.9, 0.6)
        };
        if gw <= 0.02 {
            continue;
        }
        let r_px = ((g.size_kpc * 0.5 * KPC) / inp.cam.mpp) as f32;
        let px = proj.px(g.pos);
        if !on_screen(px, inp.w, inp.h, r_px.max(20.0) + 30.0) {
            continue;
        }
        draw_galaxy_glyph(inp, proj, gi, px, dl, out, gw);
    }
    // the big two sit in vast, faint halos; at this scale those are the structure
    let halo_w = band(lw, 22.0, 24.6, 0.6) as f32;
    if halo_w > 0.01 {
        let tint = [0.42, 0.52, 0.92, halo_w];
        push_galaxy(
            proj,
            dl,
            proj.px(u.gal.center),
            300.0 * KPC,
            None,
            3.0,
            0.21,
            tint,
            inp.cam.mpp,
        );
        if let Some(m31) = u.galaxies.iter().find(|g| g.name.contains("M31")) {
            let tint = [0.52, 0.50, 0.90, halo_w];
            push_galaxy(
                proj,
                dl,
                proj.px(m31.pos),
                300.0 * KPC,
                None,
                3.0,
                0.47,
                tint,
                inp.cam.mpp,
            );
        }
    }
    // The Milky Way itself, as a galaxy like the others, once the map's own
    // illustration has shrunk away. Same tilt the real one has in this frame.
    let mw_w = band(lw, 22.0, 25.0, 0.6);
    let mw_px = proj.px(u.gal.center);
    if mw_w > 0.02 && on_screen(mw_px, inp.w, inp.h, 200.0) {
        let radius = 15.0 * KPC;
        let tint = [0.62, 0.74, 1.0, 1.9 * mw_w as f32];
        push_galaxy(
            proj,
            dl,
            mw_px,
            radius,
            Some((u.gal.x, u.gal.y)),
            0.0,
            0.42,
            tint,
            inp.cam.mpp,
        );
        let r_px = (radius / inp.cam.mpp) as f32;
        if r_px < 2.5 {
            dl.point(mw_px, 1.0, 2.2, 0.0, [0.9, 0.9, 1.0, 1.2 * mw_w as f32]);
        }
        out.labels.push(Label {
            pos: [mw_px[0], mw_px[1] - r_px.max(6.0) - 12.0],
            text: "Milky Way (you are here)".into(),
            size: 12.5,
            color: theme::rgba(theme::INK_GOLD, 0.95 * mw_w as f32),
            priority: 48,
            obj: None,
        });
        let _ = out;
    }
}

// ------------------------------------------------------------ cosmic layer

fn cosmic_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, truth) = (inp.u, inp.modes.truth);
    let w = band(lw, 23.0, 26.1, 0.6);
    if w <= 0.02 {
        return;
    }
    seen(out, crate::evidence::Ev::CompiledCatalog, "cluster catalog");
    seen(out, crate::evidence::Ev::WebProcedural, "filament field");
    for (ni, n) in u.nodes.iter().enumerate() {
        let px = proj.px(n.pos);
        let r_px = ((n.size_mpc * 0.5 * MPC) / inp.cam.mpp) as f32;
        if !on_screen(px, inp.w, inp.h, r_px + 40.0) {
            continue;
        }
        let c = ink([1.0, 0.86, 0.66], 1.6 * w as f32, truth, Prov::Measured);
        // the catalog position, plus a swarm that reads as a cluster's galaxies
        // (the members themselves are illustrative)
        dl.point(px, 1.4, r_px.clamp(2.0, 18.0) * 0.5, 0.0, c);
        let spread = r_px.max(5.0) as f64;
        for k in 0..36 {
            let ang = seeded(k, ni * 7 + 3) * std::f64::consts::TAU;
            let rad = spread * (seeded(k, ni * 7 + 4)).powf(1.4) * 1.3;
            let p = [
                px[0] + (rad * ang.cos()) as f32,
                px[1] + (rad * ang.sin()) as f32,
            ];
            let a = (0.35 + seeded(k, ni * 7 + 5) as f32) * 0.8 * w as f32;
            dl.point(
                p,
                0.6,
                1.0,
                0.0,
                ink([1.0, 0.88, 0.72], a, truth, Prov::Illustrative),
            );
        }
        out.picks.push(Pickable {
            pos: px,
            r: r_px.max(11.0),
            obj: ObjectRef::Node(ni),
        });
        out.labels.push(Label {
            pos: [px[0], px[1] - r_px.max(4.0) - 11.0],
            text: n.name.into(),
            size: 12.0,
            color: theme::rgba(theme::INK_WHITE, 0.85 * w as f32),
            priority: 12,
            obj: Some(ObjectRef::Node(ni)),
        });
    }
    // You are here, at the scale of everything
    let sun_px = proj.px(DVec3::ZERO);
    if lw > 23.7 && on_screen(sun_px, inp.w, inp.h, 20.0) {
        dl.circle(
            sun_px,
            6.0,
            StrokeStyle::new(theme::rgba(theme::INK_GOLD, 0.95), 1.1),
        );
        out.labels.push(Label {
            pos: [sun_px[0], sun_px[1] - 16.0],
            text: "You are here".into(),
            size: 12.5,
            color: theme::rgba(theme::INK_GOLD, 0.95),
            priority: 60,
            obj: Some(ObjectRef::Sun),
        });
    }
}

// ------------------------------------------------------------- exo systems

fn exo_active_system(inp: &BuildIn) -> Option<usize> {
    let ei = inp.scene_exo?;
    let lw = inp.cam.log_width(inp.w);
    if lw > 15.2 {
        return None;
    }
    // only while the camera is actually near the host
    let d = (inp.cam.center_root() - inp.u.exos[ei].pos).length();
    if d > 0.5 * PC {
        return None;
    }
    Some(ei)
}

fn exo_layer(
    inp: &BuildIn,
    proj: &Proj,
    lw: f64,
    ei: usize,
    dl: &mut DrawList,
    out: &mut FrameOut,
) {
    let (u, cam, jd, truth) = (inp.u, inp.cam, inp.clock.jd, inp.modes.truth);
    let s = &u.exos[ei];
    let host = s.pos;
    // When the camera is attached to this host's frame, all
    // pocket geometry projects through frame-local coordinates — deep zoom
    // precision is AU-scale f64, never 1e17-scale
    let framed = (inp.cam.frame_origin - host).length() < 1.0;
    let to_px = |local: DVec3| -> [f32; 2] {
        if framed {
            proj.px_local(local)
        } else {
            proj.px(host + local)
        }
    };
    let local_z = |local: DVec3| -> f64 {
        if framed {
            proj.screen_local_z(local)
        } else {
            proj.screen(host + local)[2]
        }
    };
    let host_px = to_px(DVec3::ZERO);
    let w = band(lw, 6.0, 15.2, 0.5);
    seen(out, crate::evidence::Ev::ExoArchive, "system parameters");
    seen(out, crate::evidence::Ev::ExoGeometry, "orbit orientation");
    seen(out, crate::evidence::Ev::ExoPhase, "orbital phases");

    // schematic radius remapping for System View
    let a_list: Vec<Option<(f64, Prov)>> = s.planets.iter().map(|p| s.a_au_of(p)).collect();
    let a_min = a_list
        .iter()
        .flatten()
        .map(|(a, _)| *a)
        .fold(f64::MAX, f64::min);
    let remap = |a: f64| -> f64 {
        if !inp.modes.system_view || a_min == f64::MAX {
            return a;
        }
        let base = a_min.max(1e-3);
        base * (1.0 + 1.35 * (a / base).ln().max(0.0))
    };

    // habitable zone (inferred from stellar luminosity)
    if let (Some(teff), Some(rad)) = (s.st_teff, s.st_rad) {
        let lum = rad * rad * (teff / 5772.0).powi(4);
        let (ri, ro) = ((lum / 1.1f64).sqrt(), (lum / 0.53f64).sqrt());
        for r_au in [ri, ro] {
            let r_px = (remap(r_au) * AU / cam.mpp) as f32;
            if r_px > 10.0 && r_px < 30_000.0 {
                let c = ink([0.35, 0.75, 0.55], 0.30 * w as f32, truth, Prov::Inferred);
                dl.circle(host_px, r_px, StrokeStyle::new(c, 0.9).dashed(7.0, 0.5));
            }
        }
        let mid_px = (remap((ri + ro) * 0.5) * AU / cam.mpp) as f32;
        if mid_px > 60.0 && mid_px < 12_000.0 {
            out.labels.push(Label {
                pos: [host_px[0], host_px[1] - mid_px],
                text: "habitable zone (inferred)".into(),
                size: 10.5,
                color: theme::rgba([0.4, 0.78, 0.58], 0.55),
                priority: 4,
                obj: None,
            });
        }
    }

    // host star — a measured radius earns a physical disc; an unknown radius
    // gets an explicitly schematic glyph (never silently render
    // a default 1 R☉ as if measured).
    let rgb = s
        .st_teff
        .map(crate::data::teff_rgb)
        .unwrap_or([1.0, 0.85, 0.6]);
    let host_z = zmap(local_z(DVec3::ZERO), cam.mpp * inp.w);
    let r_px = match s.st_rad {
        Some(rad) => {
            let r_px = (rad * R_SUN_M / cam.mpp) as f32;
            if r_px > 2.0 {
                let (bx, by, bz) = view_axes(proj, frame_about(DVec3::new(0.0, 0.34, 0.94)));
                let mut d = DiscInst::plain(
                    host_px,
                    [r_px, 0.0, KIND_STAR, 0.4 + ei as f32 * 0.013],
                    host_z,
                );
                d.sun = [0.0, 0.0, 1.0, 1.0];
                d.axis = [bz[0], bz[1], bz[2], (inp.anim_t * 0.02) as f32];
                d.bx = [bx[0], bx[1], bx[2], 0.0];
                d.by = [by[0], by[1], by[2], 0.0];
                d.ca = [rgb[0], rgb[1], rgb[2], 1.0];
                d.cb = [rgb[0] * 0.9, rgb[1] * 0.65, rgb[2] * 0.5, 1.0];
                d.flags = [
                    0.0,
                    0.0,
                    0.0,
                    1.45 + 6.5 * (1.0 - smoothstep(6.0, 150.0, r_px as f64)) as f32,
                ];
                dl.disc(d, host_z, [None, None, None]);
            }
            if r_px < 90.0 {
                let a = 3.0 * (1.0 - smoothstep(4.0, 90.0, r_px as f64)) as f32;
                dl.point(
                    host_px,
                    1.8f32.max(r_px * 0.8),
                    13.0,
                    0.25,
                    [rgb[0], rgb[1], rgb[2], a],
                );
            }
            r_px
        }
        None => {
            dl.point(host_px, 2.0, 13.0, 0.25, [rgb[0], rgb[1], rgb[2], 2.6]);
            let c = ink(Prov::Unknown.rgb(), 0.7, true, Prov::Unknown);
            dl.circle(host_px, 11.0, StrokeStyle::new(c, 1.0).dashed(4.0, 0.55));
            11.0
        }
    };
    out.picks.push(Pickable {
        pos: host_px,
        r: r_px.max(10.0),
        obj: ObjectRef::ExoHost(ei),
    });
    out.labels.push(Label {
        pos: [host_px[0], host_px[1] - r_px.max(8.0) - 12.0],
        text: format!(
            "{}{}",
            s.host,
            if s.st_rad.is_none() {
                " (radius unknown)"
            } else {
                ""
            }
        ),
        size: 13.0,
        color: theme::rgba(theme::INK_WHITE, 0.85),
        priority: 40,
        obj: Some(ObjectRef::ExoHost(ei)),
    });

    // planets — world-space conics; the marker rides the same curve as the
    // engraving.
    let (e1, e2) = exo_plane(u, ei);
    let mut prev_per: Option<f64> = None;
    let mut prev_a_px = 0.0f32;
    for (k, p) in s.planets.iter().enumerate() {
        let Some((a_au, a_prov)) = a_list[k] else {
            continue;
        };
        let a_draw = remap(a_au);
        let a_px = (a_draw * AU / cam.mpp) as f32;
        if !(4.0..=60_000.0).contains(&a_px) {
            continue;
        }
        let ecc = p.e.unwrap_or(0.0).clamp(0.0, 0.95);
        let a_m = a_draw * AU;
        let b_m = a_m * (1.0 - ecc * ecc).sqrt();
        let n = 140;
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let ea = i as f64 / n as f64 * std::f64::consts::TAU;
                to_px(e1 * (a_m * (ea.cos() - ecc)) + e2 * (b_m * ea.sin()))
            })
            .collect();
        let mut st = StrokeStyle::new(
            ink(
                theme::INK_GOLD,
                0.5 * w as f32,
                truth,
                if p.a_au.is_some() {
                    Prov::Measured
                } else {
                    a_prov
                },
            ),
            1.0,
        )
        .glow(1.3);
        if p.e.is_none() {
            st = st.dashed(10.0, 0.62);
        }
        dl.polyline(&pts, true, st);

        // marker: same conic, schematic-scaled when System View is active
        let rel = exo_planet_rel(u, ei, k, jd).unwrap_or(DVec3::ZERO) * (a_draw / a_au);
        let ppx = to_px(rel);
        let pr_px = p
            .rade
            .map(|r| (r * R_EARTH_M / cam.mpp) as f32)
            .unwrap_or(0.0);
        let rocky = p.rade.map(|r| r < 1.8).unwrap_or(true);
        if pr_px > 1.8 {
            let sun_dir = view_dir(proj, (-rel).normalize_or_zero());
            let zd = zmap(local_z(rel), cam.mpp * inp.w);
            let (bx, by, bz) = view_axes(proj, frame_about(DVec3::new(0.1, 0.37, 0.92)));
            let mut d = DiscInst::plain(
                ppx,
                [
                    pr_px,
                    0.0,
                    if rocky { 1.0 } else { 2.0 },
                    0.29 + k as f32 * 0.047,
                ],
                zd,
            );
            d.sun = [sun_dir[0], sun_dir[1], sun_dir[2], 0.06];
            d.axis = [
                bz[0],
                bz[1],
                bz[2],
                ((jd - J2000) * 0.3 % std::f64::consts::TAU) as f32,
            ];
            d.bx = [bx[0], bx[1], bx[2], 0.0];
            d.by = [by[0], by[1], by[2], 0.0];
            d.ca = if rocky {
                [0.72, 0.58, 0.45, 0.0]
            } else {
                [0.62, 0.72, 0.85, 0.0]
            };
            d.cb = if rocky {
                [0.45, 0.36, 0.3, 1.0]
            } else {
                [0.4, 0.5, 0.68, 1.0]
            };
            d.rim = [0.6, 0.75, 1.0, if rocky { 0.2 } else { 0.5 }];
            d.fx[0] = 0.02;
            dl.disc(d, zd, [None, None, None]);
        } else {
            let c = if p.rade.is_none() && truth {
                theme::rgba(Prov::Unknown.rgb(), 0.9)
            } else {
                theme::rgba([0.75, 0.82, 0.95], 0.9)
            };
            dl.point(ppx, 2.4, 6.0, 0.0, c);
        }
        out.picks.push(Pickable {
            pos: ppx,
            r: pr_px.max(8.0),
            obj: ObjectRef::ExoPlanet(ei, k),
        });
        if a_px > 30.0 {
            let short = p.name.rsplit(' ').next().unwrap_or(&p.name);
            out.labels.push(Label {
                pos: [ppx[0], ppx[1] - pr_px.max(3.0) - 8.0],
                text: short.into(),
                size: 11.0,
                color: theme::rgba(theme::INK_WHITE, 0.7),
                priority: 8,
                obj: Some(ObjectRef::ExoPlanet(ei, k)),
            });
        }
        // resonance chains
        if let (Some(p1), Some(p0)) = (p.per_d, prev_per) {
            let ratio = p1 / p0;
            for (num, den) in [(2u32, 1u32), (3, 2), (4, 3), (5, 4), (5, 3), (3, 1)] {
                let ideal = num as f64 / den as f64;
                if (ratio / ideal - 1.0).abs() < 0.02 && a_px > 40.0 {
                    let mid = (a_px + prev_a_px) * 0.5;
                    out.labels.push(Label {
                        pos: [
                            host_px[0] + mid * std::f32::consts::FRAC_1_SQRT_2,
                            host_px[1] + mid * std::f32::consts::FRAC_1_SQRT_2,
                        ],
                        // near-commensurability of periods, not a proven resonance
                        text: format!("≈ {num}:{den}"),
                        size: 10.5,
                        color: theme::rgba([0.55, 0.9, 0.75], 0.8),
                        priority: 6,
                        obj: None,
                    });
                    break;
                }
            }
        }
        prev_per = p.per_d;
        prev_a_px = a_px;
    }
    if inp.modes.system_view {
        out.labels.push(Label {
            pos: [(inp.w * 0.5) as f32, (inp.h - 76.0) as f32],
            text: "System view: orbit spacing is logarithmic and schematic".into(),
            size: 12.0,
            color: theme::rgba(theme::INK_GOLD, 0.75),
            priority: 90,
            obj: None,
        });
    }
}

// ------------------------------------------------------- focused star scene

/// Local scene for a plain catalog star: Gaia gives no radius, so the glyph is
/// explicitly schematic — a luminous point in a dashed unknown-size ring.
fn focused_star_layer(
    inp: &BuildIn,
    proj: &Proj,
    lw: f64,
    si: usize,
    dl: &mut DrawList,
    out: &mut FrameOut,
) {
    if lw > 15.6 {
        return; // the global star field still covers this scale
    }
    let s = &inp.u.stars[si];
    let pos = s.pos_at(inp.clock.jd);
    let px = proj.px(pos);
    if !on_screen(px, inp.w, inp.h, 60.0) {
        return;
    }
    let rgb = crate::data::star_rgb(s.bp_rp);
    dl.point(px, 3.2, 34.0, 0.8, [rgb[0], rgb[1], rgb[2], 0.95]);
    let c = ink(Prov::Unknown.rgb(), 0.6, true, Prov::Unknown);
    dl.circle(px, 13.0, StrokeStyle::new(c, 1.0).dashed(4.0, 0.55));
    out.picks.push(Pickable {
        pos: px,
        r: 13.0,
        obj: ObjectRef::Star(si),
    });
    out.labels.push(Label {
        pos: [px[0], px[1] - 24.0],
        text: format!(
            "{} — schematic (no measured radius)",
            obj_name(inp.u, ObjectRef::Star(si))
        ),
        size: 11.5,
        color: theme::rgba(theme::INK_WHITE, 0.75),
        priority: 35,
        obj: None,
    });
}

// ------------------------------------------------------------ gravity view

fn gravity_layer(inp: &BuildIn, proj: &Proj, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, jd, truth) = (inp.u, inp.clock.jd, inp.modes.truth);
    // choose the pair: focused moon → (planet, moon); focused planet → (Sun, planet)
    // primary position and GM, secondary position and GM, separation
    type Pair = (DVec3, f64, DVec3, f64, f64);
    let pair: Option<Pair> = match inp.focus {
        Some(ObjectRef::Moon(mi)) => {
            let m = &MOONS[mi];
            let pp = obj_pos(u, ObjectRef::Planet(m.planet), jd, inp.island);
            let mp = obj_pos(u, ObjectRef::Moon(mi), jd, inp.island);
            // authoritative satellite GM
            Some((pp, PLANETS[m.planet].gm, mp, sim::MOON_GM[mi], m.a_km * 1e3))
        }
        Some(ObjectRef::Planet(i)) => {
            let pp = obj_pos(u, ObjectRef::Planet(i), jd, inp.island);
            Some((DVec3::ZERO, GM_SUN, pp, PLANETS[i].gm, pp.length()))
        }
        _ => None,
    };
    let Some((pa, gm1, pb, gm2, a_m)) = pair else {
        return;
    };
    seen(out, crate::evidence::Ev::GravityModel, "gravity view");
    let apx = proj.px(pa);
    let bpx = proj.px(pb);
    let sep = ((bpx[0] - apx[0]).powi(2) + (bpx[1] - apx[1]).powi(2)).sqrt();
    if !(40.0..30_000.0).contains(&sep) {
        return;
    }
    let mu = gm2 / (gm1 + gm2);
    out.uni.grav_a = [apx[0], apx[1], mu as f32, sep];
    out.uni.grav_b = [bpx[0], bpx[1], 1.0, 4.0];

    // Lagrange point markers in the true 3D orbital plane
    // (the plane basis comes from the pair's relative angular momentum, then
    // projects — correct under any camera tilt).
    let sep_m = (pb - pa).length().max(1.0);
    let e1w = (pb - pa) / sep_m;
    let dtd = 0.01; // days, for the relative velocity direction
    let obj_b = inp.focus.unwrap_or(ObjectRef::Sun);
    let vrel = (obj_pos(u, obj_b, jd + dtd, inp.island) - obj_pos(u, obj_b, jd - dtd, inp.island))
        / (2.0 * dtd);
    let mut nrm = e1w.cross(vrel.normalize_or_zero());
    if nrm.length_squared() < 1e-6 {
        nrm = DVec3::Z;
    }
    let nrm = nrm.normalize();
    let e2w = nrm.cross(e1w);
    for (k, l) in lagrange_points(mu).iter().enumerate() {
        let world = pa + e1w * ((l[0] + mu) * sep_m) + e2w * (l[1] * sep_m);
        let px = proj.px(world);
        let c = ink([0.9, 0.75, 0.4], 0.9, truth, Prov::Modeled);
        dl.circle(px, 5.0, StrokeStyle::new(c, 1.0));
        dl.point(px, 1.2, 3.0, 0.0, c);
        out.labels.push(Label {
            pos: [px[0], px[1] - 12.0],
            text: format!("L{}", k + 1),
            size: 11.0,
            color: theme::rgba([0.9, 0.75, 0.4], 0.85),
            priority: 25,
            obj: None,
        });
    }
    // Hill sphere of the secondary
    let rh = hill_radius(a_m, gm1, gm2);
    let rh_px = (rh / inp.cam.mpp) as f32;
    if rh_px > 12.0 && rh_px < 20_000.0 {
        let c = ink([0.5, 0.8, 0.95], 0.5, truth, Prov::Modeled);
        dl.circle(bpx, rh_px, StrokeStyle::new(c, 0.9).dashed(8.0, 0.55));
        out.labels.push(Label {
            pos: [bpx[0] + rh_px * 0.71, bpx[1] - rh_px * 0.71],
            text: "Hill sphere".into(),
            size: 10.5,
            color: theme::rgba([0.5, 0.8, 0.95], 0.7),
            priority: 9,
            obj: None,
        });
    }
    // Roche limit around the primary body of the pair (for planet-moon pairs)
    if let Some(ObjectRef::Moon(_)) = inp.focus {
        let (pr, pgm) = match inp.focus {
            Some(ObjectRef::Moon(mi)) => {
                let pl = &PLANETS[MOONS[mi].planet];
                (pl.radius_km * 1e3, pl.gm)
            }
            _ => (0.0, 1.0),
        };
        let rho_p = pgm / 6.674e-11 / ((4.0 / 3.0) * std::f64::consts::PI * pr.powi(3));
        let roche = crate::sim::roche_limit(pr, rho_p, 1500.0);
        let r_px = (roche / inp.cam.mpp) as f32;
        if r_px > 10.0 && r_px < 10_000.0 {
            let c = ink([0.95, 0.5, 0.4], 0.45, truth, Prov::Modeled);
            dl.circle(apx, r_px, StrokeStyle::new(c, 0.9).dashed(4.0, 0.5));
            out.labels.push(Label {
                pos: [apx[0], apx[1] + r_px + 10.0],
                text: "Roche limit (fluid)".into(),
                size: 10.5,
                color: theme::rgba([0.95, 0.5, 0.4], 0.7),
                priority: 9,
                obj: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::galactic_frame;
    use std::cell::Cell;

    type Runs = Vec<Vec<(f64, f64, f64)>>;

    /// Run the tessellator over a curve, returning the runs and how often it was evaluated.
    fn tessellate(f: &dyn Fn(f64) -> DVec3) -> (Runs, u32) {
        let cam = Camera::new();
        let gal = galactic_frame();
        let proj = Proj::new(&cam, 1600.0, 900.0, &gal);
        let calls = Cell::new(0u32);
        let counted = |t: f64| {
            calls.set(calls.get() + 1);
            f(t)
        };
        let mut c = Curve {
            proj: &proj,
            f: &counted,
            x0: -8.0,
            y0: -8.0,
            x1: 1608.0,
            y1: 908.0,
            runs: Vec::new(),
            budget: CURVE_BUDGET,
        };
        let seeds = 64;
        let mut prev = (0.0, c.scr(0.0));
        for k in 1..=seeds {
            let t = k as f64 / seeds as f64;
            let p = c.scr(t);
            c.seg(prev.0, prev.1, t, p, 0);
            prev = (t, p);
        }
        (c.runs, calls.get())
    }

    #[test]
    fn a_curve_of_nan_costs_almost_nothing_and_draws_nothing() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let (runs, calls) = tessellate(&|_| DVec3::new(bad, bad, bad));
            assert!(runs.is_empty(), "{bad} drew something");
            assert!(calls < 100, "{bad} was evaluated {calls} times");
        }
    }

    #[test]
    fn a_curve_that_is_partly_nan_draws_its_good_part() {
        let cam_scale = Camera::new().mpp;
        let (runs, calls) = tessellate(&|t| {
            if (0.4..0.6).contains(&t) {
                DVec3::NAN
            } else {
                DVec3::new((t - 0.5) * 400.0 * cam_scale, 0.0, 0.0)
            }
        });
        assert!(!runs.is_empty());
        assert!(calls < 10_000, "evaluated {calls} times");
    }

    #[test]
    fn a_wild_finite_curve_stays_within_its_budget() {
        // every sample lands somewhere else on screen, so nothing ever looks flat enough
        let mpp = Camera::new().mpp;
        let (runs, calls) = tessellate(&|t| {
            let w = (t * 1e7).sin() * 1e4;
            DVec3::new(
                w.fract() * 800.0 * mpp,
                (w * 3.7).fract() * 450.0 * mpp,
                0.0,
            )
        });
        // each subdivision evaluates the curve at most twice more, and the 64 seed
        // spans each get one evaluation and two points of their own
        assert!(calls <= 2 * CURVE_BUDGET + 200, "evaluated {calls} times");
        let points: usize = runs.iter().map(Vec::len).sum();
        assert!(points <= 2 * CURVE_BUDGET as usize + 200, "{points} points");
    }

    /// Tessellate a circle of `radius_px` centred off to one side of the view.
    fn circle_runs(radius_px: f64, mpp: f64) -> (Runs, f64) {
        let mut cam = Camera::new();
        cam.mpp = mpp;
        let radius = radius_px * mpp;
        cam.center = DVec3::new(radius, 0.0, 0.0); // the curve passes mid-screen
        let gal = galactic_frame();
        let proj = Proj::new(&cam, 1600.0, 900.0, &gal);
        let f = move |t: f64| DVec3::new(radius * t.cos(), radius * t.sin(), 0.0);
        let mut c = Curve {
            proj: &proj,
            f: &f,
            x0: -8.0,
            y0: -8.0,
            x1: 1608.0,
            y1: 908.0,
            runs: Vec::new(),
            budget: CURVE_BUDGET,
        };
        let tau = std::f64::consts::TAU;
        let seeds = 64;
        let mut prev = (0.0, c.scr(0.0));
        for k in 1..=seeds {
            let t = tau * k as f64 / seeds as f64;
            let p = c.scr(t);
            c.seg(prev.0, prev.1, t, p, 0);
            prev = (t, p);
        }
        (c.runs, radius_px)
    }

    #[test]
    fn huge_orbits_stay_smooth_and_inside_the_view() {
        // an Earth-sized orbit seen from close by: radius of a billion pixels
        for radius_px in [500.0, 5.0e4, 1.0e7, 1.0e9] {
            let (runs, r) = circle_runs(radius_px, 1.0e3);
            let n: usize = runs.iter().map(|x| x.len()).sum();
            assert!(n >= 2, "the curve crosses the view at radius {radius_px}");
            assert!(n < 6000, "too many segments ({n}) at radius {radius_px}");
            for run in &runs {
                for w in run.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    // inside the clip rectangle, where f32 keeps sub-pixel accuracy
                    for p in [a, b] {
                        assert!(
                            p.0 >= -8.01 && p.0 <= 1608.01 && p.1 >= -8.01 && p.1 <= 908.01,
                            "{p:?}"
                        );
                    }
                    // chord error against the true circle, whose centre is at x = 800 - r
                    let mid = (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
                    let d = ((mid.0 - (800.0 - r)).powi(2) + (mid.1 - 450.0).powi(2)).sqrt();
                    assert!(
                        (d - r).abs() < 0.5,
                        "chord error {} at radius {radius_px}",
                        (d - r).abs()
                    );
                }
            }
        }
    }

    #[test]
    fn a_curve_entirely_off_screen_emits_nothing() {
        let mut cam = Camera::new();
        cam.mpp = 1.0;
        let gal = galactic_frame();
        let proj = Proj::new(&cam, 1600.0, 900.0, &gal);
        let far = 1.0e7;
        let f = move |t: f64| DVec3::new(far + 100.0 * t.cos(), far + 100.0 * t.sin(), 0.0);
        let mut c = Curve {
            proj: &proj,
            f: &f,
            x0: -8.0,
            y0: -8.0,
            x1: 1608.0,
            y1: 908.0,
            runs: Vec::new(),
            budget: CURVE_BUDGET,
        };
        let (p0, p1) = (c.scr(0.0), c.scr(3.0));
        c.seg(0.0, p0, 3.0, p1, 0);
        assert!(c.runs.is_empty());
    }

    #[test]
    fn galaxy_plane_axes_are_orthonormal_and_edge_on_at_ninety_degrees() {
        let pos = DVec3::new(1.0e22, 3.0e22, -2.0e22);
        let (a, b) = galaxy_plane(pos, 37.7, 77.5);
        assert!((a.length() - 1.0).abs() < 1e-9 && (b.length() - 1.0).abs() < 1e-9);
        assert!(a.dot(b).abs() < 1e-9);
        // face-on: the disc lies across the line of sight
        let (a0, b0) = galaxy_plane(pos, 0.0, 0.0);
        let s = pos.normalize();
        assert!(a0.dot(s).abs() < 1e-9 && b0.dot(s).abs() < 1e-9);
        // edge-on: one in-plane axis runs along the line of sight
        let (_, b90) = galaxy_plane(pos, 0.0, 90.0);
        assert!((b90.dot(s).abs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn iau_sun_frame_is_right_handed() {
        let m = sun_basis(J2000 + 1234.5);
        assert!((m.determinant() - 1.0).abs() < 1e-9);
        // about 7.25 degrees between the solar pole and the ecliptic pole
        let tilt = m.col(2).dot(DVec3::Z).acos().to_degrees();
        assert!((tilt - 7.25).abs() < 0.2, "solar axis tilt {tilt}");
    }
}
