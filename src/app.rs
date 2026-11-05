//! Application shell shared by the interactive window and headless export:
//! one code path builds the atlas frame, renders the canvas, and lays the UI
//! (where-am-I readout, scale rail, search, selection card, time bar) over it.

mod search;
mod settings;
mod ui;

use std::time::Instant;

use egui::{Align2, Color32, Rect};
use glam::DVec3;

use crate::atlas::{
    self, build, obj_half_extent, obj_name, obj_pos, BuildIn, FrameOut, Island, Modes, ObjectRef,
    Pickable,
};
use crate::data::{Prov, Universe};
use crate::engine::draw::{DrawList, Pipelines};
use crate::engine::gpu::{save_png, GpuContext, ViewTarget};
use crate::engine::textures::TextureBank;
use crate::frames::{fmt_len, Camera, AU};
use crate::sim::{self, fmt_jd, jd_now, Clock, NBody, MOONS, PLANETS};
use crate::theme;

pub use search::{Dest, SearchState};
pub use settings::Settings;

// ------------------------------------------------------------- state

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PathKey {
    #[serde(default)]
    pub center: [f64; 3],
    pub mpp: f64,
    /// the window width `mpp` was recorded at, so a render at any size frames the same view
    #[serde(default)]
    pub view_w: Option<f64>,
    #[serde(default)]
    pub tilt: f64,
    #[serde(default)]
    pub yaw: f64,
    pub jd: f64,
    pub dur: f64,
    #[serde(default)]
    pub hold: f64,
    /// anchor the key to a named object (planet, moon, craft, star, exo host)
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
}

pub struct PathPlay {
    pub keys: Vec<PathKey>,
    pub k: usize,
    pub t: f64,
}

/// What the selection card shows: title, subtitle, facts, sources, can simulate.
type CardData = (
    String,
    String,
    Vec<(String, String)>,
    Vec<(String, crate::evidence::Ev)>,
    bool,
);

/// A saved viewpoint: where Back returns to.
#[derive(Clone, Copy)]
struct HistoryEntry {
    center: DVec3,
    mpp: f64,
    tilt: f64,
    yaw: f64,
    focus: Option<ObjectRef>,
    follow: Option<(ObjectRef, DVec3)>,
    /// the exoplanet system this view was in, so Back returns to it and not to empty space
    scene_exo: Option<usize>,
}

/// Most substeps a simulation takes in one frame.
const ISLAND_MAX_SUBSTEPS: usize = 4000;

/// How many substeps to take this frame, or `None` if time has outrun the simulation. One slow
/// frame only makes its steps coarser for that frame: it takes half a second of frames that
/// can't keep up to end a simulation, or a step so coarse the moons would fly apart.
fn island_substeps(dt_sim: f64, dt_sub: f64, dt: f64, shortfall_s: &mut f64) -> Option<usize> {
    let wanted = (dt_sim.abs() / dt_sub).ceil();
    if wanted <= ISLAND_MAX_SUBSTEPS as f64 {
        *shortfall_s = 0.0;
        return Some(wanted as usize);
    }
    *shortfall_s += dt;
    if *shortfall_s > 0.5 || wanted > 20.0 * ISLAND_MAX_SUBSTEPS as f64 {
        return None;
    }
    Some(ISLAND_MAX_SUBSTEPS)
}

/// Write a new file named `stem`.json in `dir`, or `stem`-2.json and so on: never over an old one.
pub(crate) fn write_new_file(
    dir: &std::path::Path,
    stem: &str,
    contents: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use std::io::Write;
    for n in 1..10_000 {
        let name = if n == 1 {
            format!("{stem}.json")
        } else {
            format!("{stem}-{n}.json")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(contents.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("too many saved flights named {stem}")
}

/// Where the planet sits in a simulation's bodies: after the Sun, before its moons.
const ISLAND_PLANET: usize = 1;

/// The GM an object carries in a simulation: the same JPL values the gravity view uses.
fn island_gm(o: ObjectRef) -> f64 {
    match o {
        ObjectRef::Sun => sim::GM_SUN,
        ObjectRef::Planet(i) => PLANETS[i].gm,
        ObjectRef::Moon(mi) => sim::MOON_GM[mi],
        _ => 0.0,
    }
}

/// A simulation of a planet's system as it stands on this date: Sun, planet, then its moons.
fn build_island(u: &Universe, pi: usize, jd: f64) -> Island {
    let mut ids = vec![ObjectRef::Sun, ObjectRef::Planet(pi)];
    for (mi, m) in MOONS.iter().enumerate() {
        if m.planet == pi {
            ids.push(ObjectRef::Moon(mi));
        }
    }
    let dt = 60.0;
    let mut gm = Vec::new();
    let mut pos = Vec::new();
    let mut vel = Vec::new();
    for o in &ids {
        let p0 = obj_pos(u, *o, jd - dt / 86_400.0, None);
        let p1 = obj_pos(u, *o, jd + dt / 86_400.0, None);
        pos.push(obj_pos(u, *o, jd, None));
        vel.push((p1 - p0) / (2.0 * dt));
        gm.push(island_gm(*o));
    }
    let n = ids.len();
    let mut sim = NBody {
        gm,
        pos,
        vel,
        label: format!("{} system", PLANETS[pi].name),
    };
    sim.zero_momentum();
    let e0 = sim.orbit_energies(ISLAND_PLANET);
    Island {
        sim,
        ids,
        anchor: ObjectRef::Planet(pi),
        trails: vec![Vec::new(); n],
        jd,
        e0,
        peak_drift: 0.0,
        coarse_frames: 0,
        coarsest: 1.0,
    }
}

/// What a wheel zoom holds still. One rule: the zoom point moves with the thing it's on.
#[derive(Clone, Copy)]
struct ZoomPoint {
    /// What it's on: the named thing under the pointer, or with nothing named there what you're
    /// following, or with nothing followed, nothing at all.
    on: Option<ObjectRef>,
    /// The point itself when it's on nothing, in the camera's frame.
    fixed: DVec3,
    /// Where on the screen it's held.
    screen: [f64; 2],
    /// Where the thing it's on sits from there, in metres along the screen's axes, so that only
    /// the zoom changes how far that looks: neither time nor the presentation tilt can.
    gap: [f64; 2],
    /// It's near enough the camera's plane for the camera to come to its depth, which matters
    /// to what the camera counts as near. A star that only lines up with the pointer isn't.
    to_its_depth: bool,
    /// A click here would have selected it, so the camera stays with it when the zoom ends.
    aimed: bool,
}

/// The way back from a scroll out: what it held, from what scale, and where the pointer was at
/// the last notch.
#[derive(Clone, Copy)]
struct ScrolledOut {
    point: ZoomPoint,
    from_mpp: f64,
    pointer: [f32; 2],
}

pub struct Pantheon {
    pub gpu: GpuContext,
    pub pipes: Pipelines,
    pub bank: TextureBank,
    pub u: Universe,
    pub cam: Camera,
    pub clock: Clock,
    pub modes: Modes,
    pub focus: Option<ObjectRef>,
    /// active exoplanet pocket — persists independently of selection
    pub scene_exo: Option<usize>,
    /// The system a flight is heading to, once it has left the one it's in.
    scene_next: Option<usize>,
    pub follow: Option<(ObjectRef, DVec3)>,
    pub island: Option<Island>,
    /// how long the simulation has been unable to keep up with the clock, in seconds
    island_shortfall: f64,
    pub capture: Vec<PathKey>,
    pub play: Option<PathPlay>,
    pub picks: Vec<Pickable>,
    pub fo: Option<FrameOut>,
    pub dl: DrawList,
    pub anim_t: f64,
    pub show_capture: bool,
    pub show_help: bool,
    pub show_layers: bool,
    /// cinematic renders: canvas + labels only, no controls
    pub cinema: bool,
    /// exports: the camera settles at once rather than easing
    pub immediate: bool,
    /// exports: imagery loads synchronously
    pub block_textures: bool,
    /// set by shot setups that could not resolve their subject
    pub setup_error: Option<String>,
    pub status: Option<(String, f64)>,
    /// persisted preferences (GUI only; exports never touch the disk)
    pub settings: Settings,
    pub persist_settings: bool,
    /// None until the audio engine has been tried; Some(false) = no device found
    pub ambience_available: Option<bool>,
    /// glide the clock during path playback instead of jumping
    pub jd_glide: Option<f64>,
    pub search: SearchState,
    /// the canvas in logical pixels, and the display's scale
    pub view_w: f64,
    pub view_h: f64,
    pub ppp: f32,
    /// what's under the pointer right now
    pub hover: Option<ObjectRef>,
    /// labels placed last frame, for clicks and hysteresis
    sticky: std::collections::HashSet<String>,
    label_hits: Vec<(Rect, ObjectRef)>,
    panel_rects: Vec<Rect>,
    /// back-stack for flights
    history: Vec<HistoryEntry>,
    // ---- motion state
    /// log10(mpp) still to be applied by the smooth zoom
    zoom_pending: f64,
    zoom_anchor: Option<ZoomPoint>,
    /// Scrolling back with the pointer still there retraces the way out.
    scrolled_out: Option<ScrolledOut>,
    /// release velocity of a drag, px/s
    pan_vel: [f64; 2],
    drag_vel: [f64; 2],
    last_drag: f64,
    /// a held key's continuous pan/zoom
    key_pan: [f64; 2],
    /// riding the scale rail: the target scale and the body it starts from
    scrub: Option<(f64, ObjectRef)>,
    scrub_held: bool,
    pres_moving: bool,
    /// someone who has clicked a thing doesn't need telling that they can
    learned_click: bool,
    /// the first-run hint, until dismissed
    pub intro: bool,
    intro_born: f64,
    /// what a fresh clone is missing, in plain words
    pub data_gaps: Vec<String>,
}

/// A body fills this fraction of the shorter window side when flown to.
const FRAME_FILL: f64 = 0.62;

#[derive(Clone, Copy)]
enum KeyAction {
    Escape,
    TogglePlay,
    Truth,
    Gravity,
    SystemView,
    Search,
    Help,
    Home,
    Back,
    Now,
    Ambience,
    FlyToSelection,
    LevelView,
    Slower,
    Faster,
}

impl Pantheon {
    pub fn new(gpu: GpuContext) -> Self {
        let (u, health) = Universe::load_with_health();
        let problems: Vec<&String> = health
            .lines
            .iter()
            .filter(|l| l.contains("MISSING") || l.contains("INVALID"))
            .collect();
        for p in &problems {
            log::info!("data health: {p}");
        }
        let clock = Clock::now();
        let mut cam = Camera::new();
        // the opening view: Earth, large, where it is right now
        cam.center = sim::body_pos(2, clock.jd);
        let view_h = 900.0;
        cam.mpp = (crate::sim::PLANETS[2].radius_km * 1e3 * 2.0) / (FRAME_FILL * view_h);
        cam.tilt = 0.0;
        let bank = TextureBank::new(&gpu);
        let pipes = Pipelines::new(&gpu, &bank.bgl);
        let mut data_gaps: Vec<String> = Vec::new();
        if !bank.status.any_planet_imagery() {
            data_gaps.push("Planet maps are missing, so surfaces are procedural.".into());
        }
        let damaged = crate::spice::problems();
        data_gaps.extend(damaged.iter().cloned());
        if crate::spice::spice().is_none() && damaged.is_empty() {
            data_gaps.push(
                "JPL kernels are missing, so positions use approximate orbital elements.".into(),
            );
        }
        if u.craft.is_empty() {
            data_gaps.push("Spacecraft tracks are missing.".into());
        }
        if u.stars.len() < 1000 {
            data_gaps.push("Gaia star catalogs are missing.".into());
        }
        if !data_gaps.is_empty() {
            eprintln!("pantheon: running with partial data. Run tools/fetch_data.py to add it:");
            for g in &data_gaps {
                eprintln!("  - {g}");
            }
        }
        let status = match damaged.first() {
            Some(problem) => Some((problem.clone(), 30.0)),
            None => (!data_gaps.is_empty()).then(|| {
                (
                    "Running with basic data. Open the view menu to see what's missing."
                        .to_string(),
                    12.0,
                )
            }),
        };
        let search = SearchState::new(&u);
        Self {
            gpu,
            pipes,
            bank,
            u,
            cam,
            clock,
            modes: Modes::default(),
            focus: Some(ObjectRef::Planet(2)),
            scene_exo: None,
            scene_next: None,
            follow: Some((ObjectRef::Planet(2), DVec3::ZERO)),
            island: None,
            island_shortfall: 0.0,
            capture: Vec::new(),
            play: None,
            picks: Vec::new(),
            fo: None,
            dl: DrawList::default(),
            anim_t: 0.0,
            show_capture: false,
            show_help: false,
            show_layers: false,
            cinema: false,
            immediate: false,
            block_textures: false,
            setup_error: None,
            status,
            settings: Settings::default(),
            persist_settings: false,
            ambience_available: None,
            jd_glide: None,
            search,
            view_w: 1600.0,
            view_h,
            ppp: 1.0,
            hover: None,
            sticky: std::collections::HashSet::new(),
            label_hits: Vec::new(),
            panel_rects: Vec::new(),
            history: Vec::new(),
            zoom_pending: 0.0,
            zoom_anchor: None,
            scrolled_out: None,
            pan_vel: [0.0, 0.0],
            drag_vel: [0.0, 0.0],
            last_drag: -1.0,
            key_pan: [0.0, 0.0],
            scrub: None,
            scrub_held: false,
            pres_moving: false,
            learned_click: false,
            intro: false,
            intro_born: 0.0,
            data_gaps,
        }
    }

    /// Hand over to the GUI: remembered preferences, and the first-run hint.
    pub fn attach_settings(&mut self, settings: Settings) {
        self.intro = !settings.intro_seen;
        self.intro_born = self.anim_t;
        self.settings = settings;
        self.persist_settings = true;
    }

    pub fn save_settings(&self) {
        if self.persist_settings {
            self.settings.save();
        }
    }

    pub fn dismiss_intro(&mut self) {
        if self.intro {
            self.intro = false;
            self.settings.intro_seen = true;
            self.save_settings();
        }
    }

    /// Remember the current viewpoint before a jump (dive, rail, home).
    fn push_history(&mut self) {
        // the first-run panel says "This is Earth", which stops being true on the way out
        self.dismiss_intro();
        self.history.push(HistoryEntry {
            center: self.cam.center_root(),
            mpp: self.cam.mpp,
            tilt: self.cam.tilt,
            yaw: self.cam.yaw,
            focus: self.focus,
            follow: self.follow,
            scene_exo: self.scene_exo,
        });
        if self.history.len() > 32 {
            self.history.remove(0);
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty()
    }

    /// Fly back to the previous viewpoint (Backspace or the back button).
    pub fn go_back(&mut self) {
        let Some(h) = self.history.pop() else {
            self.note("No earlier view to return to");
            return;
        };
        self.stop_motion();
        self.focus = h.focus;
        self.follow = h.follow;
        if let Some(ei) = h.scene_exo {
            self.head_for_scene(ei);
        }
        let dur = self.cam.flight_seconds(h.center, h.mpp, self.view_w);
        self.cam
            .fly_to_view(h.center, h.mpp, h.tilt, h.yaw, self.view_w, dur);
    }

    /// A wheel notch or pinch at the pointer, or at the middle of the view with no pointer.
    fn wheel_zoom(&mut self, s: Option<[f32; 2]>, dex: f64, proj: &crate::frames::Proj) {
        self.zoom_anchor = s.map(|s| self.zoom_point_at(s, dex, proj));
        self.zoom_pending += dex;
        self.cam.anim = None;
        self.play = None;
        self.pan_vel = [0.0, 0.0];
        self.dismiss_intro();
    }

    /// A wheel or pinch's zoom point at the pointer. Diving in, it leans toward the nearest thing
    /// with a name, so repeated scrolling converges on it instead of demanding pixel-perfect aim
    /// across 20 decades, and only with nothing named near the pointer toward the selection.
    /// Scrolling back from a scroll out retraces it, so you return to where you were whatever
    /// came into view at the far end. A resting hand moves a little between notches, so only a
    /// deliberate move ends that: past `DELIBERATE_PX`, or onto something else with a name.
    fn zoom_point_at(&mut self, s: [f32; 2], dex: f64, proj: &crate::frames::Proj) -> ZoomPoint {
        const DELIBERATE_PX: f32 = 12.0;
        let (sx, sy) = (s[0] as f64, s[1] as f64);
        let nearest = self
            .picks
            .iter()
            .map(|pk| ((pk.pos[0] - s[0]).hypot(pk.pos[1] - s[1]), pk.obj))
            .filter(|(d, _)| *d < 150.0)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(mut back) = self.scrolled_out {
            let moved = (back.pointer[0] - s[0]).hypot(back.pointer[1] - s[1]);
            let onto_another =
                nearest.is_some_and(|(d, obj)| Some(obj) != back.point.on && d < moved / 2.0);
            let still_out = dex > 0.0 || proj.mpp > back.from_mpp * 1.001;
            if moved <= DELIBERATE_PX && !onto_another && still_out {
                back.pointer = s;
                // the same gap all the way back, unless time has moved things meanwhile: measured
                // again at a far scale it would lose what precision it has for the near one
                let p = back.point;
                let now = self.zoom_point(p.on, p.fixed, p.screen, 1.0, proj);
                let drift = (now.gap[0] - p.gap[0]).hypot(now.gap[1] - p.gap[1]) / proj.mpp;
                if drift > 0.5 {
                    back.point = now;
                }
                self.scrolled_out = Some(back);
                return back.point;
            }
        }
        self.scrolled_out = None;
        let selected = self.focus.map(|obj| {
            let px = proj.px(obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref()));
            ((px[0] - s[0]).hypot(px[1] - s[1]), obj)
        });
        let named = nearest.or(selected.filter(|(d, _)| *d < 150.0));
        let (on, keep) = match named {
            Some((d, obj)) if dex < 0.0 => (
                Some(obj),
                1.0 - (0.45 * (1.0 - d as f64 / 150.0)).clamp(0.0, 0.45),
            ),
            Some((_, obj)) => (Some(obj), 1.0),
            None => (self.follow.map(|(obj, _)| obj), 1.0),
        };
        // local anchor math: full precision inside attached frames
        let here = proj.unproject_local(sx, sy);
        let mut point = self.zoom_point(on, here, [sx, sy], keep, proj);
        if dex > 0.0 {
            self.scrolled_out = Some(ScrolledOut {
                point,
                from_mpp: proj.mpp,
                pointer: s,
            });
        } else {
            point.aimed = named.is_some_and(|(_, obj)| self.hover == Some(obj));
        }
        point
    }

    /// A zoom point held at `screen`, with what it's on where it is now. `keep` below 1 draws
    /// that thing toward `screen` as the zoom starts.
    fn zoom_point(
        &self,
        on: Option<ObjectRef>,
        fixed: DVec3,
        screen: [f64; 2],
        keep: f64,
        proj: &crate::frames::Proj,
    ) -> ZoomPoint {
        let at = proj.screen_local(self.zoom_base(on, fixed));
        ZoomPoint {
            on,
            fixed,
            screen,
            gap: [
                (at[0] - screen[0]) * proj.mpp * keep,
                (at[1] - screen[1]) * proj.mpp * keep,
            ],
            to_its_depth: (self.zoom_base(on, fixed) - proj.center)
                .dot(proj.out)
                .abs()
                < proj.mpp * proj.w,
            aimed: false,
        }
    }

    /// Where a zoom point's thing is now, simulated if it's simulated, in the camera's frame.
    fn zoom_base(&self, on: Option<ObjectRef>, fixed: DVec3) -> DVec3 {
        on.map_or(fixed, |obj| {
            obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref()) - self.cam.frame_origin
        })
    }

    /// Put the camera where the zoom point sits where it should, through the projection as it
    /// is this frame, with time moved on. It runs until the view has stopped turning too, since
    /// the presentation tilt outlasts the zoom.
    fn hold_zoom_point(&mut self) {
        let Some(z) = self.zoom_anchor else {
            return;
        };
        if self.cam.anim.is_some() {
            self.zoom_anchor = None;
            return;
        }
        let proj = crate::frames::Proj::new(&self.cam, self.view_w, self.view_h, &self.u.gal);
        let m = self.cam.mpp;
        let base = self.zoom_base(z.on, z.fixed);
        let depth = if z.to_its_depth {
            0.0
        } else {
            (self.cam.center - base).dot(proj.out)
        };
        self.cam.center = base
            - proj.right * ((z.screen[0] - proj.w * 0.5) * m + z.gap[0])
            - proj.up * ((proj.h * 0.5 - z.screen[1]) * m - z.gap[1])
            + proj.out * depth;
        self.sync_follow();
        if self.zoom_pending != 0.0 || self.pres_moving {
            return;
        }
        self.zoom_anchor = None;
        if let (true, Some(obj)) = (z.aimed, z.on) {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            self.follow = Some((obj, self.cam.center_root() - p));
        }
    }

    /// End a flight where the camera is now. A follow left pointing at the destination
    /// would otherwise snap the camera to it at whatever zoom the flight had reached.
    fn stop_flight_here(&mut self) {
        if self.cam.anim.take().is_some() {
            self.sync_follow();
        }
    }

    fn stop_motion(&mut self) {
        self.zoom_pending = 0.0;
        self.zoom_anchor = None;
        self.scrolled_out = None;
        self.scene_next = None;
        self.pan_vel = [0.0, 0.0];
    }

    /// Fly to Earth, the place everyone starts from.
    pub fn go_home(&mut self) {
        self.dive_to(ObjectRef::Planet(2));
    }

    fn note(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), self.anim_t + 4.0));
    }

    // --------------------------------------------------------- sim tick

    pub fn tick(&mut self, dt: f64) {
        self.anim_t += dt;
        // cinematic path playback
        if let Some(play) = &mut self.play {
            if self.cam.anim.is_none() {
                play.t += dt;
                let hold = play.keys.get(play.k).map(|k| k.hold).unwrap_or(0.0);
                if play.t >= hold {
                    play.k += 1;
                    play.t = 0.0;
                    if let Some(key) = play.keys.get(play.k).cloned() {
                        self.apply_key(&key);
                    } else {
                        self.play = None;
                        self.note("Path finished");
                    }
                }
            }
        }
        // path glides: ease the clock alongside the camera animation
        if let Some(jd1) = self.jd_glide {
            match &self.cam.anim {
                Some(a) => {
                    let s = (a.t / a.dur).clamp(0.0, 1.0);
                    let e = s * s * (3.0 - 2.0 * s);
                    self.clock.jd += (jd1 - self.clock.jd) * e.max(0.02);
                }
                None => {
                    self.clock.jd = jd1;
                    self.jd_glide = None;
                }
            }
        }
        // a flight chases a moving target instead of snapping to it on arrival
        if let (Some(a), Some((obj, off))) = (&mut self.cam.anim, self.follow) {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            a.c1 = (p + off) - self.cam.frame_origin;
        }
        self.cam.tick(dt, self.view_w);
        self.apply_scrub(dt);
        self.apply_smooth_zoom(dt);
        self.apply_pan_inertia(dt);
        // one time switch: play/pause is the source of truth, "living" derives
        self.modes.living = self.clock.playing;
        self.clock.tick(dt);

        // N-body island stepping
        let mut outrun = false;
        if self.modes.living && self.clock.playing {
            let dt_sim = dt * self.clock.rate;
            if let Some(isl) = &mut self.island {
                let min_p = isl
                    .ids
                    .iter()
                    .filter_map(|o| match o {
                        ObjectRef::Moon(mi) => Some(MOONS[*mi].period_d.abs()),
                        _ => None,
                    })
                    .fold(f64::MAX, f64::min);
                let dt_sub = if min_p == f64::MAX {
                    3600.0
                } else {
                    min_p * 86_400.0 / 600.0
                };
                match island_substeps(dt_sim, dt_sub, dt, &mut self.island_shortfall) {
                    None => outrun = true,
                    Some(0) => {}
                    Some(n) => {
                        let h = dt_sim / n as f64;
                        let coarse = h.abs() / dt_sub;
                        if coarse > 1.001 {
                            isl.coarse_frames += 1;
                            isl.coarsest = isl.coarsest.max(coarse);
                        }
                        for s in 0..n {
                            isl.sim.step(h);
                            isl.peak_drift = isl
                                .peak_drift
                                .max(isl.sim.orbit_energy_drift(ISLAND_PLANET, &isl.e0));
                            if s % 4 == 0 {
                                for (k, p) in isl.sim.pos.iter().enumerate() {
                                    isl.trails[k].push(*p);
                                    if isl.trails[k].len() > 500 {
                                        isl.trails[k].remove(0);
                                    }
                                }
                            }
                        }
                        isl.jd += dt_sim / 86_400.0;
                    }
                }
            }
        }
        // follow a moving focus so Living Orbits keeps the subject centered
        if self.cam.anim.is_none() {
            if let Some((obj, off)) = self.follow {
                let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
                self.cam.center = (p + off) - self.cam.frame_origin;
            }
        }
        // camera frame attachment: inside an exoplanet
        // pocket the camera rebases to the host's frame so local precision
        // is AU-scale, not 1e17-scale
        let want_origin = self
            .scene_exo
            .map(|ei| self.u.exos[ei].pos)
            .unwrap_or(DVec3::ZERO);
        if (want_origin - self.cam.frame_origin).length() > 1e-3 {
            // a zoom point on nothing is held in the old frame's coordinates
            let delta = self.cam.frame_origin - want_origin;
            if let Some(z) = &mut self.zoom_anchor {
                z.fixed += delta;
            }
            if let Some(back) = &mut self.scrolled_out {
                back.point.fixed += delta;
            }
            self.cam.rebase(want_origin);
        }

        // a simulation only means something at the clock's date, so it ends rather than
        // showing bodies somewhere else: when time outruns it, or the clock jumps away
        if self.island.is_some() {
            let away = self
                .island
                .as_ref()
                .is_some_and(|isl| (isl.jd - self.clock.jd).abs() > 0.5);
            if outrun {
                self.island = None;
                self.note("Simulation ended: time is running too fast for it");
            } else if away {
                self.island = None;
                self.note("Simulation ended: the clock moved away from it");
            }
        }
        self.update_presentation(dt);
        self.hold_zoom_point();
        self.update_scene();
        if let Some((_, until)) = self.status {
            if self.anim_t > until {
                self.status = None;
            }
        }
    }

    /// Is a view this wide, centred here, in this system? The star's within `reach` views of it,
    /// or the view is among the planets.
    fn view_in_system(&self, ei: usize, center: DVec3, view_m: f64, reach: f64) -> bool {
        let among = obj_half_extent(&self.u, ObjectRef::ExoHost(ei));
        (center - self.u.exos[ei].pos).length() < reach * view_m.max(among)
    }

    /// An exoplanet system stays on until the camera has really left it. A flight there starts
    /// far away, so distance means nothing until it has landed; a flight anywhere else leaves
    /// when it leaves. And whenever the view comes to rest on a host, by any route, its system
    /// is what's there.
    fn update_scene(&mut self) {
        let center = self.cam.center_root();
        let view_m = self.cam.mpp * self.view_w;
        let lw = view_m.log10();
        if let Some(ei) = self.scene_exo {
            let arriving = self.cam.anim.as_ref().is_some_and(|a| {
                let dest = self.cam.frame_origin + a.c1;
                self.view_in_system(ei, dest, 10f64.powf(a.lm1) * self.view_w, 3.0)
            });
            if !arriving && (lw > 15.8 || !self.view_in_system(ei, center, view_m, 3.0)) {
                self.scene_exo = self.scene_next.take();
            }
        }
        if self.cam.anim.is_some() {
            return;
        }
        if let Some(next) = self.scene_next.take() {
            if self.view_in_system(next, center, view_m, 3.0) {
                self.scene_exo = Some(next);
            }
        }
        if self.scene_exo.is_none() && lw < 15.6 {
            let nearest = self
                .u
                .exos
                .iter()
                .enumerate()
                .filter(|(_, e)| e.dist_pc.is_some())
                .map(|(ei, e)| ((center - e.pos).length_squared(), ei))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, ei)) =
                nearest.filter(|(_, ei)| self.view_in_system(*ei, center, view_m, 0.6))
            {
                self.scene_exo = Some(ei);
            }
        }
    }

    /// A flight's headed for this system. The one it's in, if any, stays until it has left.
    fn head_for_scene(&mut self, ei: usize) {
        if self.scene_exo.is_some_and(|now| now != ei) {
            self.scene_next = Some(ei);
        } else {
            self.scene_exo = Some(ei);
        }
    }

    /// Resolve a human name to an object (planets, moons, craft, named stars,
    /// exoplanet hosts, and a couple of frame anchors).
    pub fn find_object(&self, name: &str) -> Option<ObjectRef> {
        let n = name.trim();
        if n.eq_ignore_ascii_case("sun") || n.eq_ignore_ascii_case("sol") {
            return Some(ObjectRef::Sun);
        }
        if let Some(i) = PLANETS.iter().position(|p| p.name.eq_ignore_ascii_case(n)) {
            return Some(ObjectRef::Planet(i));
        }
        if let Some(i) = MOONS.iter().position(|m| m.name.eq_ignore_ascii_case(n)) {
            return Some(ObjectRef::Moon(i));
        }
        if let Some(i) = self
            .u
            .craft
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(n))
        {
            return Some(ObjectRef::Craft(i));
        }
        if let Some(i) = self
            .u
            .stars
            .iter()
            .position(|s| s.name.map(|sn| sn.eq_ignore_ascii_case(n)).unwrap_or(false))
        {
            return Some(ObjectRef::Star(i));
        }
        self.u.find_system(n).map(ObjectRef::ExoHost)
    }

    /// Where a scale on the rail looks: the chosen body when close, the Sun at
    /// system and stellar scales, the galactic centre for the Galaxy, and the
    /// Local Group beyond. The blend makes the whole ladder one continuous ride.
    fn ladder_center(&self, lw: f64, subject: ObjectRef) -> DVec3 {
        use crate::frames::smoothstep;
        let body = obj_pos(&self.u, subject, self.clock.jd, self.island.as_ref());
        let leaves = Self::ladder_leaves(subject);
        let near = body.lerp(DVec3::ZERO, smoothstep(leaves, leaves + 1.8, lw));
        let t = smoothstep(19.2, 20.6, lw) * (1.0 - smoothstep(22.2, 23.2, lw));
        near.lerp(self.u.gal.center, t)
    }

    /// The scale where the ladder leaves its subject for the Sun: a body in the Solar System is
    /// lost among the planets, and an exoplanet system only ends with interstellar space.
    fn ladder_leaves(subject: ObjectRef) -> f64 {
        match subject {
            ObjectRef::ExoHost(_) | ObjectRef::ExoPlanet(..) => 15.8,
            _ => 10.2,
        }
    }

    /// Start dragging the rail, or carry on with it. In a system, the rail is about that system.
    fn scrub_to(&mut self, v: f64) {
        let in_system = self.scene_exo.map(|ei| match self.focus {
            Some(ObjectRef::ExoPlanet(e, k)) if e == ei => ObjectRef::ExoPlanet(e, k),
            _ => ObjectRef::ExoHost(ei),
        });
        let subject = self
            .scrub
            .map(|(_, o)| o)
            .or(in_system)
            .or_else(|| self.near_subject())
            .unwrap_or(ObjectRef::Planet(2));
        if self.scrub.is_none() {
            self.push_history();
            self.stop_motion();
        }
        self.scrub = Some((v, subject));
        self.dismiss_intro();
    }

    /// Dragging the rail moves through scale directly, staying on the ladder so
    /// there's always something to see, wherever the camera started.
    fn apply_scrub(&mut self, dt: f64) {
        let Some((target, subject)) = self.scrub else {
            return;
        };
        let k = if self.immediate {
            1.0
        } else {
            1.0 - (-dt / 0.08).exp()
        };
        let lw = self.cam.log_width(self.view_w);
        let now = lw + (target - lw) * k;
        self.cam.mpp =
            (10f64.powf(now) / self.view_w).clamp(crate::frames::MPP_MIN, crate::frames::MPP_MAX);
        let on_ladder = self.ladder_center(now, subject) - self.cam.frame_origin;
        self.cam.center = self.cam.center.lerp(on_ladder, k);
        self.cam.anim = None;
        self.follow = None;
        if !self.scrub_held && (target - now).abs() < 0.002 {
            self.scrub = None;
            if now < Self::ladder_leaves(subject) {
                self.follow = Some((subject, DVec3::ZERO));
            }
        }
    }

    /// A click on the canvas selects what it hit, and means you've finished with the way back.
    fn click_canvas(&mut self, hit: Option<ObjectRef>) {
        self.pan_vel = [0.0, 0.0];
        self.focus = hit;
        self.learned_click |= hit.is_some();
        self.scrolled_out = None;
        // following continues only for the thing already being followed
        if self.follow.map(|(o, _)| Some(o) != hit).unwrap_or(false) {
            self.stop_following();
        }
    }

    /// Stop following, and stop holding anything on the screen once a zoom is over.
    pub(crate) fn stop_following(&mut self) {
        self.follow = None;
        if self.zoom_pending == 0.0 {
            self.zoom_anchor = None;
        }
    }

    /// Smooth zoom: wheel notches feed `zoom_pending`, which drains over ~0.1 s
    /// around a fixed anchor, so a burst of notches glides instead of stepping.
    fn apply_smooth_zoom(&mut self, dt: f64) {
        if self.zoom_pending == 0.0 {
            return;
        }
        let take = if self.immediate {
            1.0
        } else {
            1.0 - (-dt / 0.09).exp()
        };
        let mut step = self.zoom_pending * take;
        if self.zoom_pending.abs() < 0.0004 {
            step = self.zoom_pending;
        }
        self.zoom_pending -= step;
        let new =
            (self.cam.mpp * 10f64.powf(step)).clamp(crate::frames::MPP_MIN, crate::frames::MPP_MAX);
        // hitting the zoom limits ends the glide
        if new == crate::frames::MPP_MIN || new == crate::frames::MPP_MAX {
            self.zoom_pending = 0.0;
        }
        self.cam.mpp = new;
        // a zoom point sets the camera at the end of the frame, with everything moved
        if self.zoom_anchor.is_none() {
            self.sync_follow();
        }
    }

    /// A flung drag coasts and settles.
    fn apply_pan_inertia(&mut self, dt: f64) {
        let held = self.key_pan != [0.0, 0.0];
        if self.pan_vel == [0.0, 0.0] && !held {
            return;
        }
        if held {
            self.zoom_anchor = None;
            self.scrolled_out = None;
        }
        let proj = crate::frames::Proj::new(&self.cam, self.view_w, self.view_h, &self.u.gal);
        let (vx, vy) = (
            self.pan_vel[0] + self.key_pan[0],
            self.pan_vel[1] + self.key_pan[1],
        );
        self.cam.center -= proj.right * (vx * dt * self.cam.mpp);
        self.cam.center += proj.up * (vy * dt * self.cam.mpp);
        let k = (-dt / 0.38).exp();
        self.pan_vel = [self.pan_vel[0] * k, self.pan_vel[1] * k];
        if self.pan_vel[0].hypot(self.pan_vel[1]) < 6.0 {
            self.pan_vel = [0.0, 0.0];
        }
        self.sync_follow();
    }

    /// Close to a body, ease to a view that shows it well: sunlit three-quarter,
    /// north up. Further out the map returns to plan view on its own.
    fn update_presentation(&mut self, dt: f64) {
        let view_m = self.cam.mpp * self.view_w;
        let c = self.cam.center_root();
        let jd = self.clock.jd;
        let mut target = (0.0, 0.0);
        if self.scene_exo.is_none() && self.cam.log_width(self.view_w) < 11.2 {
            // the subject: what's selected if it's near, else the closest body
            let mut best: Option<(f64, ObjectRef, f64)> = None;
            let mut consider = |obj: ObjectRef, this: &Self| {
                let Some(r) = atlas::body_frame_radius(obj) else {
                    return;
                };
                let p = obj_pos(&this.u, obj, jd, this.island.as_ref());
                let d = (p - c).length();
                if d < view_m * 0.9 {
                    let score = d / r;
                    if best.map(|(b, _, _)| score < b).unwrap_or(true) {
                        best = Some((score, obj, r));
                    }
                }
            };
            consider(ObjectRef::Sun, self);
            for i in 0..PLANETS.len() {
                consider(ObjectRef::Planet(i), self);
            }
            for mi in 0..MOONS.len() {
                consider(ObjectRef::Moon(mi), self);
            }
            if let Some((_, obj, r)) = best {
                // full effect while the body spans a sixth of the view or more
                let span = view_m / (2.0 * r);
                let w = 1.0 - crate::frames::smoothstep(6.0, 30.0, span);
                if w > 0.0 {
                    let (t, y) = atlas::presentation_angles(&self.u, obj, jd, self.island.as_ref());
                    target = (t * w, crate::frames::shortest_arc(self.cam.pres_yaw, y) * w);
                }
            }
        }
        let k = if self.immediate {
            1.0
        } else {
            1.0 - (-dt / 0.45).exp()
        };
        self.cam.pres_tilt += (target.0 - self.cam.pres_tilt) * k;
        let dy = crate::frames::shortest_arc(self.cam.pres_yaw, target.1) - self.cam.pres_yaw;
        self.cam.pres_yaw += dy * k;
        self.pres_moving = dy.abs() > 1e-4 || (target.0 - self.cam.pres_tilt).abs() > 1e-4;
    }

    fn apply_key(&mut self, key: &PathKey) {
        self.scrolled_out = None;
        if let Some(host) = &key.host {
            if let Some(ei) = self.u.find_system(host) {
                self.focus = Some(ObjectRef::ExoHost(ei));
                self.scene_exo = Some(ei);
            }
        }
        self.follow = None;
        // glide time with the camera instead of jumping
        self.jd_glide = Some(key.jd);
        let mut center = DVec3::new(key.center[0], key.center[1], key.center[2]);
        if let Some(name) = &key.at {
            if let Some(obj) = self.find_object(name) {
                center = obj_pos(&self.u, obj, key.jd, None);
                if key.host.is_none() {
                    self.focus = Some(obj); // anchors refresh focus
                }
            } else if name.eq_ignore_ascii_case("galactic centre")
                || name.eq_ignore_ascii_case("galactic center")
            {
                center = self.u.gal.center;
            } else {
                eprintln!("warning: path key anchor '{name}' not found");
            }
        }
        self.cam.fly_to_view(
            center,
            key.view_w.map_or(key.mpp, |w0| key.mpp * w0 / self.view_w),
            key.tilt,
            key.yaw,
            self.view_w,
            key.dur.max(0.05),
        );
    }

    /// Metres per pixel that frames a half-extent at a comfortable size.
    fn framing_mpp(&self, half_extent: f64) -> f64 {
        half_extent * 2.0 / (FRAME_FILL * self.view_h.min(self.view_w))
    }

    /// Select an object and fly to it, framed.
    pub fn dive_to(&mut self, obj: ObjectRef) {
        if let ObjectRef::ExoHost(ei) | ObjectRef::ExoPlanet(ei, _) = obj {
            if self.u.exos[ei].dist_pc.is_none() {
                self.note("This system has no measured distance, so it can't be placed on the map");
                return;
            }
        }
        let ext = obj_half_extent(&self.u, obj);
        self.fly_to_object(obj, ext);
        let name = obj_name(&self.u, obj);
        self.note(format!("Flying to {name}"));
    }

    /// Frame a planet's whole moon system.
    pub fn show_moons(&mut self, pi: usize) {
        if let Some(ext) = atlas::moon_system_extent(pi) {
            self.fly_to_object(ObjectRef::Planet(pi), ext);
        }
    }

    fn fly_to_object(&mut self, obj: ObjectRef, half_extent: f64) {
        let pos = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
        let mpp = self.framing_mpp(half_extent);
        // already framed here: nothing to come back from
        let view = self.cam.mpp * self.view_w;
        let here = self.cam.anim.is_none()
            && (self.cam.center_root() - pos).length() < 0.02 * view
            && (mpp / self.cam.mpp - 1.0).abs() < 0.05;
        if !here {
            self.push_history();
        }
        self.stop_motion();
        // after the history entry above, which remembers the scene being left
        if let ObjectRef::ExoHost(ei) | ObjectRef::ExoPlanet(ei, _) = obj {
            self.head_for_scene(ei);
        }
        self.focus = Some(obj);
        self.follow = Some((obj, DVec3::ZERO));
        let dur = self.cam.flight_seconds(pos, mpp, self.view_w);
        self.cam.fly_to_view(pos, mpp, 0.0, 0.0, self.view_w, dur);
    }

    /// Search results and rail rungs land here.
    pub fn go_to(&mut self, dest: Dest) {
        match dest {
            Dest::Obj(obj) => self.dive_to(obj),
            Dest::Place {
                center,
                width_m,
                name,
            } => self.fly_to_place(center, width_m, name),
        }
    }

    /// Fly to a location in space at a given view width.
    pub fn fly_to_place(&mut self, center: DVec3, width_m: f64, name: &str) {
        self.push_history();
        self.stop_motion();
        self.follow = None;
        let mpp = width_m / self.view_w;
        let dur = self.cam.flight_seconds(center, mpp, self.view_w);
        self.cam
            .fly_to_view(center, mpp, 0.0, 0.0, self.view_w, dur);
        if !name.is_empty() {
            self.note(format!("Flying to {name}"));
        }
    }

    /// The running simulation, if it's the one this object belongs to.
    pub fn island_of(&self, obj: ObjectRef) -> Option<&Island> {
        let planet = match obj {
            ObjectRef::Planet(i) => i,
            ObjectRef::Moon(mi) => MOONS[mi].planet,
            _ => return None,
        };
        self.island
            .as_ref()
            .filter(|isl| isl.anchor == ObjectRef::Planet(planet))
    }

    pub fn awaken(&mut self) {
        let Some(focus) = self.focus else { return };
        let pi = match focus {
            ObjectRef::Planet(i) => i,
            ObjectRef::Moon(mi) => MOONS[mi].planet,
            _ => {
                self.note("Select a planet or moon first");
                return;
            }
        };
        self.island = Some(build_island(&self.u, pi, self.clock.jd));
        self.modes.living = true;
        self.clock.playing = true;
        // a legible default rate: moons visibly orbit, nothing teleports
        self.clock.rate = 3_600.0;
        self.note(format!(
            "Simulating the {} system with N-body gravity",
            PLANETS[pi].name
        ));
    }

    // ------------------------------------------------------ frame build

    pub fn build_canvas(&mut self, w: f64, h: f64, target: &ViewTarget) {
        let t = self.anim_t;
        self.bank.pump(&self.gpu, t);
        let mut guard = 0;
        let fo = loop {
            self.dl.clear();
            let inp = BuildIn {
                u: &self.u,
                cam: &self.cam,
                clock: &self.clock,
                modes: &self.modes,
                focus: self.focus,
                scene_exo: self.scene_exo,
                island: self.island.as_ref(),
                w,
                h,
                anim_t: self.anim_t,
                ppp: self.ppp,
                tex: &self.bank.status,
            };
            let fo = build(&inp, &mut self.dl);
            for t in std::mem::take(&mut self.dl.want_tex) {
                self.bank.want(t);
            }
            guard += 1;
            // exports wait for imagery so a shot never catches a half-loaded surface
            if self.block_textures && self.bank.busy() && guard < 4 {
                self.bank.finish_loading(&self.gpu);
                continue;
            }
            break fo;
        };
        // keep a copy of pickables for next frame's input hit tests
        self.picks.clear();
        self.picks.extend(fo.picks.iter().map(|p| Pickable {
            pos: p.pos,
            r: p.r,
            obj: p.obj,
        }));
        let uni = fo.uni;
        self.fo = Some(fo);
        self.pipes
            .render(&self.gpu, target, &self.dl, &uni, &mut self.bank);
    }

    // -------------------------------------------------------------- input

    /// What's under a canvas point: a label first (they're the bigger target),
    /// then the nearest body. A click anywhere on a large disc still lands on it.
    fn pick_at(&self, sp: [f32; 2], rect: Rect) -> Option<ObjectRef> {
        let pos = egui::pos2(rect.left() + sp[0], rect.top() + sp[1]);
        if let Some((_, obj)) = self.label_hits.iter().find(|(r, _)| r.contains(pos)) {
            return Some(*obj);
        }
        let mut best: Option<(f32, ObjectRef)> = None;
        for pk in &self.picks {
            let d = (pk.pos[0] - sp[0]).hypot(pk.pos[1] - sp[1]);
            if d < pk.r.max(11.0) {
                // tiny things sitting on big things should win the click
                let score = d / pk.r.max(11.0) + pk.r.min(4000.0) * 1e-4;
                if best.map(|(b, _)| score < b).unwrap_or(true) {
                    best = Some((score, pk.obj));
                }
            }
        }
        best.map(|(_, o)| o)
    }

    pub fn handle_input(&mut self, ctx: &egui::Context, resp: &egui::Response, rect: Rect) {
        let (scroll, zoom_mul, pointer, shift, dt) = ctx.input(|i| {
            (
                i.raw_scroll_delta,
                i.zoom_delta(),
                i.pointer.hover_pos(),
                i.modifiers.shift,
                i.stable_dt as f64,
            )
        });
        let proj = crate::frames::Proj::new(
            &self.cam,
            rect.width() as f64,
            rect.height() as f64,
            &self.u.gal,
        );
        let sp = pointer.map(|p| [p.x - rect.left(), p.y - rect.top()]);

        // ---- hover: name it and show the pointer can act on it
        self.hover = None;
        if resp.hovered() || resp.dragged() {
            if let (Some(s), false) = (sp, resp.dragged()) {
                self.hover = self.pick_at(s, rect);
            }
            ctx.set_cursor_icon(if resp.dragged() {
                egui::CursorIcon::Grabbing
            } else if self.hover.is_some() {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Grab
            });
        }

        // ---- zoom: a notch is ~0.25 decades, Shift for fine control. Pinch too.
        let notch = (scroll.y + if shift { scroll.x } else { 0.0 }) as f64;
        let mut dex = -notch * 0.0032 * if shift { 0.2 } else { 1.0 };
        if zoom_mul != 1.0 {
            dex -= (zoom_mul as f64).log10();
        }
        if dex.abs() > 1e-6 && resp.hovered() {
            self.wheel_zoom(sp, dex, &proj);
        }

        // ---- drag: pan with left (coasts on release); orbit with right or Shift
        let orbiting = resp.dragged_by(egui::PointerButton::Secondary)
            || (shift && resp.dragged_by(egui::PointerButton::Primary));
        if orbiting {
            let d = resp.drag_delta();
            self.cam.tilt = (self.cam.tilt + d.y as f64 * 0.005).clamp(-1.2, 1.45);
            self.cam.yaw -= d.x as f64 * 0.005;
            self.stop_flight_here();
            self.zoom_anchor = None;
            self.scrolled_out = None;
            self.pan_vel = [0.0, 0.0];
        } else if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            self.cam.center -= proj.right * (d.x as f64 * self.cam.mpp);
            self.cam.center += proj.up * (d.y as f64 * self.cam.mpp);
            self.cam.anim = None;
            self.play = None;
            self.pan_vel = [0.0, 0.0];
            self.zoom_pending = 0.0;
            self.zoom_anchor = None;
            self.scrolled_out = None;
            self.sync_follow();
            // track release velocity; ignore frames where the hand stopped
            let inst = [d.x as f64 / dt.max(1e-3), d.y as f64 / dt.max(1e-3)];
            self.drag_vel = [
                self.drag_vel[0] * 0.55 + inst[0] * 0.45,
                self.drag_vel[1] * 0.55 + inst[1] * 0.45,
            ];
            if d != egui::Vec2::ZERO {
                self.last_drag = self.anim_t;
            }
            self.dismiss_intro();
        } else {
            // "still moving at release" must also hold on a slow machine, where a
            // single frame can be longer than this window
            if resp.drag_stopped() && self.anim_t - self.last_drag < 0.08f64.max(2.5 * dt) {
                let speed = self.drag_vel[0].hypot(self.drag_vel[1]);
                if speed > 40.0 {
                    let cap = (2600.0 / speed).min(1.0);
                    self.pan_vel = [self.drag_vel[0] * cap, self.drag_vel[1] * cap];
                }
            }
            self.drag_vel = [0.0, 0.0];
        }

        // ---- click: select. Double-click: fly there, or zoom in on empty space.
        if resp.drag_started() {
            self.pan_vel = [0.0, 0.0];
        }
        if let Some(s) = sp {
            let hit = self.pick_at(s, rect);
            if resp.double_clicked() {
                self.pan_vel = [0.0, 0.0];
                match hit {
                    Some(obj) => {
                        self.learned_click = true;
                        self.dive_to(obj);
                    }
                    None => {
                        let world = proj.unproject(s[0] as f64, s[1] as f64);
                        self.follow = None;
                        self.scrolled_out = None;
                        let mpp = self.cam.mpp / 2.5;
                        self.cam.fly_to(world, mpp, rect.width() as f64, 0.7);
                    }
                }
            } else if resp.clicked() {
                self.click_canvas(hit);
            }
        }

        // ---- keys, suppressed while typing
        self.key_pan = [0.0, 0.0];
        // the palette owns the keyboard while open, even on the frame egui has
        // already dropped text focus because of Esc
        if ctx.wants_keyboard_input() || self.search.open {
            return;
        }
        use egui::Key as K;
        let (mut actions, held) = ctx.input(|i| {
            let mut a: Vec<KeyAction> = Vec::new();
            for (key, act) in [
                (K::Escape, KeyAction::Escape),
                (K::L, KeyAction::TogglePlay),
                (K::Space, KeyAction::TogglePlay),
                (K::T, KeyAction::Truth),
                (K::G, KeyAction::Gravity),
                (K::Y, KeyAction::SystemView),
                (K::H, KeyAction::Home),
                (K::Backspace, KeyAction::Back),
                (K::N, KeyAction::Now),
                (K::M, KeyAction::Ambience),
                (K::F, KeyAction::FlyToSelection),
                (K::Num0, KeyAction::LevelView),
                (K::OpenBracket, KeyAction::Slower),
                (K::CloseBracket, KeyAction::Faster),
            ] {
                if i.key_pressed(key) {
                    a.push(act);
                }
            }
            if i.key_pressed(K::Slash) {
                // "?" is Shift+/ on most layouts
                a.push(if i.modifiers.shift {
                    KeyAction::Help
                } else {
                    KeyAction::Search
                });
            }
            if i.modifiers.command && i.key_pressed(K::K) {
                a.push(KeyAction::Search);
            }
            for e in &i.events {
                if let egui::Event::Text(t) = e {
                    if t == "?" {
                        a.push(KeyAction::Help);
                    }
                }
            }
            let mut held = (0.0f64, 0.0f64, 0.0f64);
            if i.key_down(K::ArrowLeft) || i.key_down(K::A) {
                held.0 += 1.0;
            }
            if i.key_down(K::ArrowRight) || i.key_down(K::D) {
                held.0 -= 1.0;
            }
            if i.key_down(K::ArrowUp) || i.key_down(K::W) {
                held.1 += 1.0;
            }
            if i.key_down(K::ArrowDown) || i.key_down(K::S) {
                held.1 -= 1.0;
            }
            if i.key_down(K::Plus) || i.key_down(K::Equals) {
                held.2 -= 1.0;
            }
            if i.key_down(K::Minus) {
                held.2 += 1.0;
            }
            (a, held)
        });
        self.key_pan = [held.0 * 520.0, held.1 * 520.0];
        if held.2 != 0.0 {
            self.zoom_pending += held.2 * 1.35 * dt;
            let (cx, cy) = (rect.width() as f64 * 0.5, rect.height() as f64 * 0.5);
            let on = self.follow.map(|(obj, _)| obj);
            let middle = proj.unproject_local(cx, cy);
            self.zoom_anchor = Some(self.zoom_point(on, middle, [cx, cy], 1.0, &proj));
            self.scrolled_out = None;
            self.cam.anim = None;
            self.dismiss_intro();
        }
        actions.dedup_by(|a, b| std::mem::discriminant(a) == std::mem::discriminant(b));
        for act in actions {
            self.key_action(act);
        }
    }

    fn key_action(&mut self, act: KeyAction) {
        match act {
            KeyAction::Escape => {
                if self.show_help {
                    self.show_help = false;
                } else if self.show_layers {
                    self.show_layers = false;
                } else {
                    self.focus = None;
                    self.stop_following();
                }
            }
            KeyAction::TogglePlay => {
                self.clock.playing = !self.clock.playing;
                self.modes.living = self.clock.playing;
            }
            KeyAction::Truth => self.modes.truth = !self.modes.truth,
            KeyAction::Gravity => {
                if matches!(
                    self.focus,
                    Some(ObjectRef::Planet(_)) | Some(ObjectRef::Moon(_))
                ) {
                    self.modes.gravity = !self.modes.gravity;
                } else {
                    self.modes.gravity = false;
                    self.note("Select a planet or moon to see its gravity field");
                }
            }
            KeyAction::SystemView => {
                if self.scene_exo.is_some() {
                    self.modes.system_view = !self.modes.system_view;
                } else {
                    self.note("System view is for exoplanet systems: search for one to fly there");
                }
            }
            KeyAction::Search => self.search.open_palette(),
            KeyAction::Help => self.show_help = !self.show_help,
            KeyAction::Home => self.go_home(),
            KeyAction::Back => self.go_back(),
            KeyAction::Now => {
                self.clock.jd = jd_now();
                self.note("Back to the present moment");
            }
            KeyAction::Ambience => self.toggle_ambience(),
            KeyAction::FlyToSelection => {
                if let Some(obj) = self.focus {
                    self.dive_to(obj);
                }
            }
            KeyAction::LevelView => {
                let (c, mpp) = (self.cam.center_root(), self.cam.mpp);
                self.cam.fly_to_view(c, mpp, 0.0, 0.0, self.view_w, 0.5);
            }
            KeyAction::Slower => self.step_rate(-1),
            KeyAction::Faster => self.step_rate(1),
        }
    }

    pub fn toggle_ambience(&mut self) {
        if self.ambience_available == Some(false) {
            self.note("No audio output was found, so ambience is unavailable");
            return;
        }
        if !cfg!(feature = "audio") {
            self.note("This build has no audio support");
            return;
        }
        self.settings.ambience_on = !self.settings.ambience_on;
        self.save_settings();
    }

    pub fn step_rate(&mut self, dir: i32) {
        let i = ui::RATES
            .iter()
            .position(|(_, r)| (self.clock.rate - r).abs() / r < 0.01)
            .unwrap_or(3) as i32;
        let n = (i + dir).clamp(0, ui::RATES.len() as i32 - 1) as usize;
        self.clock.rate = ui::RATES[n].1;
    }

    fn sync_follow(&mut self) {
        if let Some((obj, _)) = self.follow {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            let off = self.cam.center_root() - p;
            // release once the subject is far outside the view
            if off.length() > 12.0 * self.cam.mpp * self.view_w {
                self.follow = None;
            } else {
                self.follow = Some((obj, off));
            }
        }
    }

    /// True while something on screen is moving and needs every frame drawn.
    pub fn animating(&self) -> bool {
        self.cam.anim.is_some()
            || self.zoom_pending != 0.0
            || self.pan_vel != [0.0, 0.0]
            || self.key_pan != [0.0, 0.0]
            || self.scrub.is_some()
            || self.clock.playing
            || self.bank.busy()
            || self.play.is_some()
            || self.bank.status.fade.iter().any(|f| *f > 0.0 && *f < 1.0)
            || self.pres_moving
    }

    // ----------------------------------------------------------- overlay

    pub fn draw_overlay(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        let Some(fo) = &self.fo else { return };
        // labels with collision skip; last frame's winners get a hysteresis
        // bonus so labels don't flicker as the camera drifts
        let mut order: Vec<usize> = (0..fo.labels.len()).collect();
        order.sort_by_key(|&i| {
            -(fo.labels[i].priority
                + if self.sticky.contains(&fo.labels[i].text) {
                    12
                } else {
                    0
                })
        });
        // panels from last frame count as taken: no names hiding behind the UI
        let mut placed: Vec<Rect> = self.panel_rects.clone();
        let mut placed_texts: std::collections::HashSet<String> = Default::default();
        let mut hits: Vec<(Rect, ObjectRef)> = Vec::new();
        let show = self.modes.labels;
        for i in order {
            let l = &fo.labels[i];
            if !show && l.priority < 90 {
                continue;
            }
            if l.color[3] < 0.02 {
                continue;
            }
            let pos = egui::pos2(rect.left() + l.pos[0], rect.top() + l.pos[1]);
            let w = l.text.chars().count() as f32 * l.size * 0.5 + 6.0;
            let r = Rect::from_center_size(pos, egui::vec2(w, l.size + 5.0));
            if !rect.intersects(r) || placed.iter().any(|p| p.intersects(r)) {
                continue;
            }
            placed.push(r);
            placed_texts.insert(l.text.clone());
            if let Some(o) = l.obj {
                hits.push((r, o));
            }
            let hovered = l.obj.is_some() && l.obj == self.hover;
            let col = if hovered {
                theme::BRASS
            } else {
                theme::col32([l.color[0], l.color[1], l.color[2]], l.color[3])
            };
            let font = theme::serif(l.size);
            ui::halo_text(
                &painter,
                pos,
                Align2::CENTER_CENTER,
                &l.text,
                font,
                col,
                l.color[3],
            );
        }
        self.sticky = placed_texts;
        self.label_hits = hits;

        let proj = crate::frames::Proj::new(
            &self.cam,
            rect.width() as f64,
            rect.height() as f64,
            &self.u.gal,
        );
        // selection and hover: thin rings hugging the object, whatever its size
        let body_r = |this: &Self, obj: ObjectRef| -> f32 {
            this.picks
                .iter()
                .find(|p| p.obj == obj)
                .map(|p| p.r)
                .unwrap_or(8.0)
        };
        if let Some(obj) = self.hover {
            if Some(obj) != self.focus {
                let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
                let s = proj.px(p);
                let c = egui::pos2(rect.left() + s[0], rect.top() + s[1]);
                let r = body_r(self, obj).max(6.0) + 5.0;
                if r < 90.0 {
                    painter.circle_stroke(
                        c,
                        r,
                        egui::Stroke::new(1.0, theme::col32(theme::INK_WHITE, 0.45)),
                    );
                }
            }
        }
        if let Some(obj) = self.focus {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            let s = proj.px(p);
            let c = egui::pos2(rect.left() + s[0], rect.top() + s[1]);
            let r = body_r(self, obj).max(7.0) + 8.0;
            // no reticle once the body is large: it's plainly the subject
            if rect.expand(r).contains(c) && r < 70.0 {
                let gap = 0.42f32;
                for q in 0..4 {
                    let a0 = q as f32 * std::f32::consts::FRAC_PI_2 + gap;
                    let a1 = (q + 1) as f32 * std::f32::consts::FRAC_PI_2 - gap;
                    let pts: Vec<egui::Pos2> = (0..=10)
                        .map(|k| {
                            let a = a0 + (a1 - a0) * k as f32 / 10.0;
                            egui::pos2(c.x + r * a.cos(), c.y + r * a.sin())
                        })
                        .collect();
                    painter.add(egui::Shape::line(pts, egui::Stroke::new(1.2, theme::BRASS)));
                }
            }
        }
    }

    // ------------------------------------------------------------ panels

    fn card_data(&self, obj: ObjectRef) -> CardData {
        use crate::evidence::Ev;
        let u = &self.u;
        let jd = self.clock.jd;
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut provs: Vec<(String, Ev)> = Vec::new();
        let mut can_awaken = false;
        let title = obj_name(u, obj);
        let subtitle;
        match obj {
            ObjectRef::Sun => {
                subtitle = "Our star, type G2V".into();
                rows.push(("radius".into(), "695,700 km".into()));
                rows.push(("GM".into(), "1.327e20 m³/s²".into()));
                provs.push(("physical constants".into(), Ev::PhysicalConstants));
                if self
                    .bank
                    .status
                    .on_disk(crate::engine::textures::TexId::Sun)
                {
                    provs.push(("surface map".into(), Ev::SurfaceImagery));
                }
                provs.push(("corona and surface motion".into(), Ev::CoronaIllustrative));
            }
            ObjectRef::Planet(i) => {
                let p = &PLANETS[i];
                subtitle = "Planet of the Solar System".into();
                let pos = obj_pos(u, obj, jd, self.island.as_ref());
                rows.push(("distance from Sun".into(), fmt_len(pos.length())));
                rows.push(("semi-major axis".into(), format!("{:.3} AU", p.el[0])));
                rows.push(("eccentricity".into(), format!("{:.4}", p.el[1])));
                rows.push(("period".into(), format!("{:.2} yr", p.el[0].powf(1.5))));
                rows.push(("radius".into(), format!("{:.0} km", p.radius_km)));
                rows.push(("day length".into(), format!("{:.2} h", p.rot_hours.abs())));
                if let Some(res) = u.planet_residual_km(i, jd) {
                    let txt = if res < 10.0 {
                        format!("{res:.2} km")
                    } else {
                        format!("{res:.0} km")
                    };
                    rows.push(("difference from JPL Horizons".into(), txt));
                    provs.push(("Horizons cross-check".into(), Ev::HorizonsVectors));
                    provs.push(("residual interpolation".into(), Ev::HorizonsInterp));
                }
                if let Some(isl) = self.island_of(obj) {
                    let what = if isl.ids.len() > 2 { "moon" } else { "orbit" };
                    rows.push((
                        format!("{what} energy drift, largest so far"),
                        format!("{:.2}%", isl.peak_drift * 100.0),
                    ));
                    if isl.coarse_frames > 0 {
                        rows.push((
                            "coarser steps".into(),
                            format!(
                                "{} slow frame{}, up to {:.1}x",
                                isl.coarse_frames,
                                if isl.coarse_frames == 1 { "" } else { "s" },
                                isl.coarsest
                            ),
                        ));
                    }
                    provs.push(("island diagnostics".into(), Ev::NBodyIsland));
                }
                if sim::eph_is_spice(jd) {
                    provs.push(("state".into(), Ev::De440));
                    provs.push(("orbit engraving".into(), Ev::ApproxElements));
                } else {
                    provs.push(("state + orbit".into(), Ev::ApproxElements));
                }
                let has_map =
                    atlas::planet_tex(p.name).is_some_and(|t| self.bank.status.on_disk(t));
                provs.push((
                    "surface".into(),
                    if has_map {
                        Ev::SurfaceImagery
                    } else {
                        Ev::ProceduralSurface
                    },
                ));
                provs.push(("orientation".into(), Ev::IauRotation));
                provs.push(("atmosphere glow".into(), Ev::AtmosphereGlow));
                if i == 2 {
                    provs.push(("coastlines (Truth mode)".into(), Ev::Coastlines));
                    if !self
                        .bank
                        .status
                        .on_disk(crate::engine::textures::TexId::EarthNight)
                    {
                        provs.push(("night lights".into(), Ev::LandLights));
                    }
                }
                can_awaken = true;
            }
            ObjectRef::Moon(mi) => {
                let m = &MOONS[mi];
                subtitle = format!("Moon of {}", PLANETS[m.planet].name);
                rows.push(("semi-major axis".into(), format!("{:.0} km", m.a_km)));
                rows.push(("period".into(), format!("{:.3} d", m.period_d.abs())));
                rows.push(("radius".into(), format!("{:.0} km", m.radius_km)));
                rows.push(("eccentricity".into(), format!("{:.4}", m.e)));
                if sim::MOON_INCL[mi] > 90.0 {
                    rows.push(("orbit".into(), "retrograde".into()));
                }
                if sim::MOON_INCL[mi] > 0.5 {
                    rows.push((
                        "inclination to equator".into(),
                        format!("{:.1}°", sim::MOON_INCL[mi]),
                    ));
                }
                if sim::moon_is_spice(mi, jd) {
                    provs.push((
                        "state".into(),
                        if m.name == "Moon" {
                            Ev::De440
                        } else {
                            Ev::SatelliteSpk
                        },
                    ));
                } else if m.name == "Moon" {
                    provs.push(("state".into(), Ev::LunaModel));
                } else {
                    provs.push(("state".into(), Ev::MoonMeanElements));
                    provs.push(("orbital phase".into(), Ev::MoonPhase));
                }
                let has_map = m.name == "Moon"
                    && self
                        .bank
                        .status
                        .on_disk(crate::engine::textures::TexId::Moon);
                provs.push((
                    "surface".into(),
                    if has_map {
                        Ev::SurfaceImagery
                    } else {
                        Ev::ProceduralSurface
                    },
                ));
                // the same condition as drawing it
                if m.name == "Moon" && u.moon_truth.as_ref().is_some_and(|t| t.covers(jd, 5.0)) {
                    provs.push(("dashed arc (Truth mode)".into(), Ev::HorizonsVectors));
                }
                can_awaken = true;
            }
            ObjectRef::Craft(ci) => {
                let c = &u.craft[ci];
                subtitle = format!("Spacecraft: {}", c.note);
                let pos = obj_pos(u, obj, jd, None);
                rows.push((
                    "distance from Sun".into(),
                    format!("{:.2} AU", pos.length() / AU),
                ));
                let (jd0, jd1) = c.track.span();
                rows.push((
                    "trajectory arc".into(),
                    format!("{} → {}", &fmt_jd(jd0)[..11], &fmt_jd(jd1)[..11]),
                ));
                provs.push(("trajectory".into(), Ev::HorizonsVectors));
                provs.push(("between samples".into(), Ev::HorizonsInterp));
            }
            ObjectRef::Star(si) => {
                let s = &u.stars[si];
                subtitle = if s.compiled {
                    "Star, from a compiled catalog".into()
                } else {
                    "Star, from Gaia DR3".into()
                };
                let dist = if s.dist_err_pc > 0.0 {
                    format!("{:.2} ± {:.2} pc", s.dist_pc, s.dist_err_pc)
                } else {
                    format!("{:.1} light-years", s.dist_pc * 3.2616)
                };
                rows.push(("distance".into(), dist));
                rows.push(("G magnitude".into(), format!("{:.2}", s.gmag)));
                rows.push(("absolute mag".into(), format!("{:.2}", s.absmag)));
                rows.push(("BP–RP colour".into(), format!("{:.2}", s.bp_rp)));
                let kms = s.drift.length() / 3.155_76e10; // m/yr → km/s
                rows.push(("space motion".into(), format!("{kms:.1} km/s")));
                provs.push((
                    "astrometry".into(),
                    if s.compiled {
                        Ev::GaiaCompiled
                    } else {
                        Ev::GaiaDr3
                    },
                ));
                provs.push(("position at this epoch".into(), Ev::ProperMotion));
            }
            ObjectRef::ExoHost(ei) => {
                let s = &u.exos[ei];
                subtitle = format!("Star with {} known planets", s.planets.len());
                if let Some(d) = s.dist_pc {
                    rows.push(("distance".into(), ui::fmt_words(d * crate::frames::PC)));
                }
                if let Some(t) = s.st_teff {
                    rows.push(("T_eff".into(), format!("{t:.0} K")));
                }
                if let Some(r) = s.st_rad {
                    rows.push(("radius".into(), format!("{r:.2} R☉")));
                }
                if let Some(m) = s.st_mass {
                    rows.push(("mass".into(), format!("{m:.2} M☉")));
                }
                if !s.spec.is_empty() {
                    rows.push(("spectral type".into(), s.spec.clone()));
                }
                provs.push(("stellar parameters".into(), Ev::ExoArchive));
                provs.push(("orbit geometry".into(), Ev::ExoGeometry));
            }
            ObjectRef::ExoPlanet(ei, k) => {
                let s = &u.exos[ei];
                let p = &s.planets[k];
                subtitle = format!(
                    "Exoplanet, found by {}{}",
                    p.method.to_lowercase(),
                    p.year.map(|y| format!(" in {y}")).unwrap_or_default()
                );
                if let Some(per) = p.per_d {
                    rows.push(("period".into(), format!("{per:.3} d")));
                    provs.push(("period".into(), Ev::ExoArchive));
                }
                match s.a_au_of(p) {
                    Some((a, Prov::Measured)) => {
                        rows.push(("semi-major axis".into(), format!("{a:.4} AU")));
                    }
                    Some((a, _)) => {
                        rows.push((
                            "semi-major axis".into(),
                            format!("{a:.4} AU (from Kepler's 3rd)"),
                        ));
                        provs.push(("semi-major axis".into(), Ev::ExoDerivedSemiMajor));
                    }
                    None => {
                        rows.push(("semi-major axis".into(), "unknown".into()));
                        provs.push(("semi-major axis".into(), Ev::ExoUnknownValue));
                    }
                }
                match p.rade {
                    Some(r) => rows.push(("radius".into(), format!("{r:.2} R⊕"))),
                    None => {
                        rows.push(("radius".into(), "unknown".into()));
                        provs.push(("radius".into(), Ev::ExoUnknownValue));
                    }
                }
                if let Some(m) = p.masse {
                    rows.push(("mass".into(), format!("{m:.1} M⊕")));
                }
                if let Some(t) = p.eqt {
                    rows.push(("equilibrium T".into(), format!("{t:.0} K")));
                }
                if p.e.is_none() {
                    provs.push(("eccentricity assumed 0".into(), Ev::ExoDerivedSemiMajor));
                }
                provs.push(("orbital phase".into(), Ev::ExoPhase));
            }
            ObjectRef::Galaxy(gi) => {
                let g = &u.galaxies[gi];
                subtitle = if g.spiral {
                    "Spiral galaxy in the Local Group".into()
                } else {
                    "Dwarf galaxy in the Local Group".into()
                };
                rows.push((
                    "distance".into(),
                    ui::fmt_words(g.dist_kpc * crate::frames::KPC),
                ));
                rows.push((
                    "size".into(),
                    format!("about {}", ui::fmt_words(g.size_kpc * crate::frames::KPC)),
                ));
                provs.push(("position & distance".into(), Ev::CompiledCatalog));
                provs.push(("appearance".into(), Ev::GlyphIllustrative));
            }
            ObjectRef::Node(ni) => {
                let n = &u.nodes[ni];
                subtitle = "Galaxy cluster or attractor".into();
                rows.push((
                    "distance".into(),
                    ui::fmt_words(n.dist_mpc * crate::frames::MPC),
                ));
                rows.push((
                    "size".into(),
                    format!("about {}", ui::fmt_words(n.size_mpc * crate::frames::MPC)),
                ));
                provs.push(("position & distance".into(), Ev::CompiledCatalog));
                provs.push(("web filaments".into(), Ev::WebProcedural));
            }
        }
        (title, subtitle, rows, provs, can_awaken)
    }
}

// ------------------------------------------------------------- GUI shell

pub struct GuiApp {
    app: Pantheon,
    rs: eframe::egui_wgpu::RenderState,
    canvas: Option<(ViewTarget, egui::TextureId)>,
    last: Instant,
    /// the opening view is framed once the real window size is known
    framed: bool,
    #[cfg(feature = "audio")]
    ambience: Option<crate::audio::Ambience>,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("wgpu backend required")
            .clone();
        let gpu = GpuContext::from_eframe(&rs);
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let mut app = Pantheon::new(gpu);
        app.attach_settings(Settings::load());
        Self {
            app,
            rs,
            canvas: None,
            last: Instant::now(),
            framed: false,
            #[cfg(feature = "audio")]
            ambience: None,
        }
    }

    /// The audio device is opened the first time someone asks for sound, never before.
    #[cfg(feature = "audio")]
    fn drive_audio(&mut self) {
        let want = self.app.settings.ambience_on;
        if want && self.ambience.is_none() && self.app.ambience_available != Some(false) {
            self.ambience = crate::audio::Ambience::start(self.app.settings.ambience_vol);
            self.app.ambience_available = Some(self.ambience.is_some());
            if self.ambience.is_none() {
                self.app.settings.ambience_on = false;
                self.app.status = Some((
                    "No audio output was found, so ambience is unavailable".into(),
                    self.app.anim_t + 6.0,
                ));
            }
        }
        if let Some(a) = &self.ambience {
            a.set_gain(if want {
                self.app.settings.ambience_vol
            } else {
                0.0
            });
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // a real long stall should not fast-forward the camera; a slow machine
        // should still finish a flight in about the time it was meant to take
        let dt = self.last.elapsed().as_secs_f64().min(0.25);
        self.last = Instant::now();
        #[cfg(feature = "audio")]
        self.drive_audio();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let ppp = ctx.pixels_per_point();
                // the canvas renders in physical pixels; all
                // geometry stays in logical points, the shaders know the scale
                let (w, h) = (
                    ((rect.width() * ppp).round() as u32).max(64),
                    ((rect.height() * ppp).round() as u32).max(64),
                );
                let recreate = self
                    .canvas
                    .as_ref()
                    .map(|(t, _)| t.width != w || t.height != h)
                    .unwrap_or(true);
                if recreate {
                    let target = ViewTarget::new_hdr(&self.app.gpu, w, h);
                    let mut rend = self.rs.renderer.write();
                    if let Some((_, old)) = self.canvas.take() {
                        rend.free_texture(&old);
                    }
                    let id = rend.register_native_texture(
                        &self.app.gpu.device,
                        &target.resolve_view,
                        wgpu::FilterMode::Nearest,
                    );
                    self.canvas = Some((target, id));
                }
                self.app.view_w = rect.width() as f64;
                self.app.view_h = rect.height() as f64;
                self.app.ppp = ppp;
                if !self.framed {
                    self.framed = true;
                    self.app.cam.mpp = self
                        .app
                        .framing_mpp(atlas::obj_half_extent(&self.app.u, ObjectRef::Planet(2)));
                }
                let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                self.app.handle_input(ctx, &resp, rect);
                self.app.tick(dt);
                let (target, tex) = self.canvas.as_ref().unwrap();
                self.app
                    .build_canvas(rect.width() as f64, rect.height() as f64, target);
                ui.painter().image(
                    *tex,
                    rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                self.app.draw_overlay(ui, rect);
                self.app.draw_panels(ctx, rect);
            });
        if self.app.animating() {
            ctx.request_repaint();
        } else {
            // the Sun's surface and the clouds still drift, but gently
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }
}

pub fn run_gui() -> anyhow::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1680.0, 980.0])
        .with_min_inner_size([560.0, 380.0])
        .with_app_id("pantheon")
        .with_title("Pantheon: an atlas of the universe");
    // window/taskbar icon from the generated atlas emblem
    let icon_path = crate::data::asset_root().join("icon.png");
    if let Ok(img) = image::open(&icon_path) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        viewport = viewport.with_icon(std::sync::Arc::new(egui::IconData {
            rgba: rgba.into_raw(),
            width: w,
            height: h,
        }));
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Pantheon",
        options,
        Box::new(|cc| Ok(Box::new(GuiApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

// --------------------------------------------------------- headless host

pub struct Headless {
    pub app: Pantheon,
    ctx: egui::Context,
    rend: eframe::egui_wgpu::Renderer,
    canvas: Option<(ViewTarget, egui::TextureId)>,
    // persistent final compositing target (no per-frame allocs)
    final_target: Option<ViewTarget>,
}

impl Headless {
    pub fn new() -> anyhow::Result<Self> {
        let gpu = GpuContext::headless()?;
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::apply(&ctx);
        let rend = eframe::egui_wgpu::Renderer::new(
            &gpu.device,
            crate::engine::gpu::FORMAT,
            None,
            crate::engine::gpu::SAMPLES,
            false,
        );
        let mut app = Pantheon::new(gpu);
        app.immediate = true;
        app.block_textures = true;
        app.status = None;
        Ok(Self {
            app,
            ctx,
            rend,
            canvas: None,
            final_target: None,
        })
    }

    /// Advance dt and render one full frame (canvas + UI) at (w, h).
    /// `read = false` skips the GPU readback (settle frames).
    pub fn frame(&mut self, w: u32, h: u32, dt: f64, read: bool) -> anyhow::Result<Vec<u8>> {
        self.app.view_w = w as f64;
        self.app.view_h = h as f64;
        self.app.tick(dt);
        let recreate = self
            .canvas
            .as_ref()
            .map(|(t, _)| t.width != w || t.height != h)
            .unwrap_or(true);
        if recreate {
            let target = ViewTarget::new_hdr(&self.app.gpu, w, h);
            if let Some((_, old)) = self.canvas.take() {
                self.rend.free_texture(&old);
            }
            let id = self.rend.register_native_texture(
                &self.app.gpu.device,
                &target.resolve_view,
                wgpu::FilterMode::Linear,
            );
            self.canvas = Some((target, id));
        }
        let (target, tex) = self.canvas.as_ref().unwrap();
        let tex = *tex;
        self.app.build_canvas(w as f64, h as f64, target);

        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(w as f32, h as f32),
            )),
            time: Some(self.app.anim_t),
            ..Default::default()
        };
        let app = &mut self.app;
        let full = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    ui.painter().image(
                        tex,
                        rect,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    app.draw_overlay(ui, rect);
                    app.draw_panels(ctx, rect);
                });
        });
        let jobs = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        for (id, delta) in &full.textures_delta.set {
            self.rend
                .update_texture(&self.app.gpu.device, &self.app.gpu.queue, *id, delta);
        }
        let screen = eframe::egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: 1.0,
        };
        let recreate_final = self
            .final_target
            .as_ref()
            .map(|t| t.width != w || t.height != h)
            .unwrap_or(true);
        if recreate_final {
            self.final_target = Some(ViewTarget::new(&self.app.gpu, w, h));
        }
        let final_target = self.final_target.as_ref().unwrap();
        let mut encoder = self
            .app
            .gpu
            .device
            .create_command_encoder(&Default::default());
        let cmds = self.rend.update_buffers(
            &self.app.gpu.device,
            &self.app.gpu.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            // egui's pipelines declare no depth state -> depth-less pass
            let mut pass = final_target
                .begin_no_depth(&mut encoder, Some(wgpu::Color::BLACK))
                .forget_lifetime();
            self.rend.render(&mut pass, &jobs, &screen);
        }
        self.app
            .gpu
            .queue
            .submit(cmds.into_iter().chain([encoder.finish()]));
        for id in &full.textures_delta.free {
            self.rend.free_texture(id);
        }
        if read {
            self.final_target
                .as_ref()
                .unwrap()
                .read_pixels(&self.app.gpu)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn export_png(&mut self, w: u32, h: u32, path: &std::path::Path) -> anyhow::Result<()> {
        // settle frames (font atlas, imagery fades, presentation) without readback
        for _ in 0..3 {
            let _ = self.frame(w, h, 0.05, false)?;
        }
        let px = self.frame(w, h, 0.05, true)?;
        save_png(path, w, h, &px)
    }
}

// ------------------------------------------------------------- shots

pub struct Shot {
    pub name: &'static str,
    pub desc: &'static str,
    pub setup: fn(&mut Pantheon),
}

fn exo_view(app: &mut Pantheon, name: &str, zoom_out: f64) {
    if let Some(ei) = app.u.find_system(name) {
        let pos = app.u.exos[ei].pos;
        let ext = obj_half_extent(&app.u, ObjectRef::ExoHost(ei));
        view(app, pos, ext * zoom_out, Some(ObjectRef::ExoHost(ei)));
    } else {
        app.setup_error = Some(format!("no exoplanet system matches '{name}'"));
    }
}

fn view(app: &mut Pantheon, center: DVec3, width_m: f64, focus: Option<ObjectRef>) {
    app.cam.center = center;
    app.cam.mpp = width_m / 1600.0; // rescaled to the real export width in run_shots
    app.cam.anim = None;
    app.cam.tilt = 0.0;
    app.cam.yaw = 0.0;
    app.focus = focus;
    if let Some(ObjectRef::ExoHost(ei)) | Some(ObjectRef::ExoPlanet(ei, _)) = focus {
        app.scene_exo = Some(ei);
    }
    // track the subject WITHOUT recentring on it (the offset
    // preserves the shot's declared composition)
    app.follow = focus.map(|o| (o, center - obj_pos(&app.u, o, app.clock.jd, None)));
}

pub const SHOTS: &[Shot] = &[
    Shot {
        name: "earth",
        desc: "Earth close-up: imagery, night lights, clouds, atmosphere",
        setup: |a| {
            let p = sim::body_pos(2, a.clock.jd);
            view(a, p, 3.4e7, Some(ObjectRef::Planet(2)));
        },
    },
    Shot {
        name: "earth_moon",
        desc: "Earth–Moon system with measured lunar arc",
        setup: |a| {
            let p = sim::body_pos(2, a.clock.jd);
            view(a, p, 9.5e8, Some(ObjectRef::Planet(2)));
        },
    },
    Shot {
        name: "gravity_moon",
        desc: "Gravity view: Earth–Moon Lagrange points and Hill sphere",
        setup: |a| {
            let p = sim::body_pos(2, a.clock.jd);
            view(a, p, 1.35e9, Some(ObjectRef::Moon(0)));
            a.modes.gravity = true;
        },
    },
    Shot {
        name: "inner",
        desc: "Inner solar system orbit engravings",
        setup: |a| view(a, DVec3::ZERO, 1.15e12, None),
    },
    Shot {
        name: "outer",
        desc: "Outer system with spacecraft trails",
        setup: |a| view(a, DVec3::ZERO, 1.4e13, None),
    },
    Shot {
        name: "heliosphere",
        desc: "Heliosphere: termination shock, heliopause, Voyagers",
        setup: |a| view(a, DVec3::ZERO, 6.5e13, None),
    },
    Shot {
        name: "oort",
        desc: "Oort cloud shell",
        setup: |a| view(a, DVec3::ZERO, 4.5e15, None),
    },
    Shot {
        name: "mars",
        desc: "Mars close-up: polar caps, cratered surface",
        setup: |a| {
            let p = sim::body_pos(3, a.clock.jd);
            view(a, p, 1.9e7, Some(ObjectRef::Planet(3)));
        },
    },
    Shot {
        name: "luna",
        desc: "The Moon close-up: maria and craters",
        setup: |a| {
            let p = obj_pos(&a.u, ObjectRef::Moon(0), a.clock.jd, None);
            view(a, p, 1.0e7, Some(ObjectRef::Moon(0)));
        },
    },
    Shot {
        name: "galaxy_close",
        desc: "Galaxy at 3 kpc: glow resolving into individual stars",
        setup: |a| {
            let gc = crate::frames::galactic_frame().center;
            view(a, gc * 0.5, 2.0e20, None);
        },
    },
    Shot {
        name: "jupiter",
        desc: "Jupiter, close",
        setup: |a| {
            let p = sim::body_pos(4, a.clock.jd);
            view(a, p, 4.0e8, Some(ObjectRef::Planet(4)));
        },
    },
    Shot {
        name: "jupiter_moons",
        desc: "Jupiter and the Galilean moons",
        setup: |a| {
            let p = sim::body_pos(4, a.clock.jd);
            view(a, p, 5.0e9, Some(ObjectRef::Planet(4)));
        },
    },
    Shot {
        name: "saturn_moons",
        desc: "Saturn and its moon system",
        setup: |a| {
            let p = sim::body_pos(5, a.clock.jd);
            view(a, p, 3.6e9, Some(ObjectRef::Planet(5)));
        },
    },
    Shot {
        name: "sun",
        desc: "The Sun: surface map, limb, corona",
        setup: |a| view(a, DVec3::ZERO, 4.4e9, Some(ObjectRef::Sun)),
    },
    Shot {
        name: "venus",
        desc: "Venus, cloud-covered",
        setup: |a| {
            let p = sim::body_pos(1, a.clock.jd);
            view(a, p, 3.2e7, Some(ObjectRef::Planet(1)));
        },
    },
    Shot {
        name: "mercury",
        desc: "Mercury",
        setup: |a| {
            let p = sim::body_pos(0, a.clock.jd);
            view(a, p, 1.3e7, Some(ObjectRef::Planet(0)));
        },
    },
    Shot {
        name: "uranus",
        desc: "Uranus and its thin ring",
        setup: |a| {
            let p = sim::body_pos(6, a.clock.jd);
            view(a, p, 2.4e8, Some(ObjectRef::Planet(6)));
        },
    },
    Shot {
        name: "neptune",
        desc: "Neptune",
        setup: |a| {
            let p = sim::body_pos(7, a.clock.jd);
            view(a, p, 1.4e8, Some(ObjectRef::Planet(7)));
        },
    },
    Shot {
        name: "saturn",
        desc: "Saturn and its rings, with shadows",
        setup: |a| {
            let p = sim::body_pos(5, a.clock.jd);
            view(a, p, 6.5e8, Some(ObjectRef::Planet(5)));
        },
    },
    Shot {
        name: "stars",
        desc: "Gaia stellar neighbourhood (~20 pc)",
        setup: |a| view(a, DVec3::ZERO, 6.2e17, None),
    },
    Shot {
        name: "starfield",
        desc: "Star field and exoplanet hosts",
        setup: |a| view(a, DVec3::ZERO, 6.0e16, None),
    },
    Shot {
        name: "neighbourhood",
        desc: "The edge of the dense Gaia sample, about 35 pc out",
        setup: |a| view(a, DVec3::ZERO, 3.0e18, None),
    },
    Shot {
        name: "galactic_centre",
        desc: "Zoomed in near the galactic centre",
        setup: |a| {
            let gc = crate::frames::galactic_frame().center;
            view(a, gc, 6.0e19, None);
        },
    },
    Shot {
        name: "galaxy",
        desc: "The Milky Way: fog, arms, Sol marker",
        setup: |a| {
            let gc = crate::frames::galactic_frame().center;
            view(a, gc, 2.6e21, None);
        },
    },
    Shot {
        name: "localgroup",
        desc: "Local Group: M31, M33, dwarf satellites",
        setup: |a| view(a, DVec3::ZERO, 7.0e22, None),
    },
    Shot {
        name: "cosmic",
        desc: "Cosmic web with named attractors",
        setup: |a| view(a, DVec3::ZERO, 8.0e24, None),
    },
    Shot {
        name: "trappist1",
        desc: "Dive: TRAPPIST-1 seven-planet system",
        setup: |a| exo_view(a, "TRAPPIST-1", 2.6),
    },
    Shot {
        name: "kepler90",
        desc: "Dive: Kepler-90, eight planets",
        setup: |a| exo_view(a, "Kepler-90", 2.6),
    },
    Shot {
        name: "truth_earth",
        desc: "Truth mode over Earth: coastlines checking the map's alignment",
        setup: |a| {
            let p = sim::body_pos(2, a.clock.jd);
            view(a, p, 3.4e7, Some(ObjectRef::Planet(2)));
            a.modes.truth = true;
        },
    },
    Shot {
        name: "truth_inner",
        desc: "Truth mode over the inner system",
        setup: |a| {
            view(a, DVec3::ZERO, 1.15e12, Some(ObjectRef::Planet(2)));
            a.modes.truth = true;
        },
    },
    Shot {
        name: "system_trappist",
        desc: "System View: TRAPPIST-1 log-spaced schematic",
        setup: |a| {
            if let Some(&ei) = a.u.exo_by_host.get("TRAPPIST-1") {
                let pos = a.u.exos[ei].pos;
                view(a, pos, 0.9e11, Some(ObjectRef::ExoHost(ei)));
                a.modes.system_view = true;
            }
        },
    },
];

pub fn run_shots(which: &str, w: u32, h: u32, out: &std::path::Path) -> anyhow::Result<()> {
    let mut host = Headless::new()?;
    host.app.view_w = w as f64;
    host.app.view_h = h as f64;
    let mut done = 0;
    let mut failures: Vec<String> = Vec::new();
    let wanted: Vec<&str> = which.split(',').map(str::trim).collect();
    for shot in SHOTS {
        if which != "all" && !wanted.contains(&shot.name) {
            continue;
        }
        // reset ALL transient state between shots, deterministic procedural
        // time included
        reset_scene(&mut host);
        (shot.setup)(&mut host.app);
        if let Some(e) = host.app.setup_error.take() {
            failures.push(format!("{}: {e}", shot.name));
            continue;
        }
        // shots are framed for a 1600 px reference width; rescale so the
        // physical view width is resolution-independent
        host.app.cam.mpp *= 1600.0 / w as f64;
        let path = out.join(format!("{}.png", shot.name));
        host.export_png(w, h, &path)?;
        println!("wrote {}", path.display());
        done += 1;
    }
    anyhow::ensure!(done > 0, "no shot named '{which}' (try --list-shots)");
    anyhow::ensure!(
        failures.is_empty(),
        "shot setup failed: {}",
        failures.join("; ")
    );
    Ok(())
}

/// Reset the transient state between scenes, so each is deterministic.
fn reset_scene(host: &mut Headless) {
    host.app.modes = Modes::default();
    host.app.island = None;
    host.app.play = None;
    host.app.focus = None;
    host.app.scene_exo = None;
    host.app.follow = None;
    host.app.cam = Camera::new();
    host.app.anim_t = 0.0;
    host.app.setup_error = None;
}

/// One continuous zoom from Earth to the cosmic web, a frame per half decade:
/// the quick way to look for dead zones and ugly transitions.
pub fn run_sweep(w: u32, h: u32, out: &std::path::Path) -> anyhow::Result<()> {
    let mut host = Headless::new()?;
    host.app.view_w = w as f64;
    host.app.view_h = h as f64;
    let mut k = 0u32;
    let mut lw = 7.5f64;
    while lw <= 25.6 {
        reset_scene(&mut host);
        host.app.modes.ladder = true;
        let jd = host.app.clock.jd;
        // the camera stays where a person zooming out from Earth would be
        host.app.cam.center = if lw < 10.5 {
            sim::body_pos(2, jd)
        } else {
            DVec3::ZERO
        };
        host.app.cam.mpp = 10f64.powf(lw) / w as f64;
        let path = out.join(format!("lw_{k:02}_{lw:.1}.png"));
        host.export_png(w, h, &path)?;
        println!("wrote {}", path.display());
        lw += 0.5;
        k += 1;
    }
    Ok(())
}

/// Frame cost on the real GPU, with no window involved: each scene renders for
/// a while with the GPU fully drained every frame, so the number is honest.
pub fn run_bench(w: u32, h: u32, which: &str) -> anyhow::Result<()> {
    let mut host = Headless::new()?;
    host.app.view_w = w as f64;
    host.app.view_h = h as f64;
    println!("adapter: {}", crate::engine::gpu::adapter_info()?);
    println!(
        "{:18} {:>9} {:>9} {:>9}",
        "scene", "mean ms", "p95 ms", "fps"
    );
    let wanted: Vec<&str> = which.split(',').map(str::trim).collect();
    for shot in SHOTS {
        if which != "all" && !wanted.contains(&shot.name) {
            continue;
        }
        reset_scene(&mut host);
        (shot.setup)(&mut host.app);
        host.app.cam.mpp *= 1600.0 / w as f64;
        for _ in 0..4 {
            host.frame(w, h, 1.0 / 60.0, false)?;
        }
        let mut ms: Vec<f64> = Vec::new();
        for _ in 0..40 {
            let t0 = Instant::now();
            host.frame(w, h, 1.0 / 60.0, false)?;
            let _ = host.app.gpu.device.poll(wgpu::PollType::Wait);
            ms.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = ms.iter().sum::<f64>() / ms.len() as f64;
        println!(
            "{:18} {:9.2} {:9.2} {:9.0}",
            shot.name,
            mean,
            ms[ms.len() * 95 / 100],
            1e3 / mean
        );
    }
    Ok(())
}

/// A recording stores the width of the window it was made in. Only a hand edit can make that
/// something no window was, and it would silently render at the zoom limits.
fn check_view_widths(keys: &[PathKey]) -> anyhow::Result<()> {
    for (i, key) in keys.iter().enumerate() {
        if let Some(w) = key.view_w {
            anyhow::ensure!(
                (1.0..=65536.0).contains(&w),
                "path file: keyframe {} has view_w {w:?}, which isn't a window width (1 to 65536 pixels)",
                i + 1
            );
        }
    }
    Ok(())
}

pub fn run_path(
    path: &std::path::Path,
    w: u32,
    h: u32,
    fps: u32,
    out: &std::path::Path,
) -> anyhow::Result<()> {
    let keys: Vec<PathKey> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    anyhow::ensure!(!keys.is_empty(), "path file has no keyframes");
    check_view_widths(&keys)?;
    let mut host = Headless::new()?;
    host.app.cinema = true;
    host.app.view_w = w as f64;
    host.app.view_h = h as f64;
    // a recorded path plays in real time: the camera eases as it would live
    host.app.immediate = false;
    host.app.block_textures = true;
    // jump instantly to the first key, then play the rest
    host.app.apply_key(&keys[0]);
    if let Some(a) = host.app.cam.anim.take() {
        host.app.cam.center = a.c1;
        host.app.cam.mpp = 10f64.powf(a.lm1);
        host.app.cam.tilt = a.tilt1;
        host.app.cam.yaw = a.yaw1;
    }
    host.app.play = Some(PathPlay {
        keys: keys.clone(),
        k: 0,
        t: 0.0,
    });
    std::fs::create_dir_all(out)?;
    let dt = 1.0 / fps as f64;
    let total: f64 = keys.iter().map(|k| k.dur + k.hold).sum();
    let frames = (total * fps as f64).ceil() as usize + fps as usize;
    for i in 0..frames {
        let px = host.frame(w, h, dt, true)?;
        save_png(&out.join(format!("frame_{i:05}.png")), w, h, &px)?;
        if host.app.play.is_none() {
            break;
        }
    }
    std::fs::write(
        out.join("make_video.txt"),
        format!("ffmpeg -framerate {fps} -i frame_%05d.png -c:v libx264 -pix_fmt yuv420p -crf 16 pantheon.mp4\n"),
    )?;
    println!("frames in {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real app, without a window, one test at a time. Each creates and destroys a Vulkan
    /// device, and with several doing so at once the test process segfaults now and then, in
    /// the Vulkan loader's `loader_get_icd_and_device` as wgpu names an object, while other
    /// threads are in `vkCreateDevice` and `vkDestroyDevice`.
    struct Host {
        // first, so the device is gone before the next test starts
        headless: Headless,
        _one_at_a_time: std::sync::MutexGuard<'static, ()>,
    }

    impl std::ops::Deref for Host {
        type Target = Headless;
        fn deref(&self) -> &Headless {
            &self.headless
        }
    }

    impl std::ops::DerefMut for Host {
        fn deref_mut(&mut self) -> &mut Headless {
            &mut self.headless
        }
    }

    static RENDERER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// With nothing here that can render, the test skips and says so, unless
    /// `PANTHEON_REQUIRE_GPU` is set, as CI sets it, and then it fails.
    fn host() -> Option<Host> {
        // a failed test only poisons this lock: the renderer it guards is its own
        let one_at_a_time = RENDERER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match Headless::new() {
            Ok(mut h) => {
                h.app.view_w = 1600.0;
                h.app.view_h = 900.0;
                Some(Host {
                    headless: h,
                    _one_at_a_time: one_at_a_time,
                })
            }
            Err(e) => {
                assert!(
                    std::env::var_os("PANTHEON_REQUIRE_GPU").is_none(),
                    "this test must render, and can't: {e:#}"
                );
                crate::test_skipped(&format!("nothing here can render: {e:#}"));
                None
            }
        }
    }

    #[test]
    fn the_headless_path_renders_a_real_picture() {
        let Some(mut h) = host() else { return };
        let (w, ht) = (640, 360);
        h.app.dive_to(ObjectRef::Planet(2));
        land(&mut h.app);
        let px = h.frame(w, ht, FRAME, true).unwrap();
        assert_eq!(px.len(), (w * ht * 4) as usize);
        let colours: std::collections::HashSet<&[u8]> = px.chunks(4).collect();
        assert!(colours.len() > 100, "{} colours", colours.len());
    }

    const FRAME: f64 = 1.0 / 60.0;

    fn land(a: &mut Pantheon) {
        for _ in 0..2000 {
            a.tick(FRAME);
            if a.cam.anim.is_none() {
                break;
            }
        }
        assert!(a.cam.anim.is_none(), "the flight never landed");
    }

    /// One wheel notch with the pointer at `s`, through the same code the window uses.
    fn wheel_at(a: &mut Pantheon, s: [f32; 2], dex: f64) {
        let proj = crate::frames::Proj::new(&a.cam, a.view_w, a.view_h, &a.u.gal);
        let rect = Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(a.view_w as f32, a.view_h as f32),
        );
        a.hover = a.pick_at(s, rect);
        a.wheel_zoom(Some(s), dex, &proj);
    }

    /// One wheel notch at the middle of the view.
    fn scroll(a: &mut Pantheon, dex: f64) {
        wheel_at(a, [(a.view_w * 0.5) as f32, (a.view_h * 0.5) as f32], dex);
    }

    /// What a real wheel notch does, and a fine one (Shift+wheel, or a trackpad's small steps).
    const NOTCH: f64 = 0.256;
    const FINE: f64 = 0.0512;
    const TEST_DATE: f64 = 2_461_349.5; // 2026-11-05

    fn screen_of(a: &Pantheon, obj: ObjectRef) -> [f32; 2] {
        let proj = crate::frames::Proj::new(&a.cam, a.view_w, a.view_h, &a.u.gal);
        proj.px(obj_pos(&a.u, obj, a.clock.jd, a.island.as_ref()))
    }

    fn px_apart(p: [f32; 2], q: [f32; 2]) -> f64 {
        ((p[0] - q[0]) as f64).hypot((p[1] - q[1]) as f64)
    }

    /// `selected` selected in the middle of the view, `target` `apart` px from it, the pointer on
    /// `target`. Scroll in by `notches`, `ticks` frames apart. The target stays under the pointer.
    fn aim_and_scroll(
        selected: &str,
        target: &str,
        apart: f64,
        dex: f64,
        notches: usize,
        ticks: usize,
    ) {
        let Some(mut h) = host() else { return };
        h.app.immediate = false;
        h.app.clock.jd = TEST_DATE;
        let (w, ht) = (1600, 900);
        let (Some(sel), Some(tgt)) = (h.app.find_object(selected), h.app.find_object(target))
        else {
            crate::test_skipped(&format!(
                "{target} needs the star catalog, which isn't here"
            ));
            return;
        };
        let a = &mut h.app;
        a.focus = Some(sel);
        a.follow = Some((sel, DVec3::ZERO));
        a.cam.center = obj_pos(&a.u, sel, a.clock.jd, None);
        a.cam.mpp = 1e9;
        a.view_w = w as f64;
        a.view_h = ht as f64;
        // frame it, on the projection the zoom will use
        for _ in 0..4 {
            let now = px_apart(screen_of(a, sel), screen_of(a, tgt));
            a.cam.mpp *= now / apart;
        }
        h.frame(w, ht, FRAME, false).unwrap();
        let pointer = screen_of(&h.app, tgt);
        assert!(
            h.app.picks.iter().any(|p| p.obj == tgt),
            "{target} isn't pickable at this scale"
        );
        let mut worst = 0.0f64;
        for notch in 0..notches {
            // a real frame gives the wheel fresh picks to lean toward
            h.frame(w, ht, FRAME, false).unwrap();
            worst = worst.max(px_apart(screen_of(&h.app, tgt), pointer));
            wheel_at(&mut h.app, pointer, dex);
            for _ in 0..ticks {
                h.app.tick(FRAME);
                let off = px_apart(screen_of(&h.app, tgt), pointer);
                assert!(
                    off < 6.0,
                    "{selected} -> {target}: notch {} left it {off:.0} px from the pointer",
                    notch + 1
                );
                worst = worst.max(off);
            }
        }
        while h.app.zoom_pending != 0.0 {
            h.app.tick(FRAME);
        }
        let off = px_apart(screen_of(&h.app, tgt), pointer);
        println!(
            "{selected} -> {target}: worst {worst:.1} px, settled {off:.1} px from the pointer"
        );
        assert!(
            off < 6.0,
            "{selected} -> {target}: settled {off:.0} px away"
        );
    }

    // The target stays under the pointer, with something else selected close by. A normal state:
    // Earth is selected at startup and every flight selects where it lands.

    #[test]
    fn zooming_at_the_moon_with_earth_selected_stays_on_the_moon() {
        aim_and_scroll("Earth", "Moon", 90.0, -NOTCH, 8, 42);
    }

    #[test]
    fn zooming_at_mars_with_earth_selected_stays_on_mars() {
        aim_and_scroll("Earth", "Mars", 42.0, -NOTCH, 7, 42);
    }

    #[test]
    fn zooming_at_europa_with_jupiter_selected_stays_on_europa() {
        aim_and_scroll("Jupiter", "Europa", 84.0, -NOTCH, 6, 42);
    }

    #[test]
    fn zooming_at_sirius_with_earth_selected_stays_on_sirius() {
        aim_and_scroll("Earth", "Sirius", 53.0, -NOTCH, 7, 42);
    }

    #[test]
    fn a_burst_of_notches_at_mars_stays_on_mars() {
        aim_and_scroll("Earth", "Mars", 42.0, -NOTCH, 8, 3);
    }

    #[test]
    fn fine_notches_at_the_moon_stay_on_the_moon() {
        aim_and_scroll("Earth", "Moon", 90.0, -FINE, 8, 21);
    }

    #[test]
    fn the_selection_only_leans_when_nothing_named_is_near_the_pointer() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        let earth = ObjectRef::Planet(2);
        a.focus = Some(earth);
        a.cam.center = obj_pos(&a.u, earth, a.clock.jd, None);
        a.cam.mpp = 1e7;
        let proj = crate::frames::Proj::new(&a.cam, a.view_w, a.view_h, &a.u.gal);
        let pointer = [900.0f32, 450.0]; // 100 px from Earth
                                         // nothing named near the pointer: it leans toward the selection
        a.picks.clear();
        let z = a.zoom_point_at(pointer, -NOTCH, &proj);
        assert!(z.on == Some(earth), "no lean toward the selection");
        // 100 px off, and the lean takes 15% of that
        let gap = (z.gap[0].hypot(z.gap[1]) / a.cam.mpp - 85.0).abs();
        assert!(gap < 0.5, "{gap}");
        // something named at the pointer: the zoom point is on that
        let moon = ObjectRef::Moon(0);
        a.picks.push(crate::atlas::Pickable {
            pos: pointer,
            r: 5.0,
            obj: moon,
        });
        let z = a.zoom_point_at(pointer, -NOTCH, &proj);
        assert!(z.on == Some(moon), "it should be on the Moon");
    }

    /// Scroll out at `pointer` and back without moving it: the view returns to where it was.
    fn out_and_back(h: &mut Headless, pointer: [f32; 2], notches: usize) -> (f64, f64) {
        h.app.immediate = false;
        let start = h.app.cam.center_root();
        let start_mpp = h.app.cam.mpp;
        let (w, ht) = (h.app.view_w as u32, h.app.view_h as u32);
        for dex in [0.1, -0.1] {
            for _ in 0..notches {
                wheel_at(&mut h.app, pointer, dex);
                h.frame(w, ht, FRAME, false).unwrap();
                for _ in 0..5 {
                    h.app.tick(FRAME);
                }
            }
            while h.app.zoom_pending != 0.0 {
                h.app.tick(FRAME);
            }
        }
        let view = h.app.cam.mpp * h.app.view_w;
        (
            (h.app.cam.center_root() - start).length() / view,
            h.app.cam.mpp / start_mpp,
        )
    }

    #[test]
    fn scrolling_out_and_back_off_centre_returns_to_the_system() {
        let Some(mut h) = host() else { return };
        let ei = h.app.u.find_system("TRAPPIST-1").unwrap();
        h.frame(800, 450, FRAME, false).unwrap();
        h.app.dive_to(ObjectRef::ExoHost(ei));
        land(&mut h.app);
        let (off, scale) = out_and_back(&mut h, [610.0, 120.0], 120);
        assert!(off < 0.02, "came back {off} of a view away");
        assert!(
            (scale - 1.0).abs() < 0.3,
            "came back at {scale} of the scale"
        );
        assert_eq!(h.app.scene_exo, Some(ei), "the system should be back");
    }

    #[test]
    fn a_deliberate_move_after_scrolling_out_aims_at_the_new_place() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        let earth = ObjectRef::Planet(2);
        a.focus = Some(earth);
        a.follow = Some((earth, DVec3::ZERO));
        a.cam.center = obj_pos(&a.u, earth, a.clock.jd, None);
        a.cam.mpp = 1e9;
        scroll(a, NOTCH);
        for _ in 0..30 {
            a.tick(FRAME);
        }
        a.picks.clear();
        a.focus = None;
        assert!(a.scrolled_out.is_some());
        // a resting hand: the way back holds
        let proj = crate::frames::Proj::new(&a.cam, a.view_w, a.view_h, &a.u.gal);
        let nudged = [(a.view_w * 0.5) as f32 + 12.0, (a.view_h * 0.5) as f32];
        let z = a.zoom_point_at(nudged, -NOTCH, &proj);
        assert_eq!(z.screen, [a.view_w * 0.5, a.view_h * 0.5]);
        assert!(a.scrolled_out.is_some());
        // more than that, since the last notch: this is somewhere else
        let pointer = [nudged[0] + 12.5, nudged[1]];
        let z = a.zoom_point_at(pointer, -NOTCH, &proj);
        assert_eq!(z.screen, [pointer[0] as f64, pointer[1] as f64]);
        assert!(a.scrolled_out.is_none());
    }

    #[test]
    fn a_click_ends_the_way_back() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        scroll(a, NOTCH);
        assert!(a.scrolled_out.is_some());
        a.click_canvas(None);
        assert!(a.scrolled_out.is_none());
    }

    /// `obj` followed in the middle of a view `lw` wide, with no presentation tilt yet.
    fn view_of(a: &mut Pantheon, obj: ObjectRef, lw: f64) {
        a.focus = Some(obj);
        a.follow = Some((obj, DVec3::ZERO));
        a.cam.anim = None;
        a.cam.center = obj_pos(&a.u, obj, a.clock.jd, a.island.as_ref()) - a.cam.frame_origin;
        a.cam.mpp = 10f64.powf(lw) / a.view_w;
        a.cam.pres_tilt = 0.0;
        a.cam.pres_yaw = 0.0;
        a.zoom_anchor = None;
        a.zoom_pending = 0.0;
        a.scrolled_out = None;
        a.history.clear();
    }

    /// The opening view is this wide, and each notch out from it adds a NOTCH.
    const OPENING_LW: f64 = 7.547;

    /// Rounded to a pixel, as a pointer is.
    fn pointer_on(a: &Pantheon, obj: ObjectRef) -> [f32; 2] {
        let p = screen_of(a, obj);
        [p[0].round(), p[1].round()]
    }

    /// Scroll in at `pointer`, `ticks` frames between notches, and return the worst distance of
    /// `target` from it. A real frame before each notch gives the wheel fresh picks.
    fn dive(
        h: &mut Host,
        target: ObjectRef,
        pointer: [f32; 2],
        notches: usize,
        ticks: usize,
    ) -> f64 {
        let (w, ht) = (h.app.view_w as u32, h.app.view_h as u32);
        let mut worst = 0.0f64;
        for _ in 0..notches {
            h.frame(w, ht, FRAME, false).unwrap();
            worst = worst.max(px_apart(screen_of(&h.app, target), pointer));
            wheel_at(&mut h.app, pointer, -NOTCH);
            for _ in 0..ticks {
                h.app.tick(FRAME);
                worst = worst.max(px_apart(screen_of(&h.app, target), pointer));
            }
        }
        // and for as long again after the last one: the zoom has ended, the target hasn't
        for _ in 0..150 {
            h.app.tick(FRAME);
            worst = worst.max(px_apart(screen_of(&h.app, target), pointer));
        }
        worst
    }

    /// Scroll out at the middle of the view, with the picks a real frame gives the wheel.
    fn out_from_the_middle(h: &mut Host, notches: usize) {
        let (w, ht) = (h.app.view_w as u32, h.app.view_h as u32);
        for _ in 0..notches {
            h.frame(w, ht, FRAME, false).unwrap();
            scroll(&mut h.app, NOTCH);
            for _ in 0..3 {
                h.app.tick(FRAME);
            }
        }
        for _ in 0..90 {
            h.app.tick(FRAME);
        }
    }

    /// One row of the time-running table: Earth followed, the pointer on `target`.
    struct TimeRow {
        target: &'static str,
        rate: f64,
        outs: f64,
        ticks: usize,
        notches: usize,
        simulate: bool,
    }

    fn time_row(row: &TimeRow) {
        let Some(mut h) = host() else { return };
        h.app.clock.jd = TEST_DATE;
        h.app.immediate = false;
        let earth = ObjectRef::Planet(2);
        h.app.focus = Some(earth);
        if row.simulate {
            h.app.awaken();
        }
        h.app.clock.playing = true;
        h.app.clock.rate = row.rate;
        let target = h.app.find_object(row.target).unwrap();
        view_of(&mut h.app, earth, OPENING_LW + row.outs * NOTCH);
        h.frame(1600, 900, FRAME, false).unwrap();
        let pointer = pointer_on(&h.app, target);
        let worst = dive(&mut h, target, pointer, row.notches, row.ticks);
        let what = format!(
            "{} at rate {}, {} frames between notches, simulated {}",
            row.target, row.rate, row.ticks, row.simulate
        );
        println!("{what}: worst {worst:.1} px from the pointer");
        if row.simulate {
            assert!(h.app.island.is_some(), "{what}: the simulation ended");
        }
        assert!(worst < 6.0, "{what}: {worst:.0} px from the pointer");
    }

    macro_rules! time_rows {
        ($($name:ident: $target:expr, $rate:expr, $outs:expr, $ticks:expr, $notches:expr, $sim:expr;)*) => {
            $(#[test]
            fn $name() {
                time_row(&TimeRow {
                    target: $target,
                    rate: $rate,
                    outs: $outs,
                    ticks: $ticks,
                    notches: $notches,
                    simulate: $sim,
                });
            })*
        };
    }

    // 60 frames a second, so 27 frames is a notch every 0.45 s
    time_rows! {
        sun_at_a_day_a_second_every_450_ms: "Sun", 86_400.0, 21.0, 27, 16, false;
        sun_at_a_day_a_second_every_200_ms: "Sun", 86_400.0, 21.0, 12, 16, false;
        sun_at_a_day_a_second_every_100_ms: "Sun", 86_400.0, 21.0, 6, 16, false;
        sun_at_a_day_a_second_burst_of_16: "Sun", 86_400.0, 21.0, 2, 16, false;
        sun_at_an_hour_a_second_every_450_ms: "Sun", 3_600.0, 21.0, 27, 16, false;
        sun_at_a_month_a_second_every_200_ms: "Sun", 2.63e6, 21.0, 12, 16, false;
        sun_in_a_simulation_at_a_day_a_second: "Sun", 86_400.0, 21.0, 27, 14, true;
        moon_at_a_day_a_second_every_450_ms: "Moon", 86_400.0, 9.0, 27, 8, false;
        moon_at_a_day_a_second_slowly: "Moon", 86_400.0, 9.0, 60, 8, false;
        moon_in_a_simulation_at_a_day_a_second: "Moon", 86_400.0, 9.0, 27, 8, true;
        mars_at_a_day_a_second_every_450_ms: "Mars", 86_400.0, 21.0, 27, 16, false;
        jupiter_at_a_day_a_second_every_450_ms: "Jupiter", 86_400.0, 24.0, 27, 16, false;
    }

    #[test]
    fn the_star_with_a_planet_followed_and_time_running() {
        let Some(mut h) = host() else { return };
        h.app.clock.jd = TEST_DATE;
        h.app.immediate = false;
        let ei = h.app.u.find_system("TRAPPIST-1").unwrap();
        h.frame(1600, 900, FRAME, false).unwrap();
        h.app.dive_to(ObjectRef::ExoPlanet(ei, 3));
        land(&mut h.app);
        out_from_the_middle(&mut h, 13);
        h.app.clock.playing = true;
        h.app.clock.rate = 86_400.0;
        h.frame(1600, 900, FRAME, false).unwrap();
        let star = ObjectRef::ExoHost(ei);
        let pointer = pointer_on(&h.app, star);
        let worst = dive(&mut h, star, pointer, 10, 27);
        assert!(worst < 6.0, "{worst:.0} px from the pointer");
    }

    /// A moon aimed at from close to its planet, time paused: the presentation tilt turns the
    /// view as the zoom goes in, and the moon stays under the pointer through it.
    fn moon_row(planet: &str, moon: &str, outs: usize) {
        let Some(mut h) = host() else { return };
        h.app.immediate = false;
        let (pl, mo) = (
            h.app.find_object(planet).unwrap(),
            h.app.find_object(moon).unwrap(),
        );
        let ObjectRef::Moon(mi) = mo else { panic!() };
        let mut ran = 0;
        let mut worst = 0.0f64;
        // around its orbit: how far it stands off the camera's plane depends on where it is
        for quarter in 0..4 {
            h.app.clock.jd = TEST_DATE + quarter as f64 * MOONS[mi].period_d.abs() / 4.0;
            h.frame(1600, 900, FRAME, false).unwrap();
            h.app.scrolled_out = None;
            h.app.dive_to(pl);
            land(&mut h.app);
            for _ in 0..120 {
                h.app.tick(FRAME);
            }
            out_from_the_middle(&mut h, outs);
            h.frame(1600, 900, FRAME, false).unwrap();
            if !h.app.picks.iter().any(|p| p.obj == mo) {
                continue;
            }
            ran += 1;
            let pointer = pointer_on(&h.app, mo);
            worst = worst.max(dive(&mut h, mo, pointer, 10, 30));
            println!("{moon} around its orbit, {quarter}/4: worst so far {worst:.1} px");
            // back to the planet for the next one
            h.app.view_w = 1600.0;
        }
        assert!(ran > 0, "{moon} was never in view");
        assert!(worst < 6.0, "{moon}: {worst:.0} px from the pointer");
    }

    #[test]
    fn phobos_stays_under_the_pointer() {
        moon_row("Mars", "Phobos", 2);
    }

    #[test]
    fn deimos_stays_under_the_pointer() {
        moon_row("Mars", "Deimos", 2);
    }

    #[test]
    fn io_stays_under_the_pointer() {
        moon_row("Jupiter", "Io", 3);
    }

    #[test]
    fn europa_stays_under_the_pointer() {
        moon_row("Jupiter", "Europa", 3);
    }

    #[test]
    fn titan_stays_under_the_pointer() {
        moon_row("Saturn", "Titan", 3);
    }

    /// With time running close to a body the view keeps turning, and the zoom point keeps being
    /// held. It must not stand in the way of anything else that moves the camera.
    #[test]
    fn a_held_zoom_point_gives_way() {
        let Some(mut h) = host() else { return };
        let earth = ObjectRef::Planet(2);
        h.app.clock.jd = TEST_DATE;
        h.app.immediate = false;
        h.frame(1600, 900, FRAME, false).unwrap();
        h.app.dive_to(earth);
        land(&mut h.app);
        h.app.clock.playing = true;
        h.app.clock.rate = 86_400.0;
        h.frame(1600, 900, FRAME, false).unwrap();
        let settled = |h: &mut Host| {
            wheel_at(&mut h.app, [1060.0, 280.0], -FINE);
            for _ in 0..90 {
                h.app.tick(FRAME);
            }
            assert!(h.app.zoom_anchor.is_some(), "the view has stopped turning");
            assert_eq!(h.app.zoom_pending, 0.0);
        };
        // the arrow keys move the view
        settled(&mut h);
        let a = &mut h.app;
        let (e0, m) = (screen_of(a, earth), a.cam.mpp);
        a.key_pan = [520.0, 0.0];
        a.tick(FRAME);
        a.tick(FRAME);
        a.key_pan = [0.0, 0.0];
        assert!(a.zoom_anchor.is_none());
        assert!(
            px_apart(screen_of(a, earth), e0) > 5.0,
            "the keys did nothing"
        );
        assert_eq!(a.cam.mpp, m);
        // a flight goes where it's going
        settled(&mut h);
        let a = &mut h.app;
        a.dive_to(ObjectRef::Planet(3));
        a.tick(FRAME);
        assert!(a.zoom_anchor.is_none());
        land(a);
        assert!(px_apart(screen_of(a, ObjectRef::Planet(3)), [800.0, 450.0]) < 1.0);
        // and letting go of what you follow lets go of it
        a.dive_to(earth);
        land(a);
        h.frame(1600, 900, FRAME, false).unwrap();
        settled(&mut h);
        let a = &mut h.app;
        a.click_canvas(None);
        assert!(a.zoom_anchor.is_none());
        let before = a.cam.center_root();
        a.tick(FRAME);
        assert_eq!(a.cam.center_root(), before, "still riding Earth");
    }

    /// Out of TRAPPIST-1 and back, with what a hand does to the pointer in between.
    fn retrace_row(moved_at_far_end: f32, drift_every_5: f32) {
        let Some(mut h) = host() else { return };
        h.app.immediate = false;
        let (w, ht) = (640, 360);
        let ei = h.app.u.find_system("TRAPPIST-1").unwrap();
        h.frame(w, ht, FRAME, false).unwrap();
        h.app.dive_to(ObjectRef::ExoHost(ei));
        land(&mut h.app);
        for _ in 0..30 {
            h.app.tick(FRAME);
        }
        let start = h.app.cam.center_root();
        let start_mpp = h.app.cam.mpp;
        let mut pointer = [440.0f32, 112.0];
        for way in [NOTCH, -NOTCH] {
            for n in 0..55 {
                if n % 5 == 4 {
                    pointer[if way > 0.0 { 0 } else { 1 }] += drift_every_5;
                }
                h.frame(w, ht, FRAME, false).unwrap();
                wheel_at(&mut h.app, pointer, way);
                for _ in 0..5 {
                    h.app.tick(FRAME);
                }
            }
            for _ in 0..60 {
                h.app.tick(FRAME);
            }
            if way > 0.0 {
                pointer[0] += moved_at_far_end;
            }
        }
        let view = h.app.cam.mpp * h.app.view_w;
        let off = (h.app.cam.center_root() - start).length() / view;
        let what = format!("moved {moved_at_far_end}, drift {drift_every_5}");
        assert!(off < 0.02, "{what}: came back {off} of a view away");
        assert!(
            (h.app.cam.mpp / start_mpp - 1.0).abs() < 0.05,
            "{what}: at {} of the scale",
            h.app.cam.mpp / start_mpp
        );
        assert_eq!(
            h.app.scene_exo,
            Some(ei),
            "{what}: the system should be back"
        );
    }

    #[test]
    fn scrolling_back_with_a_still_hand_returns() {
        retrace_row(0.0, 0.0);
    }

    #[test]
    fn scrolling_back_after_3_px_at_the_far_end_returns() {
        retrace_row(3.0, 0.0);
    }

    #[test]
    fn scrolling_back_after_6_px_at_the_far_end_returns() {
        retrace_row(6.0, 0.0);
    }

    #[test]
    fn scrolling_back_with_a_slow_drift_returns() {
        retrace_row(0.0, 1.0);
    }

    /// The tilt eases on the opening view, and that can't take the way back away.
    #[test]
    fn scrolling_back_on_the_opening_view_returns_though_the_view_turned() {
        let Some(mut h) = host() else { return };
        h.app.clock.jd = TEST_DATE;
        h.app.immediate = false;
        h.frame(1600, 900, FRAME, false).unwrap();
        let earth = ObjectRef::Planet(2);
        view_of(&mut h.app, earth, OPENING_LW);
        for _ in 0..180 {
            h.app.tick(FRAME);
        }
        let (start, tilt0) = (screen_of(&h.app, earth), h.app.cam.pres_tilt);
        let pointer = [1060.0, 280.0];
        let mut turned = 0.0f64;
        for way in [NOTCH, -NOTCH] {
            for _ in 0..6 {
                h.frame(1600, 900, FRAME, false).unwrap();
                wheel_at(&mut h.app, pointer, way);
                for _ in 0..27 {
                    h.app.tick(FRAME);
                    turned = turned.max((h.app.cam.pres_tilt - tilt0).abs());
                }
            }
        }
        for _ in 0..120 {
            h.app.tick(FRAME);
        }
        let end = screen_of(&h.app, earth);
        println!("the view turned by {turned:.2} rad, Earth {start:?} to {end:?}");
        assert!(turned > 0.1, "this should have turned the view: {turned}");
        assert!(
            px_apart(start, end) < 2.0,
            "Earth {start:?} ended at {end:?}"
        );
    }

    /// Scroll well out on Earth, and the Sun's a pixel or two away: move onto it and it's the Sun.
    #[test]
    fn a_target_right_next_to_where_you_scrolled_out_from_still_wins() {
        let Some(mut h) = host() else { return };
        h.app.clock.jd = TEST_DATE;
        h.app.immediate = false;
        h.frame(1600, 900, FRAME, false).unwrap();
        let earth = ObjectRef::Planet(2);
        view_of(&mut h.app, earth, OPENING_LW);
        out_from_the_middle(&mut h, 26);
        h.frame(1600, 900, FRAME, false).unwrap();
        let sun = ObjectRef::Sun;
        let pointer = pointer_on(&h.app, sun);
        let apart = px_apart(screen_of(&h.app, sun), screen_of(&h.app, earth));
        assert!(apart < 3.0, "the Sun is {apart} px from Earth");
        let worst = dive(&mut h, sun, pointer, 10, 27);
        assert!(worst < 6.0, "the Sun ended {worst:.0} px from the pointer");
    }

    /// What `98a22ae` did with a wheel notch: a point on the camera plane fixed in the world,
    /// leaning toward the nearest thing with a name.
    fn old_wheel(a: &mut Pantheon, s: [f32; 2], dex: f64) {
        let proj = crate::frames::Proj::new(&a.cam, a.view_w, a.view_h, &a.u.gal);
        let (sx, sy) = (s[0] as f64, s[1] as f64);
        let mut world = proj.unproject_local(sx, sy);
        if dex < 0.0 {
            let best = a
                .picks
                .iter()
                .map(|pk| ((pk.pos[0] - s[0]).hypot(pk.pos[1] - s[1]), pk.obj))
                .filter(|(d, _)| *d < 150.0)
                .min_by(|p, q| p.0.total_cmp(&q.0));
            if let Some((d, obj)) = best {
                let poi = obj_pos(&a.u, obj, a.clock.jd, None) - proj.origin;
                let k = (0.45 * (1.0 - (d as f64 / 150.0))).clamp(0.0, 0.45);
                world = world + (poi - world) * k;
            }
        }
        let mut z = a.zoom_point(None, world, [sx, sy], 1.0, &proj);
        z.gap = [0.0, 0.0];
        a.zoom_anchor = Some(z);
        a.zoom_pending += dex;
        a.cam.anim = None;
    }

    /// Time paused: the same path as `98a22ae`, frame by frame, for Earth and for the thing aimed at.
    fn paused_row(what: &str, target: &str, outs: f64, aim_off: [f32; 2]) {
        let Some(mut h) = host() else { return };
        h.app.clock.jd = TEST_DATE;
        h.app.immediate = false;
        let earth = ObjectRef::Planet(2);
        let tgt = h.app.find_object(target).unwrap();
        let mut paths: Vec<Vec<[f32; 4]>> = Vec::new();
        for old in [true, false] {
            view_of(&mut h.app, earth, OPENING_LW + outs * NOTCH);
            h.app.cam.pres_tilt = 0.0;
            h.frame(1600, 900, FRAME, false).unwrap();
            let on = pointer_on(&h.app, tgt);
            let pointer = [on[0] + aim_off[0], on[1] + aim_off[1]];
            let mut path = Vec::new();
            for _ in 0..8 {
                h.frame(1600, 900, FRAME, false).unwrap();
                if old {
                    old_wheel(&mut h.app, pointer, -NOTCH);
                } else {
                    wheel_at(&mut h.app, pointer, -NOTCH);
                }
                for _ in 0..30 {
                    h.app.tick(FRAME);
                }
                let (t, e) = (screen_of(&h.app, tgt), screen_of(&h.app, earth));
                path.push([t[0], t[1], e[0], e[1]]);
            }
            paths.push(path);
        }
        for (n, (o, s)) in paths[0].iter().zip(&paths[1]).enumerate() {
            // Earth can be a long way off by now, and then a pixel is nothing
            let target_px = (o[0] - s[0]).hypot(o[1] - s[1]) as f64;
            let earth_px = (o[2] - s[2]).hypot(o[3] - s[3]) as f64;
            let scale = (o[2] as f64 - 800.0).hypot(o[3] as f64 - 450.0).max(1.0);
            println!(
                "{what} notch {}: target {target_px:.1} px, Earth {earth_px:.1} px off",
                n + 1
            );
            assert!(
                target_px < 8.0,
                "{what} notch {}: target {target_px:.1} px",
                n + 1
            );
            assert!(
                earth_px < 8.0 + 0.02 * scale,
                "{what} notch {}: Earth {earth_px:.1} px",
                n + 1
            );
        }
    }

    #[test]
    fn paused_on_the_moon_as_before() {
        paused_row("Moon", "Moon", 9.0, [0.0, 0.0]);
    }

    #[test]
    fn paused_on_mars_as_before() {
        paused_row("Mars", "Mars", 21.0, [0.0, 0.0]);
    }

    #[test]
    fn paused_on_a_label_as_before() {
        // a label sits just above its dot
        paused_row("a label", "Moon", 9.0, [0.0, -13.0]);
    }

    #[test]
    fn paused_on_empty_space_as_before() {
        paused_row("empty space", "Earth", 9.0, [400.0, -260.0]);
    }

    /// In a system, the title and the scene answer to where the view actually is.
    fn title(h: &mut Host) -> String {
        let (w, ht) = (h.app.view_w as u32, h.app.view_h as u32);
        h.frame(w, ht, FRAME, false).unwrap();
        h.app.fo.as_ref().unwrap().band_name.clone()
    }

    fn into_system(h: &mut Host) -> usize {
        h.app.immediate = false;
        let ei = h.app.u.find_system("TRAPPIST-1").unwrap();
        h.frame(800, 450, FRAME, false).unwrap();
        h.app.dive_to(ObjectRef::ExoHost(ei));
        land(&mut h.app);
        ei
    }

    fn scroll_notches(h: &mut Host, pointer: [f32; 2], way: f64, notches: usize) {
        for _ in 0..notches {
            h.frame(800, 450, FRAME, false).unwrap();
            wheel_at(&mut h.app, pointer, way);
            for _ in 0..4 {
                h.app.tick(FRAME);
            }
        }
        for _ in 0..60 {
            h.app.tick(FRAME);
        }
    }

    #[test]
    fn the_title_does_not_name_a_star_far_out_of_view() {
        let Some(mut h) = host() else { return };
        let ei = into_system(&mut h);
        scroll_notches(&mut h, [550.0, 140.0], NOTCH, 25);
        // somewhere else, about a light-year from the star at this scale, and back in
        scroll_notches(&mut h, [502.0, 140.0], -NOTCH, 25);
        let star = h.app.u.exos[ei].pos;
        let d = (h.app.cam.center_root() - star).length();
        let view = h.app.cam.mpp * h.app.view_w;
        let name = title(&mut h);
        println!("{name}, the star {} views away", d / view);
        assert!(d > 3.0 * view, "this should end well away from the star");
        assert_ne!(
            name,
            h.app.u.exos[ei].host,
            "the star is {} views away",
            d / view
        );
        assert_eq!(h.app.scene_exo, None);
    }

    /// However you got there, the star's spot shows the star's system.
    fn lands_in_the_system(deselect: impl Fn(&mut Pantheon)) {
        let Some(mut h) = host() else { return };
        let ei = into_system(&mut h);
        let pointer = [550.0, 140.0];
        deselect(&mut h.app);
        scroll_notches(&mut h, pointer, NOTCH, 30);
        assert_eq!(h.app.scene_exo, None, "should have left");
        scroll_notches(&mut h, pointer, -NOTCH, 30);
        assert_eq!(h.app.scene_exo, Some(ei), "back on the star, no system");
        let host_name = h.app.u.exos[ei].host.clone();
        assert_eq!(title(&mut h), host_name);
    }

    #[test]
    fn a_stray_click_then_out_and_back_lands_in_the_system() {
        lands_in_the_system(|a| a.click_canvas(None));
    }

    #[test]
    fn esc_then_out_and_back_lands_in_the_system() {
        lands_in_the_system(|a| {
            a.focus = None;
            a.follow = None;
        });
    }

    #[test]
    fn dragging_the_rail_from_a_system_stays_with_it() {
        let Some(mut h) = host() else { return };
        let ei = into_system(&mut h);
        let a = &mut h.app;
        let star = a.u.exos[ei].pos;
        let lw = a.cam.log_width(a.view_w);
        a.scrub_held = true;
        a.scrub_to(lw + 1.0);
        for _ in 0..30 {
            a.tick(FRAME);
            let view = a.cam.mpp * a.view_w;
            let d = (a.cam.center_root() - star).length();
            assert!(d < 0.02 * view, "snapped {d} m in a view {view} m wide");
            assert_eq!(a.scene_exo, Some(ei), "the system went on the first frame");
        }
        assert!((a.cam.log_width(a.view_w) - lw - 1.0).abs() < 0.05);
        // on up the rail: past the system's scales it lets go, without a jump
        let mut last = a.cam.center_root();
        for n in 0..300 {
            let up = lw + 1.0 + n as f64 * 0.03;
            a.scrub_to(up);
            a.tick(FRAME);
            let view = a.cam.mpp * a.view_w;
            let at = a.cam.center_root();
            assert!((at - last).length() < view, "a jump at {up}");
            last = at;
        }
        assert_eq!(a.scene_exo, None);
    }

    #[test]
    fn flying_between_systems_keeps_the_first_until_it_has_left() {
        let Some(mut h) = host() else { return };
        let ei = into_system(&mut h);
        let Some(other) = h.app.u.find_system("Proxima Cen") else {
            crate::test_skipped("no Proxima Cen in this catalog");
            return;
        };
        let a = &mut h.app;
        a.dive_to(ObjectRef::ExoHost(other));
        a.tick(FRAME);
        assert_eq!(
            a.scene_exo,
            Some(ei),
            "the first system went on the first frame"
        );
        let mut left_at = None;
        let mut frames = 0;
        while a.cam.anim.is_some() {
            a.tick(FRAME);
            frames += 1;
            if a.scene_exo != Some(ei) && left_at.is_none() {
                left_at = Some(frames);
                let d = (a.cam.center_root() - a.u.exos[ei].pos).length();
                let view = a.cam.mpp * a.view_w;
                assert!(
                    a.cam.log_width(a.view_w) > 15.8 || d > 3.0 * view,
                    "let go with the star {} views away",
                    d / view
                );
            }
        }
        assert!(left_at.is_some());
        assert_eq!(a.scene_exo, Some(other));
    }

    #[test]
    fn a_short_hop_to_another_star_changes_the_title() {
        let Some(mut h) = host() else { return };
        h.app.immediate = false;
        let (Some(prox), Some(alpha)) = (
            h.app.u.find_system("Proxima Cen"),
            h.app
                .find_object("alf Cen A")
                .or(h.app.find_object("α Centauri")),
        ) else {
            crate::test_skipped("no Proxima Cen or α Centauri in this catalog");
            return;
        };
        h.frame(800, 450, FRAME, false).unwrap();
        h.app.dive_to(ObjectRef::ExoHost(prox));
        land(&mut h.app);
        assert_eq!(title(&mut h), "Proxima Cen");
        h.app.dive_to(alpha);
        land(&mut h.app);
        for _ in 0..10 {
            h.app.tick(FRAME);
        }
        let name = title(&mut h);
        assert_ne!(
            name,
            "Proxima Cen",
            "on {:?}",
            h.app.focus.map(|o| obj_name(&h.app.u, o))
        );
    }

    /// Frame Earth from far enough out that the presentation tilt, which turns the view close
    /// to a body, plays no part, and follow it.
    fn follow_earth_from_afar(a: &mut Pantheon) {
        let earth = ObjectRef::Planet(2);
        a.focus = Some(earth);
        a.follow = Some((earth, DVec3::ZERO));
        a.cam.anim = None;
        a.cam.center = obj_pos(&a.u, earth, a.clock.jd, a.island.as_ref()) - a.cam.frame_origin;
        a.cam.mpp = 1e7;
        a.cam.pres_tilt = 0.0;
        a.cam.pres_yaw = 0.0;
    }

    /// One notch at `pointer` in empty space: Earth's distance from the pointer changes only
    /// with scale, and the camera doesn't let go.
    fn zoom_while_following(a: &mut Pantheon, pointer: [f32; 2], dex: f64, what: &str) {
        let earth = ObjectRef::Planet(2);
        let (mpp0, gap0) = (a.cam.mpp, px_apart(screen_of(a, earth), pointer));
        wheel_at(a, pointer, dex);
        for frame in 0..40 {
            a.tick(FRAME);
            let want = gap0 * mpp0 / a.cam.mpp;
            let gap = px_apart(screen_of(a, earth), pointer);
            assert!(
                (gap - want).abs() <= 2.0,
                "{what}, frame {frame}: Earth {gap:.0} px from the pointer, not {want:.0}"
            );
            assert!(a.follow.is_some(), "{what}: the camera let go of Earth");
        }
        assert_eq!(a.zoom_pending, 0.0, "{what}");
    }

    const POINTERS: [[f32; 2]; 3] = [[800.0, 450.0], [1210.0, 180.0], [200.0, 770.0]];

    #[test]
    fn a_notch_with_time_running_holds_still_relative_to_what_you_follow() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        a.immediate = false;
        a.clock.playing = true;
        for rate in [3_600.0, 86_400.0, 2.63e6, 3.156e7] {
            a.clock.rate = rate;
            for pointer in POINTERS {
                for dex in [NOTCH, -NOTCH] {
                    follow_earth_from_afar(a);
                    a.tick(FRAME);
                    let what = format!("Play at {rate}, pointer {pointer:?}, notch {dex}");
                    zoom_while_following(a, pointer, dex, &what);
                }
            }
        }
    }

    /// The view you get on landing, where the reported notch threw Earth 77,000 px away. The
    /// presentation tilt turns the view at this scale, and that makes no difference.
    #[test]
    fn a_notch_with_time_running_keeps_a_planet_you_flew_to() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        a.immediate = false;
        for rate in [3_600.0, 86_400.0] {
            for pointer in POINTERS {
                for dex in [NOTCH, -NOTCH] {
                    a.clock.playing = false;
                    a.dive_to(ObjectRef::Planet(2));
                    land(a);
                    a.clock.playing = true;
                    a.clock.rate = rate;
                    for _ in 0..240 {
                        a.tick(FRAME);
                    }
                    let what = format!("Play at {rate}, pointer {pointer:?}, notch {dex}");
                    zoom_while_following(a, pointer, dex, &what);
                }
            }
        }
    }

    #[test]
    fn a_notch_in_a_simulation_holds_still_relative_to_what_you_follow() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        a.immediate = false;
        for rate in [3_600.0, 86_400.0] {
            for pointer in POINTERS {
                for dex in [NOTCH, -NOTCH] {
                    a.island = None;
                    a.focus = Some(ObjectRef::Planet(2));
                    a.awaken();
                    a.clock.rate = rate;
                    follow_earth_from_afar(a);
                    a.tick(FRAME);
                    let what = format!("Simulating at {rate}, pointer {pointer:?}, notch {dex}");
                    zoom_while_following(a, pointer, dex, &what);
                    assert!(a.island.is_some(), "{what}: the simulation ended");
                }
            }
        }
    }

    #[test]
    fn scrolling_out_and_back_with_time_running_returns_to_the_planet() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        a.immediate = false;
        a.clock.playing = true;
        a.clock.rate = 86_400.0;
        follow_earth_from_afar(a);
        a.tick(FRAME);
        let (earth, pointer) = (ObjectRef::Planet(2), [1210.0, 180.0]);
        let start = px_apart(screen_of(a, earth), pointer);
        let start_mpp = a.cam.mpp;
        for dex in [NOTCH, -NOTCH] {
            for _ in 0..5 {
                wheel_at(a, pointer, dex);
                for _ in 0..25 {
                    a.tick(FRAME);
                }
            }
        }
        while a.zoom_pending != 0.0 {
            a.tick(FRAME);
        }
        let end = px_apart(screen_of(a, earth), pointer);
        assert!((a.cam.mpp / start_mpp - 1.0).abs() < 1e-3);
        assert!(
            (end - start).abs() < 2.0,
            "{start:.0} px became {end:.0} px"
        );
    }

    #[test]
    fn scrolling_out_of_an_exoplanet_system_and_back_is_continuous() {
        let Some(mut h) = host() else { return };
        // the window's zoom glides over several frames, which is what crosses the boundary
        h.app.immediate = false;
        let (w, ht) = (400, 225);
        let ei = h.app.u.find_system("TRAPPIST-1").unwrap();
        h.frame(w, ht, FRAME, false).unwrap();
        h.app.dive_to(ObjectRef::ExoHost(ei));
        land(&mut h.app);
        assert_eq!(h.app.scene_exo, Some(ei));
        let start = h.app.cam.center_root();
        let start_mpp = h.app.cam.mpp;
        let watch = |a: &Pantheon, when: &str| {
            let moved = (a.cam.center_root() - start).length();
            let view = a.cam.mpp * a.view_w;
            assert!(
                moved < 0.02 * view,
                "{when}: jumped {moved} m in a view {view} m wide"
            );
        };
        // through the boundary at log width 15.8 and on out to the Local Group, where other
        // things with names crowd the middle of the view
        let notches = 120;
        for _ in 0..notches {
            // a real frame fills in the picks a wheel zoom leans toward
            h.frame(w, ht, FRAME, false).unwrap();
            scroll(&mut h.app, 0.1);
            for _ in 0..5 {
                h.app.tick(FRAME);
                watch(&h.app, "going out");
            }
        }
        while h.app.zoom_pending != 0.0 {
            h.app.tick(FRAME);
            watch(&h.app, "settling out");
        }
        assert!(
            h.app.cam.log_width(h.app.view_w) > 15.8,
            "never reached the boundary"
        );
        assert_eq!(h.app.scene_exo, None, "should have left the system");
        // and all the way back
        for _ in 0..notches {
            h.frame(w, ht, FRAME, false).unwrap();
            scroll(&mut h.app, -0.1);
            for _ in 0..5 {
                h.app.tick(FRAME);
                watch(&h.app, "coming back");
            }
        }
        while h.app.zoom_pending != 0.0 {
            h.app.tick(FRAME);
            watch(&h.app, "settling back");
        }
        let (a, mpp) = (&h.app, h.app.cam.mpp);
        assert!((mpp / start_mpp - 1.0).abs() < 0.3, "{mpp} vs {start_mpp}");
        assert_eq!(
            a.scene_exo,
            Some(ei),
            "scrolling back should show the system again"
        );
    }

    #[test]
    fn back_into_a_system_shows_the_system() {
        let Some(mut h) = host() else { return };
        let ei = h.app.u.find_system("TRAPPIST-1").unwrap();
        h.app.dive_to(ObjectRef::ExoHost(ei));
        land(&mut h.app);
        h.app.go_home();
        land(&mut h.app);
        assert_eq!(h.app.scene_exo, None, "home is not in the system");
        h.app.go_back();
        land(&mut h.app);
        assert_eq!(h.app.scene_exo, Some(ei));
        assert_eq!(h.app.focus, Some(ObjectRef::ExoHost(ei)));
        h.frame(800, 450, FRAME, false).unwrap();
        let caption = &h.app.fo.as_ref().unwrap().caption.0;
        assert!(caption.contains("Exoplanet Archive"), "{caption}");
    }

    /// The ways out of a system: it stays until the flight has left, then it's gone for good.
    #[test]
    fn flying_out_of_a_system_keeps_it_until_the_flight_leaves() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        let ei = a.u.find_system("TRAPPIST-1").unwrap();
        type Way<'a> = (&'a str, &'a dyn Fn(&mut Pantheon));
        let ways: [Way; 4] = [
            ("a search", &|a| a.dive_to(ObjectRef::Planet(3))),
            ("Home", &|a| a.go_home()),
            ("the rail", &|a| {
                a.fly_to_place(DVec3::ZERO, 1e13, "The Solar System")
            }),
            ("Back", &|a| a.go_back()),
        ];
        for (name, leave) in ways {
            // Back returns to where the last flight started
            a.dive_to(ObjectRef::Planet(2));
            land(a);
            a.dive_to(ObjectRef::ExoHost(ei));
            land(a);
            assert_eq!(a.scene_exo, Some(ei));
            let start = a.cam.center_root();
            leave(a);
            a.tick(FRAME);
            assert_eq!(a.scene_exo, Some(ei), "{name}: dropped on the first frame");
            let (mut seen_gone, mut last, mut frames) = (false, a.cam.center_root(), 0);
            while a.cam.anim.is_some() {
                a.tick(FRAME);
                frames += 1;
                let view = a.cam.mpp * a.view_w;
                let at = a.cam.center_root();
                if a.scene_exo.is_none() && !seen_gone {
                    seen_gone = true;
                    // it let go because the camera left, not before
                    let d = (at - a.u.exos[ei].pos).length();
                    assert!(
                        a.cam.log_width(a.view_w) > 15.8 || d > 3.0 * view,
                        "{name}: let go still inside, {d} m from the star"
                    );
                    assert!(frames > 1, "{name}: let go at once");
                    assert!(
                        (at - last).length() < view,
                        "{name}: the camera jumped as it let go"
                    );
                }
                assert!(
                    !seen_gone || a.scene_exo.is_none(),
                    "{name}: the system came back during the flight"
                );
                last = at;
            }
            assert_eq!(a.scene_exo, None, "{name}: still in the system on landing");
            assert!((a.cam.center_root() - start).length() > crate::frames::PC);
        }
    }

    #[test]
    fn flying_into_a_system_keeps_it_all_the_way() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        let ei = a.u.find_system("TRAPPIST-1").unwrap();
        a.dive_to(ObjectRef::Planet(2));
        land(a);
        a.dive_to(ObjectRef::ExoHost(ei));
        while a.cam.anim.is_some() {
            a.tick(FRAME);
            assert_eq!(a.scene_exo, Some(ei), "lost it on the way in");
        }
        a.go_home();
        land(a);
        a.go_back();
        while a.cam.anim.is_some() {
            a.tick(FRAME);
            assert_eq!(a.scene_exo, Some(ei), "Back lost it on the way in");
        }
    }

    #[test]
    fn the_camera_stays_on_a_simulated_planet_at_every_rate() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.dive_to(ObjectRef::Planet(2));
        land(a);
        a.awaken();
        let check = |a: &Pantheon, rate: &str| {
            let earth = obj_pos(&a.u, ObjectRef::Planet(2), a.clock.jd, a.island.as_ref());
            let off = (a.cam.center_root() - earth).length();
            let view = a.cam.mpp * a.view_w;
            assert!(
                off < 0.01 * view,
                "at {rate}: {off} m off a view {view} m wide"
            );
            assert!(a.follow.is_some(), "at {rate}: the camera let go of Earth");
        };
        // the sequence that lost it: Simulate, then ] once, and on up while it runs
        for _ in 0..60 {
            a.tick(FRAME);
            check(a, "the simulation's own rate");
        }
        for _ in 0..3 {
            a.step_rate(1);
            for _ in 0..60 {
                a.tick(FRAME);
                if a.island.is_none() {
                    return;
                }
                check(a, "a faster rate");
            }
        }
    }

    #[test]
    fn one_slow_frame_does_not_end_a_simulation() {
        // Mars at 1 month/s: Phobos's substep is about 46 s
        let (rate, substep) = (2.63e6, 46.0);
        let mut short = 0.0;
        assert_eq!(
            island_substeps(rate / 60.0, substep, 1.0 / 60.0, &mut short),
            Some(953)
        );
        assert_eq!(short, 0.0);
        // one frame of 80 ms: coarser, but it runs, and then all is forgiven
        assert_eq!(
            island_substeps(rate * 0.08, substep, 0.08, &mut short),
            Some(4000)
        );
        assert_eq!(
            island_substeps(rate / 60.0, substep, 1.0 / 60.0, &mut short),
            Some(953)
        );
        assert_eq!(short, 0.0);
        // a sustained shortfall ends it
        let mut frames = 0;
        while island_substeps(rate * 0.1, substep, 0.1, &mut short).is_some() {
            frames += 1;
            assert!(frames < 10, "it should end within half a second");
        }
        // and time far too fast ends it at once
        let mut short = 0.0;
        assert_eq!(
            island_substeps(3.156e13 / 60.0, substep, 1.0 / 60.0, &mut short),
            None
        );
    }

    #[test]
    fn a_slow_frame_in_the_real_app_keeps_the_simulation() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.dive_to(ObjectRef::Planet(3));
        land(a);
        a.awaken();
        a.clock.rate = 2.63e6;
        a.tick(FRAME);
        a.tick(0.08);
        a.tick(FRAME);
        assert!(a.island.is_some(), "one 80 ms frame ended the simulation");
    }

    #[test]
    fn flying_to_where_you_already_are_adds_no_history() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.dive_to(ObjectRef::Planet(3));
        land(a);
        let before = a.history.len();
        for _ in 0..3 {
            a.dive_to(ObjectRef::Planet(3));
            land(a);
        }
        assert_eq!(a.history.len(), before);
        a.dive_to(ObjectRef::Planet(4));
        assert_eq!(a.history.len(), before + 1);
    }

    #[test]
    fn stopping_a_flight_stays_where_it_was() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.dive_to(ObjectRef::Planet(2));
        land(a);
        a.dive_to(ObjectRef::Planet(4));
        for _ in 0..40 {
            a.tick(FRAME);
        }
        let (here, mpp) = (a.cam.center_root(), a.cam.mpp);
        a.stop_flight_here();
        for _ in 0..5 {
            a.tick(FRAME);
        }
        let moved = (a.cam.center_root() - here).length();
        assert!(moved < 0.05 * mpp * a.view_w, "snapped {moved} m");
        assert_eq!(a.cam.mpp, mpp);
    }

    #[test]
    fn a_saved_flight_never_overwrites_another() {
        let dir = std::env::temp_dir().join(format!("pantheon-save-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let a = write_new_file(&dir, "path_x", "one").unwrap();
        let b = write_new_file(&dir, "path_x", "two").unwrap();
        let (ta, tb) = (
            std::fs::read_to_string(&a).unwrap(),
            std::fs::read_to_string(&b).unwrap(),
        );
        std::fs::remove_file(&a).unwrap();
        std::fs::remove_file(&b).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert_ne!(a, b);
        assert_eq!((ta.as_str(), tb.as_str()), ("one", "two"));
    }

    #[test]
    fn a_recording_frames_the_same_view_at_any_size() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.view_w = 3840.0;
        let key = PathKey {
            center: [0.0; 3],
            mpp: 1000.0,
            view_w: Some(1700.0),
            tilt: 0.0,
            yaw: 0.0,
            jd: sim::J2000,
            dur: 0.1,
            hold: 0.0,
            at: None,
            host: None,
        };
        a.apply_key(&key);
        land(a);
        assert!(
            (a.cam.mpp * 3840.0 - 1000.0 * 1700.0).abs() < 1e-3 * 1700000.0,
            "{}",
            a.cam.mpp
        );
    }

    #[test]
    fn a_recording_with_an_impossible_width_is_refused() {
        let key = |view_w| PathKey {
            center: [0.0; 3],
            mpp: 1000.0,
            view_w,
            tilt: 0.0,
            yaw: 0.0,
            jd: sim::J2000,
            dur: 0.1,
            hold: 0.0,
            at: None,
            host: None,
        };
        for good in [None, Some(1.0), Some(840.0), Some(3840.0), Some(65536.0)] {
            assert!(check_view_widths(&[key(good)]).is_ok(), "{good:?}");
        }
        for bad in [
            0.0,
            -1680.0,
            0.5,
            1e308,
            f64::NAN,
            f64::INFINITY,
            -f64::INFINITY,
        ] {
            let e = check_view_widths(&[key(Some(1680.0)), key(Some(bad))]).unwrap_err();
            let e = e.to_string();
            assert!(e.contains("keyframe 2") && e.contains("view_w"), "{e}");
        }
    }

    /// The Lagrange markers with a simulation running, against the simulated bodies' own motion.
    #[test]
    fn lagrange_markers_follow_the_simulated_motion() {
        let u = Universe::load();
        for (name, sample_days) in [
            ("Moon", 2.0),
            ("Phobos", 0.03),
            ("Titan", 0.8),
            ("Titania", 0.4),
            ("Triton", 0.3),
            ("Charon", 0.3),
        ] {
            let mi = MOONS.iter().position(|m| m.name == name).unwrap();
            let moon = ObjectRef::Moon(mi);
            let pi = MOONS[mi].planet;
            let mut isl = build_island(&u, pi, TEST_DATE);
            let k_moon = isl.ids.iter().position(|o| *o == moon).unwrap();
            let period = isl
                .ids
                .iter()
                .filter_map(|o| match o {
                    ObjectRef::Moon(m) => Some(MOONS[*m].period_d.abs()),
                    _ => None,
                })
                .fold(f64::MAX, f64::min);
            let h = period * 86_400.0 / 600.0;
            let steps = (sample_days * 86_400.0 / h).ceil() as usize;
            let (mut swapped, mut worst) = (0, 0.0f64);
            let samples = 800;
            let rel = |isl: &Island| isl.sim.pos[k_moon] - isl.sim.pos[1];
            for _ in 0..samples {
                for _ in 0..steps {
                    isl.sim.step(h);
                }
                // the real motion and plane, from where the bodies actually go
                let r0 = rel(&isl);
                let vrel = crate::atlas::pair_relative_velocity(&u, Some(moon), 0.0, Some(&isl));
                let pa = isl.sim.pos[1];
                let pb = isl.sim.pos[k_moon];
                let [_, _, _, l4, l5] = crate::atlas::lagrange_markers(pa, pb, vrel, 1e-4);
                let mut probe = isl.sim.clone();
                probe.step(h);
                let r1 = probe.pos[k_moon] - probe.pos[1];
                let normal = r0.cross(r1).normalize();
                let ahead = normal.cross(r0).normalize();
                if (l4 - pa).dot(ahead) <= 0.0 || (l5 - pa).dot(ahead) >= 0.0 {
                    swapped += 1;
                }
                let n = (l4 - pa).cross(l5 - pa).normalize();
                worst = worst.max(n.dot(normal).abs().clamp(0.0, 1.0).acos().to_degrees());
            }
            println!("{name:8} swapped on {swapped} of {samples}, plane off by up to {worst:.2}°");
            assert_eq!(
                swapped, 0,
                "{name}: L4 and L5 swapped on {swapped} of {samples}"
            );
            assert!(worst < 1.0, "{name}: the plane is off by up to {worst}°");
        }
    }

    /// What the note says is normal, against what the app's own simulations do.
    #[test]
    fn the_energy_note_states_what_the_row_reads() {
        let u = Universe::load();
        let note = crate::evidence::rec(crate::evidence::Ev::NBodyIsland).note;
        assert!(note.contains("1% to 2.2% for our Moon"), "{note}");
        assert!(note.contains("0.2% or less for every other"), "{note}");
        let peak = |pi: usize, jd: f64, years: f64| {
            let mut isl = build_island(&u, pi, jd);
            let shortest = MOONS
                .iter()
                .filter(|m| m.planet == pi)
                .map(|m| m.period_d.abs())
                .fold(f64::MAX, f64::min);
            let h = if shortest == f64::MAX {
                3600.0
            } else {
                shortest * 86_400.0 / 600.0
            };
            let mut worst = 0.0f64;
            for _ in 0..(years * 365.25 * 86_400.0 / h) as usize {
                isl.sim.step(h);
                worst = worst.max(isl.sim.orbit_energy_drift(ISLAND_PLANET, &isl.e0));
            }
            worst * 100.0
        };
        // every 2.5 years, 1950 to 2050: this start date matters, and the worst is near the top
        for k in 0..40 {
            let jd = 2_433_282.5 + k as f64 * 913.1;
            let p = peak(2, jd, 3.0);
            assert!((1.0..=2.2).contains(&p), "our Moon from JD {jd}: {p}%");
        }
        for pi in [3, 4, 5, 6, 7, 8] {
            let p = peak(pi, TEST_DATE, 1.0);
            assert!(p <= 0.2, "{}: {p}%", PLANETS[pi].name);
        }
        for pi in [0, 1] {
            let p = peak(pi, TEST_DATE, 3.0);
            assert!(p < 0.005, "{}: {p}% wouldn't read 0.00%", PLANETS[pi].name);
        }
    }

    #[test]
    fn the_energy_row_reads_the_largest_change_so_far() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        a.dive_to(ObjectRef::Planet(2));
        land(a);
        a.awaken();
        a.clock.rate = 86_400.0;
        let (mut peak, mut fell) = (0.0f64, false);
        for _ in 0..1500 {
            a.tick(FRAME);
            let isl = a.island.as_ref().unwrap();
            let now = isl.sim.orbit_energy_drift(ISLAND_PLANET, &isl.e0);
            assert!(isl.peak_drift >= peak, "the reading went down");
            assert!(
                isl.peak_drift >= now,
                "the reading is below the current value"
            );
            fell |= now < 0.9 * isl.peak_drift;
            peak = isl.peak_drift;
        }
        assert!(peak > 0.0);
        assert!(fell, "the current value never fell, so this shows nothing");
        h.frame(800, 450, FRAME, false).unwrap();
    }

    #[test]
    fn a_slow_frame_says_it_took_coarser_steps() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        a.dive_to(ObjectRef::Planet(3));
        land(a);
        a.awaken();
        a.clock.rate = 2.63e6;
        a.tick(FRAME);
        assert_eq!(
            a.island.as_ref().unwrap().coarse_frames,
            0,
            "a normal frame"
        );
        let rows = card_rows(a);
        assert!(!rows.iter().any(|(k, _)| k == "coarser steps"), "{rows:?}");
        a.tick(0.08);
        let isl = a.island.as_ref().unwrap();
        assert_eq!(isl.coarse_frames, 1);
        // 80 ms at this rate wants about 4,600 substeps of 46 s, and gets 4,000
        assert!(isl.coarsest > 1.1 && isl.coarsest < 1.3, "{}", isl.coarsest);
        let rows = card_rows(a);
        let row = rows.iter().find(|(k, _)| k == "coarser steps");
        assert_eq!(
            row.map(|(_, v)| v.as_str()),
            Some("1 slow frame, up to 1.1x")
        );
    }

    #[test]
    fn a_planet_with_no_moons_reads_its_own_orbit() {
        let Some(mut h) = host() else { return };
        let a = &mut h.app;
        a.clock.jd = TEST_DATE;
        for pi in [0, 1] {
            a.island = None;
            a.clock.playing = false;
            a.dive_to(ObjectRef::Planet(pi));
            land(a);
            a.awaken();
            a.clock.rate = 86_400.0;
            for _ in 0..300 {
                a.tick(FRAME);
            }
            let rows = card_rows(a);
            let row = rows.iter().find(|(k, _)| k.contains("energy drift"));
            let (k, v) = row.unwrap_or_else(|| panic!("no reading: {rows:?}"));
            assert_eq!(k, "orbit energy drift, largest so far");
            assert_eq!(v, "0.00%", "{}: {v}", PLANETS[pi].name);
            let isl = a.island.as_mut().unwrap();
            // the reading can move: double the planet's speed about the Sun
            isl.sim.vel[1] *= 2.0;
            isl.sim.step(60.0);
            assert!(isl.sim.orbit_energy_drift(ISLAND_PLANET, &isl.e0) > 1.0);
        }
    }

    /// The rows of the focused object's card.
    fn card_rows(a: &Pantheon) -> Vec<(String, String)> {
        a.card_data(a.focus.unwrap()).2
    }

    #[test]
    fn simulations_use_the_real_masses() {
        assert_eq!(MOONS[0].name, "Moon");
        // a guess from radius and density gave 0.57 of this
        assert_eq!(island_gm(ObjectRef::Moon(0)), 4.902_800_066e12);
        for (mi, m) in MOONS.iter().enumerate() {
            assert_eq!(
                island_gm(ObjectRef::Moon(mi)),
                sim::MOON_GM[mi],
                "{}",
                m.name
            );
            assert!(sim::MOON_GM[mi] > 0.0, "{}", m.name);
        }
        assert_eq!(island_gm(ObjectRef::Planet(2)), PLANETS[2].gm);
        assert_eq!(island_gm(ObjectRef::Sun), sim::GM_SUN);
    }
}
