//! The atlas: composes every scale band into one continuous zoomable scene.
//! Layers own log-scale visibility windows and cross-fade; geometry is
//! projected CPU-side in f64 and emitted as screen-space vector primitives.

use glam::{DMat3, DVec3};

use crate::data::{Prov, Universe};
use crate::engine::draw::{
    DiscInst, DrawList, StrokeStyle, KIND_STAR,
};
use crate::engine::draw::CanvasUniforms;
use crate::frames::{band, fmt_len, Camera, Proj, AU, KPC, MPC, PC};
use crate::sim::{
    self, hill_radius, lagrange_points, moon_orbit, moon_rel, planet_axis, planet_orbit,
    planet_pos, Clock, NBody, GM_SUN, J2000, MOONS, PLANETS, RINGS,
    SUN_RADIUS_KM,
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
    /// initial scaled energy, for the drift diagnostic (AUDIT SIM-004)
    pub e0: f64,
}

pub struct Label {
    pub pos: [f32; 2],
    pub text: String,
    pub size: f32,
    pub color: [f32; 4],
    pub priority: i32,
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
    pub scale_text: String,
}

pub struct BuildIn<'a> {
    pub u: &'a Universe,
    pub cam: &'a Camera,
    pub clock: &'a Clock,
    pub modes: &'a Modes,
    pub focus: Option<ObjectRef>,
    /// Active exoplanet pocket scene — independent of selection (AUDIT
    /// FND-013: closing a card must not destroy the world around the camera).
    pub scene_exo: Option<usize>,
    pub island: Option<&'a Island>,
    pub w: f64,
    pub h: f64,
    pub anim_t: f64,
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

/// World-space offset of an exoplanet from its host (AUDIT FND-005: planets
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
    let (x, y) = (a * (ea.cos() - ecc), a * (1.0 - ecc * ecc).sqrt() * ea.sin());
    let (e1, e2) = exo_plane(u, ei);
    Some(e1 * x + e2 * y)
}

/// Position resolution now routes through the frame graph (ENTITY-001):
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
        ObjectRef::Sun => "Sol".into(),
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

/// Suggested view half-extent (m) when diving to an object.
pub fn obj_half_extent(u: &Universe, obj: ObjectRef) -> f64 {
    match obj {
        ObjectRef::Sun => R_SUN_M * 3.2,
        ObjectRef::Planet(i) => {
            let outer = MOONS
                .iter()
                .filter(|m| m.planet == i)
                .map(|m| m.a_km * 1e3)
                .fold(0.0f64, f64::max);
            if outer > 0.0 {
                outer * 1.35
            } else {
                PLANETS[i].radius_km * 1e3 * 6.0
            }
        }
        ObjectRef::Moon(mi) => MOONS[mi].radius_km * 1e3 * 5.0,
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
            s.planets[k].rade.map(|r| r * R_EARTH_M * 8.0).unwrap_or(0.05 * AU)
        }
        ObjectRef::Galaxy(i) => u.galaxies[i].size_kpc * KPC * 1.6,
        ObjectRef::Node(i) => u.nodes[i].size_mpc * MPC * 2.2,
    }
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

fn seeded(i: usize, k: usize) -> f64 {
    let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(k as u64 * 0x632B_E5AB);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x & 0xFFFF_FFFF) as f64 / 4_294_967_296.0
}

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
        scale_text: format!(
            "1 px = {}   ·   view ≈ {}",
            fmt_len(cam.mpp),
            fmt_len(cam.mpp * inp.w)
        ),
    };
    out.uni.size = [inp.w as f32, inp.h as f32, inp.anim_t as f32, 0.0];
    // deep field never fully dies (real sky is full of stars behind planets);
    // drift slowed 3× so zooming doesn't make the background crawl (UX B2/A1)
    out.uni.misc = [
        (1.0 - band(lw, -10.0, 8.6, 1.2)) as f32 * 0.55 + 0.45,
        (cam.mpp.log10() * 0.11) as f32,
        1.7,
        (band(lw, 18.8, 21.8, 0.6) * if truth { 0.4 } else { 1.0 }) as f32,
    ];

    // ---- fullscreen procedural planes
    let w_gal = band(lw, 18.6, 22.6, 0.9) * if truth { 0.35 } else { 1.0 };
    if w_gal > 0.001 {
        if let Some((ra, rb, inc)) =
            proj.plane_affine(u.gal.center, u.gal.x, u.gal.y, KPC)
        {
            let fade = crate::frames::smoothstep(0.08, 0.35, inc);
            out.uni.fog_a = [ra[0], ra[1], ra[2], (w_gal * fade) as f32];
            out.uni.fog_b = [rb[0], rb[1], rb[2], 1.0];
        }
    }
    let w_cos = band(lw, 22.9, 25.6, 0.9) * if truth { 0.30 } else { 1.0 };
    if w_cos > 0.001 {
        let (sx, sy, _sz) = crate::frames::supergalactic_basis(&u.gal);
        if let Some((ra, rb, inc)) = proj.plane_affine(DVec3::ZERO, sx, sy, 10.0 * MPC) {
            let fade = crate::frames::smoothstep(0.08, 0.35, inc);
            out.uni.cos_a = [ra[0], ra[1], ra[2], (w_cos * fade) as f32];
            out.uni.cos_b = [rb[0], rb[1], rb[2], 0.0];
        }
    }

    // ---- ruler rings: the physical log-scale ruler
    ruler_layer(inp, &proj, lw, dl, &mut out);

    // ---- exosystem pocket (worlds within worlds)
    let exo_active = exo_active_system(inp);
    if let Some(ei) = exo_active {
        exo_layer(inp, &proj, lw, ei, dl, &mut out);
        out.band_name = format!("Exoplanet System — {}", u.exos[ei].host);
    }

    // ---- solar system band
    if exo_active.is_none() {
        solar_layer(inp, &proj, lw, dl, &mut out);
        moons_layer(inp, &proj, lw, dl, &mut out);
        heliosphere_layer(inp, &proj, lw, dl, &mut out);
    }
    stars_layer(inp, &proj, lw, dl, &mut out);
    galaxy_layer(inp, &proj, lw, dl, &mut out);
    localgroup_layer(inp, &proj, lw, dl, &mut out);
    cosmic_layer(inp, &proj, lw, dl, &mut out);

    // ---- gravity view
    if inp.modes.gravity {
        gravity_layer(inp, &proj, dl, &mut out);
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
    // a void (AUDIT FND-012)
    if let Some(ObjectRef::Star(si)) = inp.focus {
        focused_star_layer(inp, &proj, lw, si, dl, &mut out);
    }

    // back-to-front for correct edge blending (depth buffer handles occlusion;
    // disc_z now stores mapped NDC depth, so far = larger = first)
    if dl.discs.len() == dl.disc_z.len() && dl.discs.len() > 1 {
        let mut idx: Vec<usize> = (0..dl.discs.len()).collect();
        idx.sort_by(|&a, &b| {
            dl.disc_z[b].partial_cmp(&dl.disc_z[a]).unwrap_or(std::cmp::Ordering::Equal)
        });
        dl.discs = idx.iter().map(|&i| dl.discs[i]).collect();
    }

    let jd_ok = (1800.0..=2050.0).contains(&jd_year(jd));
    if !jd_ok {
        out.labels.push(Label {
            pos: [(inp.w / 2.0) as f32, 26.0],
            text: "⚠ beyond 1800–2050: planetary elements extrapolated".into(),
            size: 13.0,
            color: theme::rgba(Prov::Inferred.rgb(), 0.9),
            priority: 100,
        });
    }
    let _ = jd;
    out
}

fn jd_year(jd: f64) -> f64 {
    2000.0 + (jd - J2000) / 365.25
}

fn band_name(lw: f64) -> &'static str {
    match lw {
        x if x < 7.5 => "Planet Close-up",
        x if x < 8.9 => "Planet",
        x if x < 10.8 => "Planet & Moons",
        x if x < 12.3 => "Inner System",
        x if x < 13.5 => "Outer System",
        x if x < 15.0 => "Heliosphere",
        x if x < 16.5 => "Oort Cloud",
        x if x < 18.0 => "Solar Neighbourhood",
        x if x < 19.5 => "Interstellar Deep",
        x if x < 21.8 => "The Galaxy",
        x if x < 23.3 => "Local Group",
        _ => "Cosmic Web",
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
    let (unit_m, unit_name) = if view_w < 0.03 * AU {
        (1e3, "km")
    } else if view_w < 0.6 * crate::frames::LY {
        (AU, "AU")
    } else if view_w < 0.5 * MPC {
        (PC, "pc")
    } else {
        (MPC, "Mpc")
    };
    let target = view_w * 0.30 / unit_m;
    let k = target.log10().floor();
    let base = 10f64.powf(k);
    let nice = [1.0, 2.0, 5.0]
        .iter()
        .map(|m| m * base)
        .min_by(|a, b| {
            (a - target).abs().partial_cmp(&(b - target).abs()).unwrap()
        })
        .unwrap();
    let c = proj.px(anchor);
    for mult in [1.0, 4.0, 0.25] {
        let r_u = nice * mult;
        let r_px = (r_u * unit_m / inp.cam.mpp) as f32;
        if !(34.0..(inp.w as f32 * 1.25)).contains(&r_px) {
            continue;
        }
        let a = if mult == 1.0 { 0.17 } else { 0.09 };
        dl.circle(c, r_px, StrokeStyle::new(theme::rgba(theme::INK_FAINT, a), 1.0));
        let ang = 0.62f32;
        let txt = if r_u >= 1.0 {
            format!("{} {unit_name}", r_u.round())
        } else {
            format!("{r_u} {unit_name}")
        };
        out.labels.push(Label {
            pos: [c[0] + r_px * ang.cos(), c[1] - r_px * ang.sin()],
            text: txt,
            size: 11.0,
            color: theme::rgba(theme::INK_FAINT, 0.75),
            priority: -5,
        });
    }
}

// ------------------------------------------------------------ solar layer

fn solar_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, cam, jd, truth) = (inp.u, inp.cam, inp.clock.jd, inp.modes.truth);
    let w_solar = band(lw, 6.0, 17.4, 0.7);
    if w_solar <= 0.001 {
        return;
    }
    let jd_ok = (1800.0..=2050.0).contains(&jd_year(jd));
    let orbit_prov = if jd_ok { Prov::Modeled } else { Prov::Inferred };

    // Sun
    let sun_px = proj.px(DVec3::ZERO);
    let sun_r_px = (R_SUN_M / cam.mpp) as f32;
    if on_screen(sun_px, inp.w, inp.h, sun_r_px + 400.0) {
        if sun_r_px > 2.5 {
            let zd = zmap(proj.screen(DVec3::ZERO)[2], cam.mpp * inp.w);
            dl.disc_z.push(zd);
            dl.discs.push(DiscInst {
                center: sun_px,
                pr: [sun_r_px, 0.0, KIND_STAR, 0.13],
                sun: [0.0, 0.0, 1.0, 1.0],
                axis: [0.0, 0.94, 0.34, (inp.anim_t * 0.02) as f32],
                ca: [1.0, 0.82, 0.55, 1.0],
                cb: [1.0, 0.6, 0.3, 1.0],
                rim: [1.0, 0.7, 0.35, 0.6],
                depth: zd,
            });
            dl.point_z(sun_px, sun_r_px * 0.7, sun_r_px * 2.6, 0.35, [1.0, 0.85, 0.6, 0.5], zd - 0.001);
        } else {
            let a = (w_solar * 0.95) as f32;
            dl.point(sun_px, 2.6, 26.0, 0.8, [1.0, 0.88, 0.62, a]);
        }
        out.picks.push(Pickable { pos: sun_px, r: sun_r_px.max(9.0), obj: ObjectRef::Sun });
        if lw > 10.5 {
            out.labels.push(Label {
                pos: [sun_px[0], sun_px[1] - sun_r_px.max(8.0) - 10.0],
                text: "Sol".into(),
                size: 13.0,
                color: theme::rgba([1.0, 0.88, 0.62], 0.8),
                priority: 30,
            });
        }
    }

    for i in 0..PLANETS.len() {
        let p = &PLANETS[i];
        let pos = obj_pos(u, ObjectRef::Planet(i), jd, inp.island);
        let a_px = (p.el[0] * AU / cam.mpp) as f32;
        let px = proj.px(pos);
        // orbit engraving, motion-graded: brightest at the planet, fading
        // around the arc behind it — the ink itself shows direction of travel
        if a_px > 7.0 && a_px < 60_000.0 {
            let fade = (1.0 - (7.0 / a_px)).min(1.0) * ((3_000.0 / a_px).min(1.0)).max(0.12);
            let n = if a_px > 600.0 { 340 } else { 180 };
            let pts = project_poly(proj, &planet_orbit(i, jd, n));
            let mut st = StrokeStyle::new(
                ink(theme::INK_GOLD, 0.5 * fade * w_solar as f32, truth, orbit_prov),
                1.0,
            )
            .glow(1.4);
            if !jd_ok {
                st = st.dashed(14.0, 0.55);
            }
            let head = pts
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    let da = (a.1[0] - px[0]).powi(2) + (a.1[1] - px[1]).powi(2);
                    let db = (b.1[0] - px[0]).powi(2) + (b.1[1] - px[1]).powi(2);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(k, _)| k)
                .unwrap_or(0);
            let nn = pts.len();
            let alphas: Vec<f32> = (0..nn)
                .map(|k| {
                    let behind = ((head + nn - k) % nn) as f32 / nn as f32;
                    0.22 + 0.78 * (1.0 - behind) * (1.0 - behind)
                })
                .collect();
            dl.polyline_alpha(&pts, true, st, &alphas);
        }
        // ephemeris truth trail (measured arc from Horizons cache) — drawn
        // only when the clock is inside the cached coverage (AUDIT SCI-017)
        if inp.modes.trails && lw < 13.8 {
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
        if !on_screen(px, inp.w, inp.h, 300.0) {
            continue;
        }
        let r_px = (p.radius_km * 1e3 / cam.mpp) as f32;
        let vis = (a_px > 5.0 || r_px > 0.5) && w_solar > 0.05;
        if !vis {
            continue;
        }
        if r_px > 1.7 {
            draw_planet_disc(inp, proj, i, pos, px, r_px, dl);
        } else {
            let s = (2.2 + 0.9 * (1.0 - (i as f32 / 9.0))).min(3.2);
            let c = ink(p.color, 0.95, truth, Prov::Modeled);
            let glow = if i < 6 { 7.0 } else { 5.0 };
            dl.point(px, s, glow, 0.0, c);
        }
        out.picks.push(Pickable { pos: px, r: r_px.max(8.0), obj: ObjectRef::Planet(i) });
        if a_px > 26.0 {
            out.labels.push(Label {
                pos: [px[0], px[1] - r_px.max(4.0) - 9.0],
                text: p.name.into(),
                size: 12.0,
                color: theme::rgba(theme::INK_WHITE, 0.72),
                priority: 20 - i as i32,
            });
        }
    }

    // spacecraft
    if lw > 10.0 {
        craft_layer(inp, proj, lw, dl, out);
    }
}

fn draw_planet_disc(
    inp: &BuildIn,
    proj: &Proj,
    i: usize,
    pos: DVec3,
    px: [f32; 2],
    r_px: f32,
    dl: &mut DrawList,
) {
    let p = &PLANETS[i];
    let jd = inp.clock.jd;
    let sun_dir = view_dir(proj, (-pos).normalize_or_zero());
    let axis = view_dir(proj, planet_axis(i, jd));
    let seed = match p.name {
        "Jupiter" => 0.72,
        "Saturn" => 0.31,
        _ => 0.11 + i as f32 * 0.07,
    };
    // per-body surface character (UX A4): ca.a = polar caps, cb.a = detail
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
    // rings: the far half renders under the disc, the near half above it
    for ring in RINGS.iter().filter(|r| r.planet == i) {
        if r_px > 5.0 {
            draw_rings(inp, proj, i, pos, ring, dl);
        }
    }
    let zd = zmap(proj.screen(pos)[2], inp.cam.mpp * inp.w);
    dl.disc_z.push(zd);
    dl.discs.push(DiscInst {
        center: px,
        pr: [r_px, p.flatten, p.kind, seed],
        sun: [sun_dir[0], sun_dir[1], sun_dir[2], 0.05],
        axis: [axis[0], axis[1], axis[2], sim::body_w(i, jd) as f32],
        ca: [p.color[0], p.color[1], p.color[2], cap],
        cb: [p.color2[0], p.color2[1], p.color2[2], det],
        rim: [
            p.rim[0],
            p.rim[1],
            p.rim[2],
            match p.name {
                "Earth" => 0.85, // HDR lets the atmosphere ring truly glow
                "Venus" => 0.7,
                _ if p.rim == [0.0, 0.0, 0.0] => 0.0,
                _ => 0.55,
            },
        ],
        depth: zd,
    });
    // mask the point→disc transition with a fading glow (UX B1)
    if r_px < 3.2 {
        let a = ((3.2 - r_px) / 2.0).clamp(0.0, 1.0) * 0.9;
        dl.point_z(px, r_px.max(1.6), 6.0, 0.0, [p.color[0], p.color[1], p.color[2], a], zd - 0.001);
    }
    // Earth: vector coastlines + graticule (measured, Natural Earth)
    if p.kind == 4.0 && r_px > 26.0 {
        earth_vector_layer(inp, proj, pos, r_px, dl);
    }
}

fn draw_rings(
    inp: &BuildIn,
    proj: &Proj,
    pi: usize,
    pos: DVec3,
    ring: &sim::RingDef,
    dl: &mut DrawList,
) {
    let eq = sim::body_equator(pi, inp.clock.jd);
    let truth = inp.modes.truth;
    for (r0, r1, dens) in ring.zones {
        let width_px = ((r1 - r0) * 1e3 / inp.cam.mpp) as f32;
        let n = (width_px / 2.6).clamp(2.0, 22.0) as usize;
        for k in 0..n {
            let f = (k as f64 + 0.5) / n as f64;
            let r_m = (r0 + (r1 - r0) * f) * 1e3;
            let fade = (1.0 - (f - 0.5).abs() as f32 * 0.9) * dens;
            let c = ink([0.85, 0.78, 0.6], 0.34 * fade, truth, Prov::Modeled);
            // split the circle into near/far runs so the near half renders in
            // front of the planet disc when tilted (AUDIT SCI-031)
            let mut run: Vec<[f32; 2]> = Vec::new();
            let mut run_front = false;
            let mut flush = |dl: &mut DrawList, run: &mut Vec<[f32; 2]>, front: bool| {
                if run.len() > 1 {
                    dl.set_over(front);
                    dl.polyline(run, false, StrokeStyle::new(c, 1.0));
                    dl.set_over(false);
                }
                run.clear();
            };
            for s in 0..=160 {
                let a = s as f64 / 160.0 * std::f64::consts::TAU;
                let local = eq * DVec3::new(r_m * a.cos(), r_m * a.sin(), 0.0);
                let front = local.dot(proj.out) > 0.0;
                if front != run_front && !run.is_empty() {
                    flush(dl, &mut run, run_front);
                }
                run_front = front;
                run.push(proj.px(pos + local));
            }
            flush(dl, &mut run, run_front);
        }
    }
}

fn earth_vector_layer(inp: &BuildIn, proj: &Proj, pos: DVec3, r_px: f32, dl: &mut DrawList) {
    dl.set_over(true);
    let jd = inp.clock.jd;
    let truth = inp.modes.truth;
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
    let c = ink([0.55, 0.85, 0.62], 0.85, truth, Prov::Measured);
    for line in &inp.u.coast {
        let mut run: Vec<[f32; 2]> = Vec::new();
        for pt in line {
            let w = to_world(pt[0] as f64, pt[1] as f64);
            if front(w) {
                run.push(proj.px(w));
            } else if run.len() > 1 {
                dl.polyline(&run, false, StrokeStyle::new(c, 1.1));
                run.clear();
            } else {
                run.clear();
            }
        }
        if run.len() > 1 {
            dl.polyline(&run, false, StrokeStyle::new(c, 1.1));
        }
    }
    // graticule
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
                    let st = StrokeStyle::new(g, if lat == 0.0 { 0.9 } else { 0.6 });
                    dl.polyline(&run, false, st);
                }
                run.clear();
            }
        }
        if run.len() > 1 {
            dl.polyline(&run, false, StrokeStyle::new(g, if lat == 0.0 { 0.9 } else { 0.6 }));
        }
    }
    dl.set_over(false);

    // S5: settlement lights on the night hemisphere (land mask measured,
    // placement illustrative), fading in across the terminator
    if r_px > 60.0 && !inp.u.night_lights.is_empty() {
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
            dl.point_z(proj.px(w), 0.85 * scale, 2.4 * scale, 0.0, [1.0, 0.78, 0.46, a], zd);
        }
    }
}

// ------------------------------------------------------------ moons layer

fn moons_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, cam, jd, truth) = (inp.u, inp.cam, inp.clock.jd, inp.modes.truth);
    if lw > 11.4 {
        return;
    }
    for mi in 0..MOONS.len() {
        let m = &MOONS[mi];
        let a_px = (m.a_km * 1e3 / cam.mpp) as f32;
        // the ORBIT is only worth drawing in a size window, but the BODY must
        // never be culled by its own orbit size (diving to a moon previously
        // showed empty space — UX audit, found via the luna shot)
        if a_px < 5.0 {
            continue;
        }
        let orbit_visible = a_px <= 30_000.0;
        let ppos = obj_pos(u, ObjectRef::Planet(m.planet), jd, inp.island);
        let ppx = proj.px(ppos);
        if !on_screen(ppx, inp.w, inp.h, a_px + 200.0) {
            continue;
        }
        let mpos = if let Some(isl) = inp.island {
            obj_pos(u, ObjectRef::Moon(mi), jd, Some(isl))
        } else {
            ppos + moon_rel(mi, jd)
        };
        let mpx = proj.px(mpos);
        // orbit, motion-graded toward the moon's position
        let is_luna = m.name == "Moon";
        if orbit_visible {
            let fade = ((a_px - 5.0) / 30.0).clamp(0.1, 1.0);
            let pts: Vec<[f32; 2]> =
                moon_orbit(mi, jd, 160).iter().map(|r| proj.px(ppos + *r)).collect();
            let head = pts
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    let da = (a.1[0] - mpx[0]).powi(2) + (a.1[1] - mpx[1]).powi(2);
                    let db = (b.1[0] - mpx[0]).powi(2) + (b.1[1] - mpx[1]).powi(2);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(k, _)| k)
                .unwrap_or(0);
            let nn = pts.len();
            let alphas: Vec<f32> = (0..nn)
                .map(|k| {
                    let behind = ((head + nn - k) % nn) as f32 / nn as f32;
                    0.25 + 0.75 * (1.0 - behind) * (1.0 - behind)
                })
                .collect();
            dl.polyline_alpha(
                &pts,
                true,
                StrokeStyle::new(ink(theme::INK_GOLD_DIM, 0.48 * fade, truth, Prov::Modeled), 0.9)
                    .glow(1.0),
                &alphas,
            );
        }
        // measured Moon arc from Horizons (geocentric) — only within coverage
        if is_luna && inp.modes.trails {
            if let Some(t) = &u.moon_truth {
                if t.covers(jd, 5.0) {
                    let pts: Vec<[f32; 2]> =
                        t.samples.iter().map(|(_, rel, _)| proj.px(ppos + *rel)).collect();
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
            let axis = view_dir(proj, planet_axis(m.planet, jd));
            let mdet = match m.name {
                "Moon" => 1.6,
                "Io" => 1.4,
                "Callisto" | "Mimas" => 1.3,
                "Europa" | "Enceladus" => 0.5,
                _ => 0.9,
            };
            let zd = zmap(proj.screen(mpos)[2], cam.mpp * inp.w);
            dl.disc_z.push(zd);
            dl.discs.push(DiscInst {
                center: mpx,
                pr: [r_px, 0.0, m.kind, 0.23 + mi as f32 * 0.041],
                sun: [sun_dir[0], sun_dir[1], sun_dir[2], 0.05],
                axis: [axis[0], axis[1], axis[2], (jd - J2000) as f32 * 0.2],
                ca: [m.color[0], m.color[1], m.color[2], 0.0],
                cb: [m.color2[0], m.color2[1], m.color2[2], mdet],
                rim: [0.8, 0.62, 0.35, if m.name == "Titan" { 0.7 } else { 0.0 }],
                depth: zd,
            });
            if r_px < 3.2 {
                let a = ((3.2 - r_px) / 2.0).clamp(0.0, 1.0) * 0.85;
                dl.point_z(mpx, r_px.max(1.4), 4.0, 0.0, [m.color[0], m.color[1], m.color[2], a], zd - 0.001);
            }
        } else {
            dl.point(mpx, 1.8, 4.0, 0.0, ink(m.color, 0.9, truth, Prov::Modeled));
        }
        out.picks.push(Pickable { pos: mpx, r: r_px.max(7.0), obj: ObjectRef::Moon(mi) });
        if a_px > 60.0 {
            out.labels.push(Label {
                pos: [mpx[0], mpx[1] - r_px.max(3.0) - 8.0],
                text: m.name.into(),
                size: 11.0,
                color: theme::rgba(theme::INK_WHITE, 0.62),
                priority: 8,
            });
        }
    }
}

// ------------------------------------------------------- heliosphere layer

fn heliosphere_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let truth = inp.modes.truth;
    let w = band(lw, 12.6, 16.9, 0.6);
    if w <= 0.01 {
        return;
    }
    let sun_px = proj.px(DVec3::ZERO);
    // Kuiper belt stipple 39..48 AU
    let kw = band(lw, 12.2, 14.9, 0.5);
    if kw > 0.01 {
        for k in 0..420 {
            let r = (39.0 + 9.0 * seeded(k, 1)) * AU;
            let th = seeded(k, 2) * std::f64::consts::TAU;
            let z = (seeded(k, 3) - 0.5) * 4.0 * AU;
            let p = proj.px(DVec3::new(r * th.cos(), r * th.sin(), z));
            if on_screen(p, inp.w, inp.h, 10.0) {
                dl.point(
                    p,
                    1.0,
                    2.0,
                    0.0,
                    ink([0.62, 0.68, 0.8], 0.4 * kw as f32, truth, Prov::Illustrative),
                );
            }
        }
    }
    // termination shock & heliopause engravings (illustrative)
    for (r_au, name, dash) in [(90.0, "termination shock", 9.0), (123.0, "heliopause", 14.0)] {
        let r_px = (r_au * AU / inp.cam.mpp) as f32;
        if r_px > 60.0 && r_px < 4_000.0 {
            let c = ink([0.5, 0.62, 0.85], 0.4 * w as f32, truth, Prov::Illustrative);
            dl.circle(sun_px, r_px, StrokeStyle::new(c, 1.0).dashed(dash, 0.55));
            out.labels.push(Label {
                pos: [sun_px[0] + r_px * 0.71, sun_px[1] - r_px * 0.71],
                text: name.into(),
                size: 11.0,
                color: theme::rgba([0.5, 0.62, 0.85], 0.6),
                priority: 4,
            });
        }
    }
    // Oort cloud shell stipple
    let ow = band(lw, 14.6, 17.0, 0.5);
    if ow > 0.01 {
        for k in 0..900 {
            let r = (2_000.0 + 78_000.0 * seeded(k, 5).powf(1.6)) * AU;
            let th = seeded(k, 6) * std::f64::consts::TAU;
            let ph = (seeded(k, 7) - 0.5) * std::f64::consts::PI;
            let p = proj.px(DVec3::new(
                r * ph.cos() * th.cos(),
                r * ph.cos() * th.sin(),
                r * ph.sin(),
            ));
            if on_screen(p, inp.w, inp.h, 8.0) {
                dl.point(
                    p,
                    0.9,
                    1.6,
                    0.0,
                    ink([0.55, 0.6, 0.75], 0.32 * ow as f32, truth, Prov::Illustrative),
                );
            }
        }
        if ow > 0.4 {
            out.labels.push(Label {
                pos: [(inp.w * 0.5) as f32, (inp.h * 0.16) as f32],
                text: "Oort cloud — illustrative".into(),
                size: 12.0,
                color: theme::rgba([0.55, 0.6, 0.78], 0.55),
                priority: 3,
            });
        }
    }
}

fn craft_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, jd, truth) = (inp.u, inp.clock.jd, inp.modes.truth);
    let w = band(lw, 10.0, 15.6, 0.6);
    if w <= 0.01 {
        return;
    }
    for (ci, c) in u.craft.iter().enumerate() {
        let (jd0, jd1) = c.track.span();
        if inp.modes.trails {
            let upto = jd.min(jd1);
            let all: Vec<[f32; 2]> = c
                .track
                .samples
                .iter()
                .filter(|(t, _, _)| *t <= upto)
                .map(|(_, p, _)| proj.px(*p))
                .collect();
            // point budget so dense trails (Parker) don't saturate (UX B5)
            let step = (all.len() / 380).max(1);
            let pts: Vec<[f32; 2]> = all.iter().step_by(step).cloned().collect();
            if pts.len() > 1 {
                let a = (0.5 * w) as f32;
                dl.polyline(
                    &pts,
                    false,
                    StrokeStyle::new(
                        ink(c.color, a, truth, Prov::Measured),
                        1.0,
                    )
                    .glow(1.3),
                );
            }
        }
        if jd >= jd0 {
            // AUDIT SCI-018: no marker outside trajectory coverage. Cassini
            // keeps a dimmed memorial at its final state (mission ended there).
            let past_horizon = jd > jd1;
            if past_horizon && c.name != "Cassini" {
                continue;
            }
            let pos = obj_pos(u, ObjectRef::Craft(ci), jd, None);
            let px = proj.px(pos);
            if on_screen(px, inp.w, inp.h, 40.0) {
                let ended = past_horizon && c.name == "Cassini";
                let a = if past_horizon { 0.4 } else { 0.95 };
                dl.point(px, 1.8, 6.0, 0.6, ink(c.color, a * w as f32, truth, Prov::Measured));
                out.picks.push(Pickable { pos: px, r: 8.0, obj: ObjectRef::Craft(ci) });
                out.labels.push(Label {
                    pos: [px[0] + 8.0, px[1] - 8.0],
                    text: if ended { format!("{} (ended 2017)", c.name) } else { c.name.into() },
                    size: 11.0,
                    color: theme::rgba(c.color, 0.8 * w as f32),
                    priority: 10,
                });
            }
        }
    }
}

// ------------------------------------------------------------- stars layer

fn stars_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, cam, truth) = (inp.u, inp.cam, inp.modes.truth);
    // lower edge covers the Oort→interstellar dead zone (UX A2)
    let w = band(lw, 14.9, 19.9, 0.7);
    if w <= 0.005 {
        return;
    }
    let margin = 24.0f32;
    for (si, s) in u.stars.iter().enumerate() {
        let pos = s.pos_at(inp.clock.jd); // one epoch-aware provider (FND-014)
        let px = proj.px(pos);
        if !on_screen(px, inp.w, inp.h, margin) {
            continue;
        }
        let m = s.absmag;
        let core = (2.5 - 0.17 * (m - 1.0)).clamp(0.55, 4.6);
        let bright = ((6.0 - m) * 0.09 + 0.28).clamp(0.14, 1.0) * w as f32;
        let rgb = crate::data::star_rgb(s.bp_rp);
        let c = if truth {
            let p = Prov::Measured.rgb();
            [p[0] * 0.6 + rgb[0] * 0.4, p[1] * 0.6 + rgb[1] * 0.4, p[2] * 0.6 + rgb[2] * 0.4, bright]
        } else {
            [rgb[0], rgb[1], rgb[2], bright]
        };
        let spike = if m < 0.6 { 0.55 } else { 0.0 };
        let glow = core * (2.2 + (2.0 - m * 0.2).clamp(0.0, 2.0));
        dl.point(px, core, glow, spike, c);
        if core > 1.4 {
            out.picks.push(Pickable { pos: px, r: core.max(6.0), obj: ObjectRef::Star(si) });
        }
        if let Some(name) = s.name {
            if lw > 16.2 {
                out.labels.push(Label {
                    pos: [px[0], px[1] - core - 8.0],
                    text: name.into(),
                    size: 11.5,
                    color: theme::rgba(theme::INK_WHITE, 0.6 * w as f32),
                    priority: (10.0 - m) as i32,
                });
            }
        }
    }
    // Sun as a star + label
    let sun_px = proj.px(DVec3::ZERO);
    if lw > 16.0 && on_screen(sun_px, inp.w, inp.h, 20.0) {
        out.labels.push(Label {
            pos: [sun_px[0], sun_px[1] - 12.0],
            text: "Sol — you are here".into(),
            size: 12.0,
            color: theme::rgba(theme::INK_GOLD, 0.8 * w as f32),
            priority: 40,
        });
        dl.circle(
            sun_px,
            8.0,
            StrokeStyle::new(theme::rgba(theme::INK_GOLD, 0.5 * w as f32), 1.0).dashed(4.0, 0.55),
        );
    }
    // exoplanet host portals — ranked, not a wall of identical rings (UX A7):
    // multi-planet systems first, then proximity to the view center
    let pw = band(lw, 15.6, 18.8, 0.6);
    if pw > 0.02 {
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
        for (k, (_, ei, px)) in cands.iter().take(130).enumerate() {
            let e = &u.exos[*ei];
            let fade = if k < 40 { 1.0 } else { 0.55 };
            let c = ink(theme::INK_GOLD, 0.5 * pw as f32 * fade, truth, Prov::Measured);
            dl.circle(*px, 7.5, StrokeStyle::new(c, 0.9).dashed(3.6, 0.6));
            out.picks.push(Pickable { pos: *px, r: 8.0, obj: ObjectRef::ExoHost(*ei) });
            if lw < 17.6 && k < 24 {
                out.labels.push(Label {
                    pos: [px[0], px[1] + 16.0],
                    text: format!("{} · {}p", e.host, e.planets.len()),
                    size: 10.5,
                    color: theme::rgba(theme::INK_GOLD, 0.55 * pw as f32),
                    priority: 5 + e.planets.len() as i32,
                });
            }
        }
    }
    let _ = cam;
}

// ------------------------------------------------------------ galaxy layer

fn galaxy_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, truth) = (inp.u, inp.modes.truth);
    let w = band(lw, 18.8, 22.8, 0.7);
    if w <= 0.01 {
        return;
    }
    let gc_px = proj.px(u.gal.center);
    // arm centerline engravings (procedural, matching the fog)
    let pitch = 0.2217f64;
    for arm in 0..4 {
        let phase = arm as f64 * std::f64::consts::PI * 0.5 + if arm % 2 == 1 { 0.15 } else { 0.0 };
        let pts: Vec<[f32; 2]> = (0..130)
            .map(|k| {
                let th = -0.6 + k as f64 / 129.0 * 9.8;
                let r = 3.2 * (pitch * th).exp();
                let a = th + phase;
                let local = u.gal.x * (r * a.cos() * KPC) + u.gal.y * (r * a.sin() * KPC);
                proj.px(u.gal.center + local)
            })
            .collect();
        let c = ink([0.55, 0.66, 0.95], 0.16 * w as f32, truth, Prov::Procedural);
        dl.polyline(&pts, false, StrokeStyle::new(c, 0.9).glow(1.6));
    }
    // Sun marker
    let sun_px = proj.px(DVec3::ZERO);
    if on_screen(sun_px, inp.w, inp.h, 60.0) && w > 0.15 {
        dl.circle(
            sun_px,
            7.0,
            StrokeStyle::new(theme::rgba(theme::INK_GOLD, 0.85 * w as f32), 1.2).glow(2.0),
        );
        dl.point(sun_px, 1.6, 5.0, 0.0, theme::rgba(theme::INK_GOLD, 0.9 * w as f32));
        out.labels.push(Label {
            pos: [sun_px[0], sun_px[1] - 16.0],
            text: "Sol — you are here".into(),
            size: 12.5,
            color: theme::rgba(theme::INK_GOLD, 0.9 * w as f32),
            priority: 50,
        });
    }
    if on_screen(gc_px, inp.w, inp.h, 60.0) && w > 0.15 {
        dl.point(gc_px, 2.4, 14.0, 0.5, [1.0, 0.9, 0.7, 0.8 * w as f32]);
        out.labels.push(Label {
            pos: [gc_px[0], gc_px[1] + 14.0],
            text: "Sagittarius A*".into(),
            size: 12.0,
            color: theme::rgba([1.0, 0.9, 0.7], 0.8 * w as f32),
            priority: 45,
        });
    }
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
    let c = ink([0.72, 0.68, 0.95], 0.5 * w as f32, truth, Prov::Measured);
    if g.spiral && r_px > 6.0 {
        // engraved spiral: three nested ellipses + two arm curls
        for (k, f) in [1.0f32, 0.62, 0.3].iter().enumerate() {
            let tilt = 0.9f32 + gi as f32 * 0.3;
            let pts: Vec<[f32; 2]> = (0..80)
                .map(|s| {
                    let a = s as f32 / 80.0 * std::f32::consts::TAU;
                    [
                        px[0] + r_px * f * (a.cos() * tilt.cos() - 0.35 * a.sin() * tilt.sin()),
                        px[1] + r_px * f * 0.42 * a.sin(),
                    ]
                })
                .collect();
            let alpha = 0.4 - k as f32 * 0.08;
            dl.polyline(&pts, true, StrokeStyle::new([c[0], c[1], c[2], alpha * w as f32], 0.9).glow(1.4));
        }
        dl.point(px, r_px * 0.16, r_px * 0.5, 0.0, [1.0, 0.9, 0.75, 0.5 * w as f32]);
    } else {
        let rr = r_px.clamp(2.0, 60.0);
        dl.circle(px, rr, StrokeStyle::new(c, 0.9).dashed(3.0, 0.5));
        dl.point(px, rr * 0.4, rr, 0.0, [c[0], c[1], c[2], 0.25 * w as f32]);
    }
    out.picks.push(Pickable { pos: px, r: r_px.max(8.0), obj: ObjectRef::Galaxy(gi) });
    out.labels.push(Label {
        pos: [px[0], px[1] - r_px.max(6.0) - 8.0],
        text: g.name.into(),
        size: 11.0,
        color: theme::rgba(theme::INK_WHITE, 0.6 * w as f32),
        priority: if g.spiral { 30 } else { 6 },
    });
}

// -------------------------------------------------------- local group layer

fn localgroup_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let u = inp.u;
    let w = band(lw, 20.2, 24.6, 0.6);
    if w <= 0.02 {
        return;
    }
    for (gi, g) in u.galaxies.iter().enumerate() {
        // nearby satellites belong to the galaxy band's own fade window too
        let gw = if g.dist_kpc <= 500.0 { w } else { band(lw, 21.4, 24.6, 0.6) };
        if gw <= 0.02 {
            continue;
        }
        let px = proj.px(g.pos);
        if !on_screen(px, inp.w, inp.h, 40.0) {
            continue;
        }
        draw_galaxy_glyph(inp, proj, gi, px, dl, out, gw);
    }
    // The Milky Way itself, engraved at the origin once the fog has faded
    let mw_w = band(lw, 22.0, 24.8, 0.5);
    let mw_px = proj.px(DVec3::ZERO);
    if mw_w > 0.05 && on_screen(mw_px, inp.w, inp.h, 200.0) {
        let r_px = ((15.0 * KPC) / inp.cam.mpp) as f32;
        let c = theme::rgba([0.72, 0.68, 0.95], 0.5 * mw_w as f32);
        for f in [1.0f32, 0.62, 0.3] {
            dl.circle(mw_px, (r_px * f).max(1.5), StrokeStyle::new(c, 0.9).glow(1.4));
        }
        dl.point(mw_px, (r_px * 0.16).max(1.0), r_px * 0.5, 0.0, [1.0, 0.9, 0.75, 0.5 * mw_w as f32]);
        out.labels.push(Label {
            pos: [mw_px[0], mw_px[1] - r_px.max(6.0) - 8.0],
            text: "Milky Way".into(),
            size: 12.0,
            color: theme::rgba(theme::INK_GOLD, 0.85 * mw_w as f32),
            priority: 48,
        });
    }
    // M31–MW distance engraving
    if let Some(m31) = u.galaxies.iter().position(|g| g.name.contains("M31")) {
        let a = proj.px(DVec3::ZERO);
        let b = proj.px(u.galaxies[m31].pos);
        if w > 0.25 {
            dl.polyline(
                &[a, b],
                false,
                StrokeStyle::new(theme::rgba(theme::INK_FAINT, 0.3 * w as f32), 0.8).dashed(8.0, 0.5),
            );
            out.labels.push(Label {
                pos: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5 - 8.0],
                text: format!("{:.2} Mpc", u.galaxies[m31].dist_kpc / 1000.0),
                size: 10.5,
                color: theme::rgba(theme::INK_FAINT, 0.6 * w as f32),
                priority: 2,
            });
        }
    }
}

// ------------------------------------------------------------ cosmic layer

fn cosmic_layer(inp: &BuildIn, proj: &Proj, lw: f64, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, truth) = (inp.u, inp.modes.truth);
    let w = band(lw, 22.8, 25.9, 0.6);
    if w <= 0.02 {
        return;
    }
    for (ni, n) in u.nodes.iter().enumerate() {
        let px = proj.px(n.pos);
        if !on_screen(px, inp.w, inp.h, 60.0) {
            continue;
        }
        let r_px = ((n.size_mpc * 0.5 * MPC) / inp.cam.mpp) as f32;
        let rr = r_px.clamp(3.0, 200.0);
        let c = ink([0.72, 0.62, 0.92], 0.55 * w as f32, truth, Prov::Measured);
        dl.circle(px, rr, StrokeStyle::new(c, 1.0).glow(1.6));
        dl.circle(px, rr * 0.55, StrokeStyle::new([c[0], c[1], c[2], c[3] * 0.5], 0.8));
        dl.point(px, 2.0, rr * 0.8, 0.0, [c[0], c[1], c[2], 0.3 * w as f32]);
        out.picks.push(Pickable { pos: px, r: rr.max(10.0), obj: ObjectRef::Node(ni) });
        out.labels.push(Label {
            pos: [px[0], px[1] - rr - 8.0],
            text: n.name.into(),
            size: 11.5,
            color: theme::rgba(theme::INK_WHITE, 0.65 * w as f32),
            priority: 12,
        });
    }
    if lw > 25.1 {
        out.labels.push(Label {
            pos: [(inp.w * 0.5) as f32, (inp.h * 0.5 - 40.0) as f32],
            text: "— edge of the local chart · filaments beyond are illustrative —".into(),
            size: 13.0,
            color: theme::rgba([0.7, 0.62, 0.9], 0.6),
            priority: 60,
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
    let d = (inp.cam.center - inp.u.exos[ei].pos).length();
    if d > 0.5 * PC {
        return None;
    }
    Some(ei)
}

fn exo_layer(inp: &BuildIn, proj: &Proj, lw: f64, ei: usize, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, cam, jd, truth) = (inp.u, inp.cam, inp.clock.jd, inp.modes.truth);
    let s = &u.exos[ei];
    let host = s.pos;
    let host_px = proj.px(host);
    let w = band(lw, 6.0, 15.2, 0.5);

    // schematic radius remapping for System View
    let a_list: Vec<Option<(f64, Prov)>> = s.planets.iter().map(|p| s.a_au_of(p)).collect();
    let a_min = a_list.iter().flatten().map(|(a, _)| *a).fold(f64::MAX, f64::min);
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
            });
        }
    }

    // host star — a measured radius earns a physical disc; an unknown radius
    // gets an explicitly schematic glyph (AUDIT FND-020: never silently render
    // a default 1 R☉ as if measured).
    let rgb = s.st_teff.map(crate::data::teff_rgb).unwrap_or([1.0, 0.85, 0.6]);
    let host_z = zmap(proj.screen(host)[2], cam.mpp * inp.w);
    let r_px = match s.st_rad {
        Some(rad) => {
            let r_px = (rad * R_SUN_M / cam.mpp) as f32;
            if r_px > 2.0 {
                dl.disc_z.push(host_z);
                dl.discs.push(DiscInst {
                    center: host_px,
                    pr: [r_px, 0.0, KIND_STAR, 0.4 + ei as f32 * 0.013],
                    sun: [0.0, 0.0, 1.0, 1.0],
                    axis: [0.0, 0.94, 0.34, (inp.anim_t * 0.02) as f32],
                    ca: [rgb[0], rgb[1], rgb[2], 1.0],
                    cb: [rgb[0] * 0.8, rgb[1] * 0.6, rgb[2] * 0.45, 1.0],
                    rim: [rgb[0], rgb[1] * 0.8, rgb[2] * 0.5, 0.6],
                    depth: host_z,
                });
            } else {
                dl.point(host_px, 3.0, 30.0, 0.8, [rgb[0], rgb[1], rgb[2], 0.95]);
            }
            r_px
        }
        None => {
            dl.point(host_px, 2.6, 22.0, 0.6, [rgb[0], rgb[1], rgb[2], 0.9]);
            let c = ink(Prov::Unknown.rgb(), 0.7, true, Prov::Unknown);
            dl.circle(host_px, 11.0, StrokeStyle::new(c, 1.0).dashed(4.0, 0.55));
            11.0
        }
    };
    out.picks.push(Pickable { pos: host_px, r: r_px.max(10.0), obj: ObjectRef::ExoHost(ei) });
    out.labels.push(Label {
        pos: [host_px[0], host_px[1] - r_px.max(8.0) - 12.0],
        text: format!("{}{}", s.host, if s.st_rad.is_none() { " (radius unknown)" } else { "" }),
        size: 13.0,
        color: theme::rgba(theme::INK_WHITE, 0.85),
        priority: 40,
    });

    // planets — world-space conics; the marker rides the same curve as the
    // engraving (AUDIT FND-019).
    let (e1, e2) = exo_plane(u, ei);
    let mut prev_per: Option<f64> = None;
    let mut prev_a_px = 0.0f32;
    for (k, p) in s.planets.iter().enumerate() {
        let Some((a_au, a_prov)) = a_list[k] else { continue };
        let a_draw = remap(a_au);
        let a_px = (a_draw * AU / cam.mpp) as f32;
        if a_px < 4.0 || a_px > 60_000.0 {
            continue;
        }
        let ecc = p.e.unwrap_or(0.0).clamp(0.0, 0.95);
        let a_m = a_draw * AU;
        let b_m = a_m * (1.0 - ecc * ecc).sqrt();
        let n = 140;
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let ea = i as f64 / n as f64 * std::f64::consts::TAU;
                proj.px(host + e1 * (a_m * (ea.cos() - ecc)) + e2 * (b_m * ea.sin()))
            })
            .collect();
        let mut st = StrokeStyle::new(
            ink(theme::INK_GOLD, 0.5 * w as f32, truth, if p.a_au.is_some() { Prov::Measured } else { a_prov }),
            1.0,
        )
        .glow(1.3);
        if p.e.is_none() {
            st = st.dashed(10.0, 0.62);
        }
        dl.polyline(&pts, true, st);

        // marker: same conic, schematic-scaled when System View is active
        let rel = exo_planet_rel(u, ei, k, jd).unwrap_or(DVec3::ZERO) * (a_draw / a_au);
        let world = host + rel;
        let ppx = proj.px(world);
        let pr_px = p.rade.map(|r| (r * R_EARTH_M / cam.mpp) as f32).unwrap_or(0.0);
        let rocky = p.rade.map(|r| r < 1.8).unwrap_or(true);
        if pr_px > 1.8 {
            let sun_dir = view_dir(proj, (-rel).normalize_or_zero());
            let zd = zmap(proj.screen(world)[2], cam.mpp * inp.w);
            dl.disc_z.push(zd);
            dl.discs.push(DiscInst {
                center: ppx,
                pr: [pr_px, 0.0, if rocky { 1.0 } else { 2.0 }, 0.29 + k as f32 * 0.047],
                sun: [sun_dir[0], sun_dir[1], sun_dir[2], 0.06],
                axis: [0.1, 0.92, 0.37, (jd - J2000) as f32 * 0.3],
                ca: if rocky { [0.72, 0.58, 0.45, 1.0] } else { [0.62, 0.72, 0.85, 1.0] },
                cb: if rocky { [0.45, 0.36, 0.3, 1.0] } else { [0.4, 0.5, 0.68, 1.0] },
                rim: [0.6, 0.75, 1.0, if rocky { 0.2 } else { 0.5 }],
                depth: zd,
            });
        } else {
            let c = if p.rade.is_none() && truth {
                theme::rgba(Prov::Unknown.rgb(), 0.9)
            } else {
                theme::rgba([0.75, 0.82, 0.95], 0.9)
            };
            dl.point(ppx, 2.4, 6.0, 0.0, c);
        }
        out.picks.push(Pickable { pos: ppx, r: pr_px.max(8.0), obj: ObjectRef::ExoPlanet(ei, k) });
        if a_px > 30.0 {
            let short = p.name.rsplit(' ').next().unwrap_or(&p.name);
            out.labels.push(Label {
                pos: [ppx[0], ppx[1] - pr_px.max(3.0) - 8.0],
                text: short.into(),
                size: 11.0,
                color: theme::rgba(theme::INK_WHITE, 0.7),
                priority: 8,
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
                        pos: [host_px[0] + mid * 0.7071, host_px[1] + mid * 0.7071],
                        // near-commensurability of periods, not a proven resonance
                        text: format!("≈ {num}:{den}"),
                        size: 10.5,
                        color: theme::rgba([0.55, 0.9, 0.75], 0.8),
                        priority: 6,
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
            pos: [(inp.w * 0.5) as f32, (inp.h - 30.0) as f32],
            text: "SYSTEM VIEW — logarithmic orbit spacing (schematic)".into(),
            size: 12.0,
            color: theme::rgba(theme::INK_GOLD, 0.75),
            priority: 90,
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
    out.picks.push(Pickable { pos: px, r: 13.0, obj: ObjectRef::Star(si) });
    out.labels.push(Label {
        pos: [px[0], px[1] - 24.0],
        text: format!("{} — schematic (no measured radius)", obj_name(inp.u, ObjectRef::Star(si))),
        size: 11.5,
        color: theme::rgba(theme::INK_WHITE, 0.75),
        priority: 35,
    });
}

// ------------------------------------------------------------ gravity view

fn gravity_layer(inp: &BuildIn, proj: &Proj, dl: &mut DrawList, out: &mut FrameOut) {
    let (u, jd, truth) = (inp.u, inp.clock.jd, inp.modes.truth);
    // choose the pair: focused moon → (planet, moon); focused planet → (Sun, planet)
    let pair: Option<(DVec3, f64, DVec3, f64, f64)> = match inp.focus {
        Some(ObjectRef::Moon(mi)) => {
            let m = &MOONS[mi];
            let pp = obj_pos(u, ObjectRef::Planet(m.planet), jd, inp.island);
            let mp = obj_pos(u, ObjectRef::Moon(mi), jd, inp.island);
            // authoritative satellite GM (AUDIT SCI-029)
            Some((pp, PLANETS[m.planet].gm, mp, sim::MOON_GM[mi], m.a_km * 1e3))
        }
        Some(ObjectRef::Planet(i)) => {
            let pp = obj_pos(u, ObjectRef::Planet(i), jd, inp.island);
            Some((DVec3::ZERO, GM_SUN, pp, PLANETS[i].gm, pp.length()))
        }
        _ => None,
    };
    let Some((pa, gm1, pb, gm2, a_m)) = pair else { return };
    let apx = proj.px(pa);
    let bpx = proj.px(pb);
    let sep = ((bpx[0] - apx[0]).powi(2) + (bpx[1] - apx[1]).powi(2)).sqrt();
    if !(40.0..30_000.0).contains(&sep) {
        return;
    }
    let mu = gm2 / (gm1 + gm2);
    out.uni.grav_a = [apx[0], apx[1], mu as f32, sep];
    out.uni.grav_b = [bpx[0], bpx[1], 1.0, 4.0];

    // Lagrange point markers in the true 3D orbital plane (AUDIT SCI-039:
    // the plane basis comes from the pair's relative angular momentum, then
    // projects — correct under any camera tilt).
    let sep_m = (pb - pa).length().max(1.0);
    let e1w = (pb - pa) / sep_m;
    let dtd = 0.01; // days, for the relative velocity direction
    let obj_b = inp.focus.unwrap_or(ObjectRef::Sun);
    let vrel = (obj_pos(u, obj_b, jd + dtd, inp.island)
        - obj_pos(u, obj_b, jd - dtd, inp.island))
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
            });
        }
    }
}
