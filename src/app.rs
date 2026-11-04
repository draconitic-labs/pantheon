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

/// A saved viewpoint: where Back returns to.
#[derive(Clone, Copy)]
struct HistoryEntry {
    center: DVec3,
    mpp: f64,
    tilt: f64,
    yaw: f64,
    focus: Option<ObjectRef>,
    follow: Option<(ObjectRef, DVec3)>,
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
    pub follow: Option<(ObjectRef, DVec3)>,
    pub island: Option<Island>,
    pub island_paused: bool,
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
    /// headless exports: imagery loads synchronously, the camera settles at once
    pub immediate: bool,
    /// set by shot setups that could not resolve their subject
    pub setup_error: Option<String>,
    pub status: Option<(String, f64)>,
    /// persisted preferences (GUI only; exports never touch the disk)
    pub settings: Settings,
    pub persist_settings: bool,
    /// None until the audio engine has been tried; Some(false) = no device found
    pub ambience_available: Option<bool>,
    /// glide the clock during path playback instead of jumping (AUDIT UI-020)
    pub jd_glide: Option<f64>,
    pub search: SearchState,
    /// the canvas in logical pixels, and the display's scale
    pub view_w: f64,
    pub view_h: f64,
    pub ppp: f32,
    /// what's under the pointer right now
    pub hover: Option<ObjectRef>,
    /// labels placed last frame, for clicks and hysteresis (UX B3)
    sticky: std::collections::HashSet<String>,
    label_hits: Vec<(Rect, ObjectRef)>,
    panel_rects: Vec<Rect>,
    /// back-stack for flights
    history: Vec<HistoryEntry>,
    // ---- motion state
    /// log10(mpp) still to be applied by the smooth zoom
    zoom_pending: f64,
    /// the screen point and frame-local world point a zoom holds fixed
    zoom_anchor: Option<(f64, f64, DVec3)>,
    /// release velocity of a drag, px/s
    pan_vel: [f64; 2],
    drag_vel: [f64; 2],
    last_drag: f64,
    /// a held key's continuous pan/zoom
    key_pan: [f64; 2],
    pres_moving: bool,
    /// the first-run hint, until dismissed
    pub intro: bool,
    intro_born: f64,
    /// what a fresh clone is missing, in plain words
    pub data_gaps: Vec<&'static str>,
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
            eprintln!("data health: {p}");
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
        let mut data_gaps: Vec<&'static str> = Vec::new();
        if !bank.status.any_planet_imagery() {
            data_gaps.push("Planet maps are missing, so surfaces are procedural.");
        }
        if crate::spice::spice().is_none() {
            data_gaps.push("JPL kernels are missing, so positions use approximate orbital elements.");
        }
        if u.craft.is_empty() {
            data_gaps.push("Spacecraft tracks are missing.");
        }
        if u.stars.len() < 1000 {
            data_gaps.push("Gaia star catalogs are missing.");
        }
        if !data_gaps.is_empty() {
            eprintln!("pantheon: running with partial data. Run tools/fetch_data.py to add it:");
            for g in &data_gaps {
                eprintln!("  - {g}");
            }
        }
        let status = (!data_gaps.is_empty()).then(|| {
            (
                "Running with basic data. Open the view menu to see what's missing.".to_string(),
                12.0,
            )
        });
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
            follow: Some((ObjectRef::Planet(2), DVec3::ZERO)),
            island: None,
            island_paused: false,
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
            pan_vel: [0.0, 0.0],
            drag_vel: [0.0, 0.0],
            last_drag: -1.0,
            key_pan: [0.0, 0.0],
            pres_moving: false,
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
        self.history.push(HistoryEntry {
            center: self.cam.center_root(),
            mpp: self.cam.mpp,
            tilt: self.cam.tilt,
            yaw: self.cam.yaw,
            focus: self.focus,
            follow: self.follow,
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
        let dur = self.cam.flight_seconds(h.center, h.mpp, self.view_w);
        self.cam.fly_to_view(h.center, h.mpp, h.tilt, h.yaw, self.view_w, dur);
    }

    fn stop_motion(&mut self) {
        self.zoom_pending = 0.0;
        self.zoom_anchor = None;
        self.pan_vel = [0.0, 0.0];
    }

    /// Fly to Earth, the place everyone starts from.
    pub fn go_home(&mut self) {
        self.scene_exo = None;
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
        self.apply_smooth_zoom(dt);
        self.apply_pan_inertia(dt);
        // one time switch: play/pause is the source of truth, "living" derives
        self.modes.living = self.clock.playing;
        self.clock.tick(dt);

        // follow a moving focus so Living Orbits keeps the subject centered
        if self.cam.anim.is_none() {
            if let Some((obj, off)) = self.follow {
                let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
                self.cam.center = (p + off) - self.cam.frame_origin;
            }
        }
        // camera frame attachment (FRAMEGRAPH-003): inside an exoplanet
        // pocket the camera rebases to the host's frame so local precision
        // is AU-scale, not 1e17-scale
        let want_origin = self
            .scene_exo
            .map(|ei| self.u.exos[ei].pos)
            .unwrap_or(DVec3::ZERO);
        if (want_origin - self.cam.frame_origin).length() > 1e-3 {
            self.cam.rebase(want_origin);
        }

        // N-body island stepping
        self.island_paused = false;
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
                let dt_sub = if min_p == f64::MAX { 3600.0 } else { min_p * 86_400.0 / 600.0 };
                let n = (dt_sim.abs() / dt_sub).ceil() as usize;
                if n > 4000 {
                    self.island_paused = true;
                } else if n > 0 {
                    let h = dt_sim / n as f64;
                    for s in 0..n {
                        isl.sim.step(h);
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
        // island invalidation when the clock is scrubbed away from it
        if let Some(isl) = &self.island {
            if (isl.jd - self.clock.jd).abs() > 0.5 {
                self.island = None;
            }
        }
        self.update_presentation(dt);
        // exoplanet pocket persists until the camera actually leaves it
        if let Some(ei) = self.scene_exo {
            let lw = self.cam.log_width(self.view_w);
            let d = (self.cam.center_root() - self.u.exos[ei].pos).length();
            if lw > 15.8 || d > crate::frames::PC {
                self.scene_exo = None;
            }
        }
        if let Some((_, until)) = self.status {
            if self.anim_t > until {
                self.status = None;
            }
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
        if let Some(i) = self.u.craft.iter().position(|c| c.name.eq_ignore_ascii_case(n)) {
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

    /// Smooth zoom: wheel notches feed `zoom_pending`, which drains over ~0.1 s
    /// around a fixed anchor, so a burst of notches glides instead of stepping.
    fn apply_smooth_zoom(&mut self, dt: f64) {
        if self.zoom_pending == 0.0 {
            return;
        }
        let take = if self.immediate { 1.0 } else { 1.0 - (-dt / 0.09).exp() };
        let mut step = self.zoom_pending * take;
        if self.zoom_pending.abs() < 0.0004 {
            step = self.zoom_pending;
        }
        self.zoom_pending -= step;
        let new = (self.cam.mpp * 10f64.powf(step)).clamp(crate::frames::MPP_MIN, crate::frames::MPP_MAX);
        // hitting the zoom limits ends the glide
        if new == crate::frames::MPP_MIN || new == crate::frames::MPP_MAX {
            self.zoom_pending = 0.0;
        }
        self.cam.mpp = new;
        if let Some((sx, sy, world)) = self.zoom_anchor {
            // solved against the NEW basis, which can rotate with scale (AUDIT FND-022)
            let proj = crate::frames::Proj::new(&self.cam, self.view_w, self.view_h, &self.u.gal);
            self.cam.center = world
                - proj.right * ((sx - proj.w * 0.5) * new)
                - proj.up * ((proj.h * 0.5 - sy) * new);
        }
        if self.zoom_pending == 0.0 {
            self.zoom_anchor = None;
        }
        self.sync_follow();
    }

    /// A flung drag coasts and settles.
    fn apply_pan_inertia(&mut self, dt: f64) {
        let held = self.key_pan != [0.0, 0.0];
        if self.pan_vel == [0.0, 0.0] && !held {
            return;
        }
        let proj = crate::frames::Proj::new(&self.cam, self.view_w, self.view_h, &self.u.gal);
        let (vx, vy) = (self.pan_vel[0] + self.key_pan[0], self.pan_vel[1] + self.key_pan[1]);
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
                let Some(r) = atlas::body_frame_radius(obj) else { return };
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
        let k = if self.immediate { 1.0 } else { 1.0 - (-dt / 0.45).exp() };
        self.cam.pres_tilt += (target.0 - self.cam.pres_tilt) * k;
        let dy = crate::frames::shortest_arc(self.cam.pres_yaw, target.1) - self.cam.pres_yaw;
        self.cam.pres_yaw += dy * k;
        self.pres_moving = dy.abs() > 1e-4 || (target.0 - self.cam.pres_tilt).abs() > 1e-4;
    }

    fn apply_key(&mut self, key: &PathKey) {
        if let Some(host) = &key.host {
            if let Some(ei) = self.u.find_system(host) {
                self.focus = Some(ObjectRef::ExoHost(ei));
                self.scene_exo = Some(ei);
            }
        }
        self.follow = None;
        // glide time with the camera instead of jumping (AUDIT UI-020)
        self.jd_glide = Some(key.jd);
        let mut center = DVec3::new(key.center[0], key.center[1], key.center[2]);
        if let Some(name) = &key.at {
            if let Some(obj) = self.find_object(name) {
                center = obj_pos(&self.u, obj, key.jd, None);
                if key.host.is_none() {
                    self.focus = Some(obj); // anchors refresh focus (AUDIT UI-021)
                }
            } else if name.eq_ignore_ascii_case("galactic centre")
                || name.eq_ignore_ascii_case("galactic center")
            {
                center = self.u.gal.center;
            } else {
                eprintln!("warning: path key anchor '{name}' not found");
            }
        }
        self.cam.fly_to_view(center, key.mpp, key.tilt, key.yaw, self.view_w, key.dur.max(0.05));
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
        self.push_history();
        self.stop_motion();
        if let ObjectRef::ExoHost(ei) | ObjectRef::ExoPlanet(ei, _) = obj {
            self.scene_exo = Some(ei);
        }
        let pos = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
        let mpp = self.framing_mpp(half_extent);
        self.focus = Some(obj);
        self.follow = Some((obj, DVec3::ZERO));
        let dur = self.cam.flight_seconds(pos, mpp, self.view_w);
        self.cam.fly_to_view(pos, mpp, 0.0, 0.0, self.view_w, dur);
    }

    /// Search results and rail rungs land here.
    pub fn go_to(&mut self, dest: Dest) {
        match dest {
            Dest::Obj(obj) => self.dive_to(obj),
            Dest::Place { center, width_m, name } => self.fly_to_place(center, width_m, name),
        }
    }

    /// Fly to a location in space at a given view width.
    pub fn fly_to_place(&mut self, center: DVec3, width_m: f64, name: &str) {
        self.push_history();
        self.stop_motion();
        self.follow = None;
        self.scene_exo = None;
        let mpp = width_m / self.view_w;
        let dur = self.cam.flight_seconds(center, mpp, self.view_w);
        self.cam.fly_to_view(center, mpp, 0.0, 0.0, self.view_w, dur);
        if !name.is_empty() {
            self.note(format!("Flying to {name}"));
        }
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
        let jd = self.clock.jd;
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
            let p0 = obj_pos(&self.u, *o, jd - dt / 86_400.0, None);
            let p1 = obj_pos(&self.u, *o, jd + dt / 86_400.0, None);
            pos.push(obj_pos(&self.u, *o, jd, None));
            vel.push((p1 - p0) / (2.0 * dt));
            gm.push(match o {
                ObjectRef::Sun => sim::GM_SUN,
                ObjectRef::Planet(i) => PLANETS[*i].gm,
                ObjectRef::Moon(mi) => {
                    let m = &MOONS[*mi];
                    let rho = if m.kind == 1.0 { 3300.0 } else { 1900.0 };
                    6.674e-11 * (4.0 / 3.0) * std::f64::consts::PI
                        * (m.radius_km * 1e3).powi(3)
                        * rho
                }
                _ => 0.0,
            });
        }
        let n = ids.len();
        let mut sim = NBody { gm, pos, vel, label: format!("{} system", PLANETS[pi].name) };
        sim.zero_momentum();
        let e0 = sim.energy_scaled();
        self.island = Some(Island {
            sim,
            ids,
            anchor: ObjectRef::Planet(pi),
            trails: vec![Vec::new(); n],
            jd,
            e0,
        });
        self.modes.living = true;
        self.clock.playing = true;
        // a legible default rate: moons visibly orbit, nothing teleports (UX C6)
        self.clock.rate = 3_600.0;
        self.note(format!("Simulating the {} system with N-body gravity", PLANETS[pi].name));
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
            if self.immediate && self.bank.busy() && guard < 4 {
                self.bank.finish_loading(&self.gpu);
                continue;
            }
            break fo;
        };
        // keep a copy of pickables for next frame's input hit tests
        self.picks.clear();
        self.picks.extend(fo.picks.iter().map(|p| Pickable { pos: p.pos, r: p.r, obj: p.obj }));
        let uni = fo.uni;
        self.fo = Some(fo);
        self.pipes.render(&self.gpu, target, &self.dl, &uni, &mut self.bank);
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

        // ---- zoom: a notch is ~0.2 decades, Shift for fine control. Pinch too.
        let notch = (scroll.y + if shift { scroll.x } else { 0.0 }) as f64;
        let mut dex = -notch * 0.005 * if shift { 0.2 } else { 1.0 };
        if zoom_mul != 1.0 {
            dex -= (zoom_mul as f64).log10();
        }
        if dex.abs() > 1e-6 && resp.hovered() {
            let world_anchor = sp.map(|s| {
                let (sx, sy) = (s[0] as f64, s[1] as f64);
                // local anchor math: full precision inside attached frames
                let mut world = proj.unproject_local(sx, sy);
                // guided zoom: diving in, the anchor leans toward the nearest
                // point of interest, so repeated scrolling converges on it
                // instead of demanding pixel-perfect aim across 20 decades
                if dex < 0.0 {
                    let mut best: Option<(f32, ObjectRef)> = None;
                    for pk in &self.picks {
                        let d = (pk.pos[0] - s[0]).hypot(pk.pos[1] - s[1]);
                        if d < 150.0 && best.map(|(b, _)| d < b).unwrap_or(true) {
                            best = Some((d, pk.obj));
                        }
                    }
                    if let Some((d, obj)) = best {
                        let poi = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref()) - proj.origin;
                        let k = (0.45 * (1.0 - (d as f64 / 150.0))).clamp(0.0, 0.45);
                        world = world + (poi - world) * k;
                    }
                }
                (sx, sy, world)
            });
            self.zoom_pending += dex;
            self.zoom_anchor = world_anchor;
            self.cam.anim = None;
            self.play = None;
            self.pan_vel = [0.0, 0.0];
            self.dismiss_intro();
        }

        // ---- drag: pan with left (coasts on release); orbit with right or Shift
        let orbiting = resp.dragged_by(egui::PointerButton::Secondary)
            || (shift && resp.dragged_by(egui::PointerButton::Primary));
        if orbiting {
            let d = resp.drag_delta();
            self.cam.tilt = (self.cam.tilt + d.y as f64 * 0.005).clamp(-1.2, 1.45);
            self.cam.yaw -= d.x as f64 * 0.005;
            self.cam.anim = None;
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
            self.sync_follow();
            // track release velocity; ignore frames where the hand stopped
            let inst = [d.x as f64 / dt.max(1e-3), d.y as f64 / dt.max(1e-3)];
            self.drag_vel = [self.drag_vel[0] * 0.55 + inst[0] * 0.45, self.drag_vel[1] * 0.55 + inst[1] * 0.45];
            if d != egui::Vec2::ZERO {
                self.last_drag = self.anim_t;
            }
            self.dismiss_intro();
        } else {
            if resp.drag_stopped() && self.anim_t - self.last_drag < 0.08 {
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
                    Some(obj) => self.dive_to(obj),
                    None => {
                        let world = proj.unproject(s[0] as f64, s[1] as f64);
                        self.follow = None;
                        let mpp = self.cam.mpp / 2.5;
                        self.cam.fly_to(world, mpp, rect.width() as f64, 0.7);
                    }
                }
            } else if resp.clicked() {
                self.pan_vel = [0.0, 0.0];
                self.focus = hit;
                // following continues only for the thing already being followed
                if self.follow.map(|(o, _)| Some(o) != hit).unwrap_or(false) {
                    self.follow = None;
                }
            }
        }

        // ---- keys, suppressed while typing
        self.key_pan = [0.0, 0.0];
        if ctx.wants_keyboard_input() {
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
                a.push(if i.modifiers.shift { KeyAction::Help } else { KeyAction::Search });
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
            self.zoom_anchor = Some((cx, cy, proj.unproject_local(cx, cy)));
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
                    self.follow = None;
                }
            }
            KeyAction::TogglePlay => {
                self.clock.playing = !self.clock.playing;
                self.modes.living = self.clock.playing;
            }
            KeyAction::Truth => self.modes.truth = !self.modes.truth,
            KeyAction::Gravity => {
                if matches!(self.focus, Some(ObjectRef::Planet(_)) | Some(ObjectRef::Moon(_))) {
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
        // bonus so labels don't flicker as the camera drifts (UX B3)
        let mut order: Vec<usize> = (0..fo.labels.len()).collect();
        order.sort_by_key(|&i| {
            -(fo.labels[i].priority + if self.sticky.contains(&fo.labels[i].text) { 12 } else { 0 })
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
            ui::halo_text(&painter, pos, Align2::CENTER_CENTER, &l.text, font, col, l.color[3]);
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
            this.picks.iter().find(|p| p.obj == obj).map(|p| p.r).unwrap_or(8.0)
        };
        if let Some(obj) = self.hover {
            if Some(obj) != self.focus {
                let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
                let s = proj.px(p);
                let c = egui::pos2(rect.left() + s[0], rect.top() + s[1]);
                let r = body_r(self, obj).max(6.0) + 5.0;
                if r < 90.0 {
                    painter.circle_stroke(c, r, egui::Stroke::new(1.0, theme::col32(theme::INK_WHITE, 0.45)));
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

    fn card_data(
        &self,
        obj: ObjectRef,
    ) -> (String, String, Vec<(String, String)>, Vec<(String, crate::evidence::Ev)>, bool) {
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
                if self.bank.status.on_disk(crate::engine::textures::TexId::Sun) {
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
                    let txt = if res < 10.0 { format!("{res:.2} km") } else { format!("{res:.0} km") };
                    rows.push(("state vs Horizons now".into(), txt));
                    provs.push(("Horizons cross-check".into(), Ev::HorizonsVectors));
                    provs.push(("residual interpolation".into(), Ev::HorizonsInterp));
                }
                if let Some(isl) = &self.island {
                    let e = isl.sim.energy_scaled();
                    let drift = if isl.e0.abs() > 0.0 { (e - isl.e0) / isl.e0.abs() } else { 0.0 };
                    rows.push(("island energy drift".into(), format!("{:.2} ppm", drift * 1e6)));
                    provs.push(("island diagnostics".into(), Ev::NBodyIsland));
                }
                if sim::eph_is_spice(jd) {
                    provs.push(("state".into(), Ev::De440));
                    provs.push(("orbit engraving".into(), Ev::ApproxElements));
                } else {
                    provs.push(("state + orbit".into(), Ev::ApproxElements));
                }
                let has_map = self.bank.status.any_planet_imagery() && p.name != "Pluto";
                provs.push((
                    "surface".into(),
                    if has_map { Ev::SurfaceImagery } else { Ev::ProceduralSurface },
                ));
                provs.push(("orientation".into(), Ev::IauRotation));
                provs.push(("atmosphere glow".into(), Ev::AtmosphereGlow));
                if i == 2 {
                    provs.push(("coastlines (Truth mode)".into(), Ev::Coastlines));
                    if !self.bank.status.on_disk(crate::engine::textures::TexId::EarthNight) {
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
                        if m.name == "Moon" { Ev::De440 } else { Ev::SatelliteSpk },
                    ));
                } else if m.name == "Moon" {
                    provs.push(("state".into(), Ev::LunaModel));
                    provs.push(("orbital phase".into(), Ev::MoonPhase));
                } else {
                    provs.push(("state".into(), Ev::MoonMeanElements));
                    provs.push(("orbital phase".into(), Ev::MoonPhase));
                }
                if m.name == "Moon" && u.moon_truth.is_some() {
                    provs.push(("dashed arc".into(), Ev::HorizonsVectors));
                }
                can_awaken = true;
            }
            ObjectRef::Craft(ci) => {
                let c = &u.craft[ci];
                subtitle = format!("Spacecraft: {}", c.note);
                let pos = obj_pos(u, obj, jd, None);
                rows.push(("distance from Sun".into(), format!("{:.2} AU", pos.length() / AU)));
                let (jd0, jd1) = c.track.span();
                rows.push(("trajectory arc".into(), format!("{} → {}", &fmt_jd(jd0)[..11], &fmt_jd(jd1)[..11])));
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
                    format!("{:.2} pc  ({:.1} ly)", s.dist_pc, s.dist_pc * 3.2616)
                };
                rows.push(("distance".into(), dist));
                rows.push(("G magnitude".into(), format!("{:.2}", s.gmag)));
                rows.push(("absolute mag".into(), format!("{:.2}", s.absmag)));
                rows.push(("BP–RP colour".into(), format!("{:.2}", s.bp_rp)));
                let kms = s.drift.length() / 3.155_76e10; // m/yr → km/s
                rows.push(("space motion".into(), format!("{kms:.1} km/s")));
                provs.push((
                    "astrometry".into(),
                    if s.compiled { Ev::GaiaCompiled } else { Ev::GaiaDr3 },
                ));
                provs.push(("position at this epoch".into(), Ev::ProperMotion));
            }
            ObjectRef::ExoHost(ei) => {
                let s = &u.exos[ei];
                subtitle = format!("Star with {} known planets", s.planets.len());
                if let Some(d) = s.dist_pc {
                    rows.push(("distance".into(), format!("{d:.1} pc")));
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
                subtitle = format!("Exoplanet, found by {}{}", p.method.to_lowercase(), p.year.map(|y| format!(" in {y}")).unwrap_or_default());
                if let Some(per) = p.per_d {
                    rows.push(("period".into(), format!("{per:.3} d")));
                    provs.push(("period".into(), Ev::ExoArchive));
                }
                match s.a_au_of(p) {
                    Some((a, Prov::Measured)) => {
                        rows.push(("semi-major axis".into(), format!("{a:.4} AU")));
                    }
                    Some((a, _)) => {
                        rows.push(("semi-major axis".into(), format!("{a:.4} AU (from Kepler's 3rd)")));
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
                subtitle = if g.spiral { "Spiral galaxy in the Local Group".into() } else { "Dwarf galaxy in the Local Group".into() };
                rows.push(("distance".into(), format!("{:.0} kpc", g.dist_kpc)));
                rows.push(("size".into(), format!("~{:.1} kpc", g.size_kpc)));
                provs.push(("position & distance".into(), Ev::CompiledCatalog));
                provs.push(("appearance".into(), Ev::GlyphIllustrative));
            }
            ObjectRef::Node(ni) => {
                let n = &u.nodes[ni];
                subtitle = "Galaxy cluster or attractor".into();
                rows.push(("distance".into(), format!("{:.0} Mpc", n.dist_mpc)));
                rows.push(("size".into(), format!("~{:.0} Mpc", n.size_mpc)));
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
    #[cfg(feature = "audio")]
    ambience: Option<crate::audio::Ambience>,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let rs = cc.wgpu_render_state.as_ref().expect("wgpu backend required").clone();
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
            a.set_gain(if want { self.app.settings.ambience_vol } else { 0.0 });
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
                // the canvas renders in physical pixels (AUDIT FND-015); all
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
                let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                self.app.handle_input(ctx, &resp, rect);
                self.app.tick(dt);
                let (target, tex) = self.canvas.as_ref().unwrap();
                self.app.build_canvas(rect.width() as f64, rect.height() as f64, target);
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
    // persistent final compositing target (AUDIT FND-025: no per-frame allocs)
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
        app.status = None;
        Ok(Self { app, ctx, rend, canvas: None, final_target: None })
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
            self.rend.update_texture(&self.app.gpu.device, &self.app.gpu.queue, *id, delta);
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
        let mut encoder = self.app.gpu.device.create_command_encoder(&Default::default());
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
        self.app.gpu.queue.submit(cmds.into_iter().chain([encoder.finish()]));
        for id in &full.textures_delta.free {
            self.rend.free_texture(id);
        }
        if read {
            self.final_target.as_ref().unwrap().read_pixels(&self.app.gpu)
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
    // track the subject WITHOUT recentring on it (AUDIT FND-017: the offset
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
        // reset ALL transient state between shots — deterministic procedural
        // time included (AUDIT REND-033)
        host.app.modes = Modes::default();
        host.app.island = None;
        host.app.play = None;
        host.app.focus = None;
        host.app.scene_exo = None;
        host.app.follow = None;
        host.app.cam = Camera::new();
        host.app.anim_t = 0.0;
        host.app.setup_error = None;
        (shot.setup)(&mut host.app);
        if let Some(e) = host.app.setup_error.take() {
            failures.push(format!("{}: {e}", shot.name));
            continue;
        }
        // shots are framed for a 1600 px reference width; rescale so the
        // physical view width is resolution-independent (AUDIT FND-016)
        host.app.cam.mpp *= 1600.0 / w as f64;
        let path = out.join(format!("{}.png", shot.name));
        host.export_png(w, h, &path)?;
        println!("wrote {}", path.display());
        done += 1;
    }
    anyhow::ensure!(done > 0, "no shot named '{which}' (try --list-shots)");
    anyhow::ensure!(failures.is_empty(), "shot setup failed: {}", failures.join("; "));
    Ok(())
}

pub fn run_path(path: &std::path::Path, w: u32, h: u32, fps: u32, out: &std::path::Path) -> anyhow::Result<()> {
    let keys: Vec<PathKey> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    anyhow::ensure!(!keys.is_empty(), "path file has no keyframes");
    let mut host = Headless::new()?;
    host.app.cinema = true;
    // jump instantly to the first key, then play the rest
    host.app.apply_key(&keys[0]);
    if let Some(a) = host.app.cam.anim.take() {
        host.app.cam.center = a.c1;
        host.app.cam.mpp = 10f64.powf(a.lm1);
    }
    host.app.play = Some(PathPlay { keys: keys.clone(), k: 0, t: 0.0 });
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
