//! Application shell shared by the interactive window and headless export:
//! one code path builds the atlas frame, renders the canvas, and lays the
//! glass UI (cards, ladder, time bar, truth legend, capture) over it.

use std::time::Instant;

use egui::{Align2, Color32, FontFamily, FontId, Rect};
use glam::DVec3;

use crate::atlas::{
    self, build, obj_half_extent, obj_name, obj_pos, BuildIn, FrameOut, Island, Modes,
    ObjectRef, Pickable,
};
use crate::data::{Prov, Universe};
use crate::engine::draw::{DrawList, Pipelines};
use crate::engine::gpu::{save_png, GpuContext, ViewTarget};
use crate::frames::{fmt_len, Camera, AU, PC};
use crate::sim::{self, fmt_jd, jd_now, Clock, NBody, J2000, MOONS, PLANETS};
use crate::theme;

// ------------------------------------------------------------- state

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PathKey {
    #[serde(default)]
    pub center: [f64; 3],
    pub mpp: f64,
    #[serde(default)]
    pub tilt: f64,
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

pub struct Pantheon {
    pub gpu: GpuContext,
    pub pipes: Pipelines,
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
    /// cinematic renders: canvas + labels + scale chip only, no control panels
    pub cinema: bool,
    /// set by shot setups that could not resolve their subject
    pub setup_error: Option<String>,
    pub status: Option<(String, f64)>,
    // ambience controls (the engine itself lives in the GUI shell)
    pub ambience_on: bool,
    pub ambience_vol: f32,
    /// glide the clock during path playback instead of jumping (AUDIT UI-020)
    pub jd_glide: Option<(f64, f64)>,
    pub search_open: bool,
    pub search_q: String,
    /// labels placed last frame — hysteresis so labels don't flicker (UX B3)
    sticky: std::collections::HashSet<String>,
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
        // flagship start: Earth, close
        cam.center = sim::planet_pos(2, clock.jd);
        cam.mpp = 3.6e7 / 1600.0;
        cam.tilt = 0.0;
        let pipes = Pipelines::new(&gpu);
        Self {
            gpu,
            pipes,
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
            cinema: false,
            setup_error: None,
            status: problems
                .first()
                .map(|p| (format!("⚠ degraded data — {p}"), 10.0)),
            ambience_on: true,
            ambience_vol: 0.5,
            jd_glide: None,
            search_open: false,
            search_q: String::new(),
            sticky: std::collections::HashSet::new(),
        }
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
                        self.note("path finished");
                    }
                }
            }
        }
        // path glides: ease clock + tilt alongside the camera animation
        if let Some((jd1, tilt1)) = self.jd_glide {
            match &self.cam.anim {
                Some(a) => {
                    let s = (a.t / a.dur).clamp(0.0, 1.0);
                    let e = s * s * (3.0 - 2.0 * s);
                    self.clock.jd += (jd1 - self.clock.jd) * e.max(0.02);
                    self.cam.tilt += (tilt1 - self.cam.tilt) * e.max(0.02);
                }
                None => {
                    self.clock.jd = jd1;
                    self.cam.tilt = tilt1;
                    self.jd_glide = None;
                }
            }
        }
        self.cam.tick(dt);
        // one time switch: play/pause is the source of truth, "living" derives
        self.modes.living = self.clock.playing;
        self.clock.tick(dt);

        // follow a moving focus so Living Orbits keeps the subject centered
        if self.cam.anim.is_none() {
            if let Some((obj, off)) = self.follow {
                let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
                self.cam.center = p + off;
            }
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
        // exoplanet pocket persists until the camera actually leaves it
        if let Some(ei) = self.scene_exo {
            let lw = self.cam.log_width(1600.0);
            let d = (self.cam.center - self.u.exos[ei].pos).length();
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

    fn apply_key(&mut self, key: &PathKey) {
        if let Some(host) = &key.host {
            if let Some(ei) = self.u.find_system(host) {
                self.focus = Some(ObjectRef::ExoHost(ei));
                self.scene_exo = Some(ei);
            }
        }
        self.follow = None;
        // glide time and tilt with the camera instead of jumping (AUDIT UI-020)
        self.jd_glide = Some((key.jd, key.tilt));
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
        self.cam.fly_to(center, key.mpp, 1600.0, key.dur.max(0.05));
    }

    pub fn dive_to(&mut self, obj: ObjectRef, w: f64, h: f64) {
        if let ObjectRef::ExoHost(ei) | ObjectRef::ExoPlanet(ei, _) = obj {
            if self.u.exos[ei].dist_pc.is_none() {
                self.note("distance unknown — this system cannot be charted");
                return;
            }
            self.scene_exo = Some(ei);
        }
        let pos = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
        let ext = obj_half_extent(&self.u, obj);
        let mpp = (ext * 2.0) / (0.62 * h.min(w));
        self.focus = Some(obj);
        self.follow = Some((obj, DVec3::ZERO));
        let d_log = (self.cam.mpp / mpp).abs().log10().abs();
        self.cam.fly_to(pos, mpp, w, (0.9 + 0.35 * d_log).clamp(0.9, 4.2));
        let name = obj_name(&self.u, obj);
        self.note(format!("→ {name}"));
    }

    pub fn awaken(&mut self) {
        let Some(focus) = self.focus else { return };
        let pi = match focus {
            ObjectRef::Planet(i) => i,
            ObjectRef::Moon(mi) => MOONS[mi].planet,
            _ => {
                self.note("awakening needs a planet or moon focus");
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
        self.note(format!("{} system awakened — local N-body live", PLANETS[pi].name));
    }

    // ------------------------------------------------------ frame build

    pub fn build_canvas(&mut self, w: f64, h: f64, target: &ViewTarget) {
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
        };
        let fo = build(&inp, &mut self.dl);
        // keep a copy of pickables for next frame's input hit tests
        self.picks.clear();
        self.picks.extend(fo.picks.iter().map(|p| Pickable { pos: p.pos, r: p.r, obj: p.obj }));
        let uni = fo.uni;
        self.fo = Some(fo);
        self.pipes.render(&self.gpu, target, &self.dl, &uni);
    }

    // -------------------------------------------------------------- input

    pub fn handle_input(&mut self, ctx: &egui::Context, resp: &egui::Response, rect: Rect) {
        let (scroll, pointer, any_down, dbl) = ctx.input(|i| {
            (
                i.smooth_scroll_delta.y,
                i.pointer.hover_pos(),
                i.pointer.any_down(),
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
            )
        });
        let proj = crate::frames::Proj::new(
            &self.cam,
            rect.width() as f64,
            rect.height() as f64,
            &self.u.gal,
        );
        // wheel zoom anchored at the cursor — solved against the NEW basis,
        // which can rotate with scale (AUDIT FND-022)
        if scroll.abs() > 0.01 && resp.hovered() {
            let f = (-scroll as f64 * 0.0022).exp();
            let new = (self.cam.mpp * f).clamp(crate::frames::MPP_MIN, crate::frames::MPP_MAX);
            if let Some(p) = pointer {
                let sx = (p.x - rect.left()) as f64;
                let sy = (p.y - rect.top()) as f64;
                let world = proj.unproject(sx, sy);
                self.cam.mpp = new;
                let proj2 = crate::frames::Proj::new(
                    &self.cam,
                    rect.width() as f64,
                    rect.height() as f64,
                    &self.u.gal,
                );
                self.cam.center = world
                    - proj2.right * ((sx - proj2.w * 0.5) * new)
                    - proj2.up * ((proj2.h * 0.5 - sy) * new);
            } else {
                self.cam.mpp = new;
            }
            self.cam.anim = None;
            self.play = None;
            self.sync_follow();
        }
        // drag pan / tilt
        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            self.cam.center -= proj.right * (d.x as f64 * self.cam.mpp);
            self.cam.center += proj.up * (d.y as f64 * self.cam.mpp);
            self.cam.anim = None;
            self.play = None;
            self.sync_follow();
        }
        if resp.dragged_by(egui::PointerButton::Secondary) {
            let d = resp.drag_delta();
            self.cam.tilt = (self.cam.tilt + d.y as f64 * 0.004).clamp(-0.2, 1.30);
        }
        // click select / double-click dive
        if (resp.clicked() || dbl) && !any_down {
            if let Some(p) = pointer {
                let sp = [(p.x - rect.left()), (p.y - rect.top())];
                let mut best: Option<(f32, ObjectRef)> = None;
                for pk in &self.picks {
                    let dx = pk.pos[0] - sp[0];
                    let dy = pk.pos[1] - sp[1];
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < pk.r.max(12.0) && best.map(|(b, _)| d < b).unwrap_or(true) {
                        best = Some((d, pk.obj));
                    }
                }
                match best {
                    Some((_, obj)) => {
                        if dbl {
                            self.dive_to(obj, rect.width() as f64, rect.height() as f64);
                        } else {
                            self.focus = Some(obj);
                        }
                    }
                    None => {
                        // standard map behavior (UX C5): empty double-click
                        // zooms in at the cursor; empty click deselects
                        if dbl {
                            let sx = (p.x - rect.left()) as f64;
                            let sy = (p.y - rect.top()) as f64;
                            let world = proj.unproject(sx, sy);
                            self.follow = None;
                            self.cam.fly_to(
                                world,
                                self.cam.mpp / 2.5,
                                rect.width() as f64,
                                0.7,
                            );
                        } else {
                            self.focus = None;
                        }
                    }
                }
            }
        }
        // keys — suppressed while a text field is focused
        let typing = ctx.memory(|m| m.focused().is_some());
        if typing {
            return;
        }
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                self.focus = None;
                self.follow = None;
            }
            if i.key_pressed(egui::Key::L) {
                self.modes.living = !self.modes.living;
                self.clock.playing = self.modes.living;
            }
            if i.key_pressed(egui::Key::T) {
                self.modes.truth = !self.modes.truth;
            }
            if i.key_pressed(egui::Key::G) {
                self.modes.gravity = !self.modes.gravity;
            }
            if i.key_pressed(egui::Key::Y) {
                self.modes.system_view = !self.modes.system_view;
            }
            if i.key_pressed(egui::Key::Slash) {
                self.search_open = true;
            }
            if i.key_pressed(egui::Key::H) {
                let jd = self.clock.jd;
                self.focus = Some(ObjectRef::Planet(2));
                self.follow = Some((ObjectRef::Planet(2), DVec3::ZERO));
                self.cam.fly_to(sim::planet_pos(2, jd), 3.6e7 / rect.width() as f64, rect.width() as f64, 2.4);
            }
        });
    }

    fn sync_follow(&mut self) {
        if let Some((obj, _)) = self.follow {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            let off = self.cam.center - p;
            // release once the subject is far outside the view
            if off.length() > 12.0 * self.cam.mpp * 1600.0 {
                self.follow = None;
            } else {
                self.follow = Some((obj, off));
            }
        }
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
        let mut placed: Vec<Rect> = Vec::new();
        let mut placed_texts: std::collections::HashSet<String> = Default::default();
        for i in order {
            let l = &fo.labels[i];
            let pos = egui::pos2(rect.left() + l.pos[0], rect.top() + l.pos[1]);
            let w = l.text.chars().count() as f32 * l.size * 0.52 + 6.0;
            let r = Rect::from_center_size(pos, egui::vec2(w, l.size + 5.0));
            if !rect.intersects(r) || placed.iter().any(|p| p.intersects(r)) {
                continue;
            }
            placed.push(r);
            placed_texts.insert(l.text.clone());
            let col = theme::col32([l.color[0], l.color[1], l.color[2]], l.color[3]);
            let font = FontId::new(l.size, FontFamily::Proportional);
            painter.text(
                pos + egui::vec2(0.8, 0.8),
                Align2::CENTER_CENTER,
                &l.text,
                font.clone(),
                Color32::from_black_alpha(200),
            );
            painter.text(pos, Align2::CENTER_CENTER, &l.text, font, col);
        }
        self.sticky = placed_texts;
        // focus reticle
        if let Some(obj) = self.focus {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            let proj = crate::frames::Proj::new(
                &self.cam,
                rect.width() as f64,
                rect.height() as f64,
                &self.u.gal,
            );
            let s = proj.px(p);
            let c = egui::pos2(rect.left() + s[0], rect.top() + s[1]);
            if rect.contains(c) {
                let r = 14.0;
                for (a0, a1) in [(0.1f32, 0.4f32), (1.67, 1.97), (3.24, 3.54), (4.81, 5.11)] {
                    let pts: Vec<egui::Pos2> = (0..=8)
                        .map(|k| {
                            let a = a0 + (a1 - a0) * k as f32 / 8.0;
                            egui::pos2(c.x + r * a.cos(), c.y + r * a.sin())
                        })
                        .collect();
                    painter.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(1.0, theme::col32(theme::INK_GOLD, 0.85)),
                    ));
                }
            }
        }
    }

    // ------------------------------------------------------------ panels

    /// Search over everything nameable: planets, moons, craft, named stars,
    /// exoplanet hosts (UX C1).
    fn search_results(&self) -> Vec<(String, ObjectRef)> {
        let q = self.search_q.trim().to_lowercase();
        if q.len() < 2 {
            return Vec::new();
        }
        let mut out: Vec<(String, ObjectRef)> = Vec::new();
        for (i, p) in PLANETS.iter().enumerate() {
            if p.name.to_lowercase().contains(&q) {
                out.push((format!("{} — planet", p.name), ObjectRef::Planet(i)));
            }
        }
        for (i, m) in MOONS.iter().enumerate() {
            if m.name.to_lowercase().contains(&q) {
                out.push((
                    format!("{} — moon of {}", m.name, PLANETS[m.planet].name),
                    ObjectRef::Moon(i),
                ));
            }
        }
        for (i, c) in self.u.craft.iter().enumerate() {
            if c.name.to_lowercase().contains(&q) {
                out.push((format!("{} — spacecraft", c.name), ObjectRef::Craft(i)));
            }
        }
        for (i, s) in self.u.stars.iter().enumerate() {
            if let Some(n) = s.name {
                if n.to_lowercase().contains(&q) {
                    out.push((format!("{n} — star, {:.1} pc", s.dist_pc), ObjectRef::Star(i)));
                }
            }
        }
        let mut ex = 0;
        for (i, e) in self.u.exos.iter().enumerate() {
            if e.dist_pc.is_some() && e.host.to_lowercase().contains(&q) {
                out.push((
                    format!("{} — {} planets", e.host, e.planets.len()),
                    ObjectRef::ExoHost(i),
                ));
                ex += 1;
                if ex > 8 {
                    break;
                }
            }
        }
        out.truncate(12);
        out
    }

    fn search_panel(&mut self, ctx: &egui::Context, rect: Rect) {
        let results = self.search_results();
        let mut dive: Option<ObjectRef> = None;
        let mut close = false;
        egui::Area::new(egui::Id::new("search"))
            .anchor(Align2::CENTER_TOP, [0.0, 56.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.set_width(330.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("⌕").size(15.0).color(theme::GOLD));
                        let te = ui.add(
                            egui::TextEdit::singleline(&mut self.search_q)
                                .hint_text("planet, moon, star, craft, exo system…")
                                .desired_width(260.0),
                        );
                        te.request_focus();
                        if ui.button("✕").clicked() {
                            close = true;
                        }
                    });
                    for (label, obj) in &results {
                        if ui
                            .selectable_label(false, egui::RichText::new(label).size(12.0))
                            .clicked()
                        {
                            dive = Some(*obj);
                        }
                    }
                    if results.is_empty() && self.search_q.trim().len() >= 2 {
                        ui.label(
                            egui::RichText::new("no match").size(11.0).color(theme::TEXT_DIM),
                        );
                    }
                });
            });
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search_open = false;
        }
        if let Some(obj) = dive {
            self.search_open = false;
            self.search_q.clear();
            self.dive_to(obj, rect.width() as f64, rect.height() as f64);
        }
    }

    pub fn draw_panels(&mut self, ctx: &egui::Context, rect: Rect) {
        self.top_chip(ctx);
        if self.cinema {
            return;
        }
        if self.search_open {
            self.search_panel(ctx, rect);
        }
        // persistent control hints (UX C2)
        egui::Area::new(egui::Id::new("hints"))
            .anchor(Align2::RIGHT_BOTTOM, [-14.0, -12.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "scroll zoom · drag pan · double-click dive · right-drag tilt · / search",
                    )
                    .size(10.5)
                    .color(theme::col32(theme::INK_FAINT, 1.0)),
                );
            });
        self.mode_rail(ctx, rect);
        if self.modes.ladder {
            self.ladder(ctx, rect);
        }
        self.time_bar(ctx, rect);
        if self.focus.is_some() {
            self.object_card(ctx, rect);
        }
        if self.modes.truth {
            self.truth_legend(ctx);
        }
        if self.show_capture {
            self.capture_panel(ctx, rect);
        }
        if self.show_help {
            egui::Area::new(egui::Id::new("help"))
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    egui::Frame::window(&ctx.style()).show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.label(
                            egui::RichText::new("CONTROLS")
                                .font(FontId::new(17.0, FontFamily::Name("heading".into())))
                                .color(theme::GOLD),
                        );
                        for (k, v) in [
                            ("wheel", "infinite zoom at the cursor"),
                            ("left drag", "pan"),
                            ("right drag ↕", "tilt the atlas plane"),
                            ("click / double-click", "select / dive"),
                            ("L", "Living Orbits (time animation)"),
                            ("T", "Truth mode — ink by evidence"),
                            ("G", "Gravity view (L-points, Hill, Roche)"),
                            ("Y", "System View (log-spaced schematic)"),
                            ("H", "home (Earth)"),
                            ("Esc", "release selection"),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(k).size(12.0).color(theme::GOLD),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(v)
                                                .size(11.5)
                                                .color(theme::TEXT_DIM),
                                        );
                                    },
                                );
                            });
                        }
                        if ui.button("close").clicked() {
                            self.show_help = false;
                        }
                    });
                });
        }
        if let Some((msg, _)) = &self.status {
            let msg = msg.clone();
            egui::Area::new(egui::Id::new("status"))
                .anchor(Align2::CENTER_BOTTOM, [0.0, -64.0])
                .show(ctx, |ui| {
                    egui::Frame::window(&ctx.style()).show(ui, |ui| {
                        ui.label(egui::RichText::new(msg).color(theme::GOLD).size(13.0));
                    });
                });
        }
    }

    fn top_chip(&mut self, ctx: &egui::Context) {
        let Some(fo) = &self.fo else { return };
        let band = fo.band_name.clone();
        let scale = fo.scale_text.clone();
        egui::Area::new(egui::Id::new("chip"))
            .anchor(Align2::LEFT_TOP, [14.0, 12.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("PANTHEON")
                                .font(FontId::new(19.0, FontFamily::Name("heading".into())))
                                .color(theme::GOLD),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(&band)
                                .size(13.5)
                                .color(theme::TEXT),
                        );
                    });
                    ui.label(egui::RichText::new(&scale).size(11.5).color(theme::TEXT_DIM));
                    // active projection frame — the reorientation is deliberate,
                    // so say so instead of feeling like a glitch (UX B4)
                    let lw = self.cam.log_width(1600.0);
                    let frame = if lw < 18.3 {
                        "ecliptic frame"
                    } else if lw < 19.3 {
                        "reorienting → galactic"
                    } else if lw < 22.3 {
                        "galactic frame"
                    } else if lw < 23.2 {
                        "reorienting → supergalactic"
                    } else {
                        "supergalactic frame"
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(fmt_jd(self.clock.jd))
                                .size(11.5)
                                .color(theme::col32(theme::INK_BLUE, 0.9)),
                        );
                        ui.label(
                            egui::RichText::new(format!("· {frame}"))
                                .size(10.5)
                                .color(theme::TEXT_DIM),
                        );
                    });
                });
            });
    }

    fn mode_rail(&mut self, ctx: &egui::Context, _rect: Rect) {
        egui::Area::new(egui::Id::new("rail"))
            .anchor(Align2::LEFT_TOP, [14.0, 118.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.set_width(118.0);
                    let mut b = |ui: &mut egui::Ui, on: &mut bool, label: &str| {
                        if ui.selectable_label(*on, label).clicked() {
                            *on = !*on;
                        }
                    };
                    let mut playing = self.clock.playing;
                    b(ui, &mut playing, "⟳ Living Orbits");
                    self.clock.playing = playing;
                    b(ui, &mut self.modes.trails, "∿ Trails");
                    // capability-aware: disabled with a reason, never silent (UX C4)
                    let grav_ok = matches!(
                        self.focus,
                        Some(ObjectRef::Planet(_)) | Some(ObjectRef::Moon(_))
                    );
                    let r = ui
                        .add_enabled(
                            grav_ok,
                            egui::SelectableLabel::new(self.modes.gravity, "◉ Gravity"),
                        )
                        .on_disabled_hover_text("select a planet or moon first");
                    if r.clicked() {
                        self.modes.gravity = !self.modes.gravity;
                    }
                    b(ui, &mut self.modes.truth, "◈ Truth");
                    let sys_ok = self.scene_exo.is_some();
                    let r = ui
                        .add_enabled(
                            sys_ok,
                            egui::SelectableLabel::new(self.modes.system_view, "☰ System View"),
                        )
                        .on_disabled_hover_text("enter an exoplanet system first");
                    if r.clicked() {
                        self.modes.system_view = !self.modes.system_view;
                    }
                    b(ui, &mut self.modes.ladder, "☷ Ladder");
                    b(ui, &mut self.show_capture, "✧ Capture");
                    b(ui, &mut self.search_open, "⌕ Search");
                    ui.separator();
                    b(ui, &mut self.ambience_on, "♪ Ambience");
                    if self.ambience_on {
                        ui.add(
                            egui::Slider::new(&mut self.ambience_vol, 0.0..=1.0)
                                .show_value(false),
                        );
                    }
                    b(ui, &mut self.show_help, "? Help");
                    ui.separator();
                    if ui.selectable_label(false, "⌂ Home (Earth)").clicked() {
                        let jd = self.clock.jd;
                        self.focus = Some(ObjectRef::Planet(2));
                        self.follow = Some((ObjectRef::Planet(2), DVec3::ZERO));
                        self.cam.fly_to(sim::planet_pos(2, jd), 3.6e7 / 1600.0, 1600.0, 2.4);
                    }
                });
            });
    }

    fn ladder(&mut self, ctx: &egui::Context, rect: Rect) {
        egui::Area::new(egui::Id::new("ladder"))
            .anchor(Align2::RIGHT_CENTER, [-12.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    let h = (rect.height() * 0.62).clamp(300.0, 620.0);
                    let (resp, p) =
                        ui.allocate_painter(egui::vec2(150.0, h), egui::Sense::click());
                    let r = resp.rect;
                    let (lo, hi) = (6.0f32, 26.5f32);
                    let y_of = |lwv: f32| r.bottom() - (lwv - lo) / (hi - lo) * r.height();
                    p.line_segment(
                        [egui::pos2(r.left() + 12.0, r.top()), egui::pos2(r.left() + 12.0, r.bottom())],
                        egui::Stroke::new(1.0, theme::GOLD_DIM),
                    );
                    let rungs: [(f32, &str); 11] = [
                        (7.0, "Planet"),
                        (9.0, "Moons"),
                        (11.0, "Inner System"),
                        (13.0, "Outer System"),
                        (14.5, "Heliosphere"),
                        (16.0, "Oort Cloud"),
                        (17.5, "Near Stars"),
                        (19.0, "Deep Stars"),
                        (21.0, "Galaxy"),
                        (23.0, "Local Group"),
                        (24.8, "Cosmic Web"),
                    ];
                    for (lwv, name) in rungs {
                        let y = y_of(lwv);
                        p.line_segment(
                            [egui::pos2(r.left() + 6.0, y), egui::pos2(r.left() + 18.0, y)],
                            egui::Stroke::new(1.0, theme::GOLD_DIM),
                        );
                        p.text(
                            egui::pos2(r.left() + 24.0, y),
                            Align2::LEFT_CENTER,
                            name,
                            FontId::new(11.5, FontFamily::Proportional),
                            theme::TEXT_DIM,
                        );
                        p.text(
                            egui::pos2(r.right() - 4.0, y),
                            Align2::RIGHT_CENTER,
                            format!("10^{lwv:.0}"),
                            FontId::new(9.5, FontFamily::Proportional),
                            theme::col32(theme::INK_FAINT, 1.0),
                        );
                    }
                    // current position marker
                    let lw_now = self.cam.log_width(rect.width() as f64) as f32;
                    let y = y_of(lw_now.clamp(lo, hi));
                    p.circle_stroke(
                        egui::pos2(r.left() + 12.0, y),
                        5.0,
                        egui::Stroke::new(1.4, theme::GOLD),
                    );
                    // click to jump — rungs carry semantic destinations, not
                    // just a scale (AUDIT FND-021)
                    if resp.clicked() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let lwv = (lo + (r.bottom() - pos.y) / r.height() * (hi - lo)) as f64;
                            let mpp = 10f64.powf(lwv) / rect.width() as f64;
                            let anchor = if lwv < 16.0 {
                                self.cam.center // stay with the current subject
                            } else if lwv < 20.0 {
                                DVec3::ZERO // the Sun anchors stellar scales
                            } else if lwv < 22.4 {
                                self.u.gal.center // the Galaxy band
                            } else {
                                DVec3::ZERO // Local Group / web charts around the MW
                            };
                            self.follow = None;
                            self.cam.fly_to(anchor, mpp, rect.width() as f64, 1.8);
                        }
                    }
                    ui.label(
                        egui::RichText::new("SCALE LADDER · log₁₀ view width (m)")
                            .size(10.0)
                            .color(theme::TEXT_DIM),
                    );
                });
            });
    }

    fn time_bar(&mut self, ctx: &egui::Context, _rect: Rect) {
        egui::Area::new(egui::Id::new("timebar"))
            .anchor(Align2::CENTER_BOTTOM, [0.0, -12.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let play_label = if self.clock.playing { "⏸" } else { "▶" };
                        if ui.button(play_label).clicked() {
                            self.clock.playing = !self.clock.playing;
                            if self.clock.playing {
                                self.modes.living = true;
                            }
                        }
                        if ui.button("now").clicked() {
                            self.clock.jd = jd_now();
                        }
                        ui.separator();
                        for (label, rate) in [
                            ("1s", 1.0),
                            ("1m", 60.0),
                            ("1h", 3_600.0),
                            ("1d", 86_400.0),
                            ("1mo", 2.63e6),
                            ("1yr", 3.156e7),
                            ("1kyr", 3.156e10),
                            ("1Myr", 3.156e13),
                        ] {
                            let on = (self.clock.rate - rate).abs() / rate < 0.01;
                            if ui.selectable_label(on, label).clicked() {
                                self.clock.rate = rate;
                            }
                        }
                        ui.separator();
                        // time machine scrub ±centuries; write back only on real
                        // interaction (Slider display-rounding must never move time)
                        let mut yr = 2000.0 + (self.clock.jd - J2000) / 365.25;
                        let resp = ui.add(
                            egui::Slider::new(&mut yr, 1600.0..=2500.0)
                                .show_value(true)
                                .custom_formatter(|v, _| format!("{v:.1}")),
                        );
                        if resp.changed() && (resp.dragged() || resp.clicked() || resp.has_focus())
                        {
                            self.clock.jd = J2000 + (yr - 2000.0) * 365.25;
                        }
                        // fine control: the slider is ~4.5 yr per pixel (UX C7)
                        let mut yr_fine = 2000.0 + (self.clock.jd - J2000) / 365.25;
                        let dv = ui.add(
                            egui::DragValue::new(&mut yr_fine)
                                .speed(0.02)
                                .range(1600.0..=2500.0)
                                .suffix(" yr"),
                        );
                        if dv.changed() && (dv.dragged() || dv.has_focus()) {
                            self.clock.jd = J2000 + (yr_fine - 2000.0) * 365.25;
                        }
                    });
                });
            });
    }

    fn truth_legend(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("truthleg"))
            .anchor(
                Align2::RIGHT_TOP,
                [if self.modes.ladder { -200.0 } else { -12.0 }, 12.0],
            )
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("TRUTH MODE — ink by evidence")
                            .size(12.0)
                            .color(theme::GOLD),
                    );
                    for p in [
                        Prov::Measured,
                        Prov::Modeled,
                        Prov::Inferred,
                        Prov::Procedural,
                        Prov::Illustrative,
                        Prov::Unknown,
                    ] {
                        ui.horizontal(|ui| {
                            let (rc, _) =
                                ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter().circle_filled(rc.center(), 4.0, theme::col32(p.rgb(), 1.0));
                            ui.label(egui::RichText::new(p.label()).size(11.5));
                        });
                    }
                    ui.label(
                        egui::RichText::new(format!("data retrieved {}", self.u.retrieved))
                            .size(10.5)
                            .color(theme::TEXT_DIM),
                    );
                });
            });
    }

    fn capture_panel(&mut self, ctx: &egui::Context, rect: Rect) {
        egui::Area::new(egui::Id::new("capture"))
            .anchor(Align2::RIGHT_BOTTOM, [-12.0, -70.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.set_width(240.0);
                    ui.label(
                        egui::RichText::new("CINEMATIC CAPTURE").size(12.5).color(theme::GOLD),
                    );
                    ui.label(
                        egui::RichText::new(format!("{} keyframes", self.capture.len()))
                            .size(11.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("+ keyframe").clicked() {
                            let host = match self.focus {
                                Some(ObjectRef::ExoHost(i)) | Some(ObjectRef::ExoPlanet(i, _)) => {
                                    Some(self.u.exos[i].host.clone())
                                }
                                _ => None,
                            };
                            self.capture.push(PathKey {
                                center: [self.cam.center.x, self.cam.center.y, self.cam.center.z],
                                mpp: self.cam.mpp,
                                tilt: self.cam.tilt,
                                jd: self.clock.jd,
                                dur: 4.0,
                                hold: 1.0,
                                at: None,
                                host,
                            });
                        }
                        if ui.button("clear").clicked() {
                            self.capture.clear();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("▶ play").clicked() && !self.capture.is_empty() {
                            let keys = self.capture.clone();
                            self.apply_key(&keys[0]);
                            self.play = Some(PathPlay { keys, k: 0, t: 0.0 });
                        }
                        if ui.button("save path").clicked() {
                            let dir = std::path::PathBuf::from("exports/paths");
                            let stamp = crate::sim::fmt_jd(crate::sim::jd_now())
                                .replace([' ', ':'], "-");
                            let path = dir.join(format!("path_{stamp}.json"));
                            match std::fs::create_dir_all(&dir)
                                .map_err(anyhow::Error::from)
                                .and_then(|_| Ok(serde_json::to_string_pretty(&self.capture)?))
                                .and_then(|js| Ok(std::fs::write(&path, js)?))
                            {
                                Ok(()) => self.note(format!("saved {}", path.display())),
                                Err(e) => self.note(format!("save failed: {e}")),
                            }
                        }
                    });
                    ui.label(
                        egui::RichText::new("render: pantheon --path exports/paths/path.json")
                            .size(10.0)
                            .color(theme::TEXT_DIM),
                    );
                });
            });
        let _ = rect;
    }

    // -------------------------------------------------------- object card

    fn object_card(&mut self, ctx: &egui::Context, rect: Rect) {
        let Some(obj) = self.focus else { return };
        let (title, subtitle, rows, provs, can_awaken) = self.card_data(obj);
        let mut dive: Option<ObjectRef> = None;
        egui::Area::new(egui::Id::new("card"))
            .anchor(Align2::LEFT_BOTTOM, [14.0, -70.0])
            .show(ctx, |ui| {
                egui::Frame::window(&ctx.style()).show(ui, |ui| {
                    ui.set_width(270.0);
                    ui.label(
                        egui::RichText::new(&title)
                            .font(FontId::new(19.0, FontFamily::Name("heading".into())))
                            .color(theme::GOLD),
                    );
                    ui.label(egui::RichText::new(&subtitle).size(11.5).color(theme::TEXT_DIM));
                    ui.add_space(3.0);
                    for (k, v) in &rows {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(k).size(11.0).color(theme::TEXT_DIM),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(egui::RichText::new(v).size(11.5).color(theme::TEXT));
                                },
                            );
                        });
                    }
                    ui.add_space(3.0);
                    for (what, p) in &provs {
                        ui.horizontal(|ui| {
                            let (rc, _) = ui
                                .allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(rc.center(), 3.4, theme::col32(p.rgb(), 1.0));
                            ui.label(
                                egui::RichText::new(format!("{} — {}", p.label(), what))
                                    .size(10.5)
                                    .color(theme::TEXT_DIM),
                            );
                        });
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("◎ dive").clicked() {
                            dive = Some(obj);
                        }
                        if can_awaken {
                            let lbl = if self.island.is_some() { "☽ sleep" } else { "☀ awaken" };
                            if ui.button(lbl).clicked() {
                                if self.island.is_some() {
                                    self.island = None;
                                } else {
                                    self.awaken();
                                }
                            }
                        }
                        if ui.button("✕").clicked() {
                            self.focus = None;
                            self.follow = None;
                        }
                    });
                    if self.island_paused {
                        ui.label(
                            egui::RichText::new("island paused — time rate too high for N-body")
                                .size(10.0)
                                .color(theme::col32(Prov::Inferred.rgb(), 1.0)),
                        );
                    }
                });
            });
        if let Some(obj) = dive {
            self.dive_to(obj, rect.width() as f64, rect.height() as f64);
        }
    }

    fn card_data(
        &self,
        obj: ObjectRef,
    ) -> (String, String, Vec<(String, String)>, Vec<(String, Prov)>, bool) {
        let u = &self.u;
        let jd = self.clock.jd;
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut provs: Vec<(String, Prov)> = Vec::new();
        let mut can_awaken = false;
        let title = obj_name(u, obj);
        let subtitle;
        match obj {
            ObjectRef::Sun => {
                subtitle = "G2V star · the anchor of the atlas".into();
                rows.push(("radius".into(), "695,700 km".into()));
                rows.push(("GM".into(), "1.327e20 m³/s²".into()));
                provs.push(("physical constants (IAU)".into(), Prov::Measured));
                provs.push(("surface granulation".into(), Prov::Procedural));
            }
            ObjectRef::Planet(i) => {
                let p = &PLANETS[i];
                subtitle = "planet · Sol system".into();
                let pos = obj_pos(u, obj, jd, self.island.as_ref());
                rows.push(("distance from Sun".into(), fmt_len(pos.length())));
                rows.push(("semi-major axis".into(), format!("{:.3} AU", p.el[0])));
                rows.push(("eccentricity".into(), format!("{:.4}", p.el[1])));
                rows.push(("period".into(), format!("{:.2} yr", p.el[0].powf(1.5))));
                rows.push(("radius".into(), format!("{:.0} km", p.radius_km)));
                rows.push(("day length".into(), format!("{:.2} h", p.rot_hours.abs())));
                if let Some(res) = u.planet_residual_km(i, jd) {
                    rows.push(("model vs JPL now".into(), format!("{res:.0} km")));
                    provs.push(("JPL Horizons cross-check".into(), Prov::Measured));
                }
                if let Some(isl) = &self.island {
                    let e = isl.sim.energy_scaled();
                    let drift = if isl.e0.abs() > 0.0 { (e - isl.e0) / isl.e0.abs() } else { 0.0 };
                    rows.push(("island energy drift".into(), format!("{:.2} ppm", drift * 1e6)));
                    provs.push(("leapfrog island diagnostics".into(), Prov::Modeled));
                }
                provs.push(("orbit: JPL approx. elements 1800–2050".into(), Prov::Modeled));
                provs.push(("surface pattern".into(), Prov::Procedural));
                if i == 2 {
                    provs.push(("coastlines: Natural Earth 110m".into(), Prov::Measured));
                }
                can_awaken = true;
            }
            ObjectRef::Moon(mi) => {
                let m = &MOONS[mi];
                subtitle = format!("moon of {}", PLANETS[m.planet].name);
                rows.push(("semi-major axis".into(), format!("{:.0} km", m.a_km)));
                rows.push(("period".into(), format!("{:.3} d", m.period_d.abs())));
                rows.push(("radius".into(), format!("{:.0} km", m.radius_km)));
                rows.push(("eccentricity".into(), format!("{:.4}", m.e)));
                if m.period_d < 0.0 {
                    rows.push(("orbit".into(), "retrograde".into()));
                }
                let prov_orbit = if m.name == "Moon" {
                    "precessing-ellipse model"
                } else {
                    "mean elements, equatorial approx."
                };
                provs.push((prov_orbit.into(), Prov::Modeled));
                if m.name == "Moon" && u.moon_truth.is_some() {
                    provs.push(("JPL Horizons arc shown dashed".into(), Prov::Measured));
                }
                provs.push(("orbital phase".into(), if m.name == "Moon" { Prov::Modeled } else { Prov::Illustrative }));
                can_awaken = true;
            }
            ObjectRef::Craft(ci) => {
                let c = &u.craft[ci];
                subtitle = format!("spacecraft · {}", c.note);
                let pos = obj_pos(u, obj, jd, None);
                rows.push(("distance from Sun".into(), fmt_len(pos.length())));
                rows.push(("".into(), format!("{:.2} AU", pos.length() / AU)));
                let (jd0, jd1) = c.track.span();
                rows.push(("trajectory arc".into(), format!("{} → {}", &fmt_jd(jd0)[..11], &fmt_jd(jd1)[..11])));
                provs.push((format!("JPL Horizons, retrieved {}", u.retrieved), Prov::Measured));
                provs.push(("position between samples".into(), Prov::Inferred));
            }
            ObjectRef::Star(si) => {
                let s = &u.stars[si];
                subtitle = if s.compiled {
                    "star · compiled catalog".into()
                } else {
                    "star · Gaia DR3".into()
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
                    if s.compiled {
                        "Hipparcos-era compilation".into()
                    } else {
                        format!("Gaia DR3, retrieved {}", u.retrieved)
                    },
                    Prov::Measured,
                ));
                provs.push(("positions propagated by proper motion".into(), Prov::Modeled));
            }
            ObjectRef::ExoHost(ei) => {
                let s = &u.exos[ei];
                subtitle = format!("exoplanet host · {} planets", s.planets.len());
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
                provs.push((format!("NASA Exoplanet Archive, retrieved {}", u.retrieved), Prov::Measured));
                provs.push(("orbit geometry (nodes unknown)".into(), Prov::Illustrative));
            }
            ObjectRef::ExoPlanet(ei, k) => {
                let s = &u.exos[ei];
                let p = &s.planets[k];
                subtitle = format!("exoplanet · {} ({})", p.method, p.year.map(|y| y.to_string()).unwrap_or_default());
                if let Some(per) = p.per_d {
                    rows.push(("period".into(), format!("{per:.3} d")));
                    provs.push(("period".into(), Prov::Measured));
                }
                match s.a_au_of(p) {
                    Some((a, Prov::Measured)) => {
                        rows.push(("semi-major axis".into(), format!("{a:.4} AU")));
                    }
                    Some((a, _)) => {
                        rows.push(("semi-major axis".into(), format!("{a:.4} AU (from Kepler's 3rd)")));
                        provs.push(("semi-major axis".into(), Prov::Inferred));
                    }
                    None => {
                        rows.push(("semi-major axis".into(), "unknown".into()));
                        provs.push(("semi-major axis".into(), Prov::Unknown));
                    }
                }
                match p.rade {
                    Some(r) => rows.push(("radius".into(), format!("{r:.2} R⊕"))),
                    None => {
                        rows.push(("radius".into(), "unknown".into()));
                        provs.push(("radius".into(), Prov::Unknown));
                    }
                }
                if let Some(m) = p.masse {
                    rows.push(("mass".into(), format!("{m:.1} M⊕")));
                }
                if let Some(t) = p.eqt {
                    rows.push(("equilibrium T".into(), format!("{t:.0} K")));
                }
                if p.e.is_none() {
                    provs.push(("eccentricity assumed 0".into(), Prov::Inferred));
                }
                provs.push(("orbital phase".into(), Prov::Illustrative));
            }
            ObjectRef::Galaxy(gi) => {
                let g = &u.galaxies[gi];
                subtitle = if g.spiral { "spiral galaxy · Local Group".into() } else { "dwarf galaxy · Local Group".into() };
                rows.push(("distance".into(), format!("{:.0} kpc", g.dist_kpc)));
                rows.push(("size".into(), format!("~{:.1} kpc", g.size_kpc)));
                provs.push(("position & distance: compiled".into(), Prov::Measured));
                provs.push(("glyph shape".into(), Prov::Illustrative));
            }
            ObjectRef::Node(ni) => {
                let n = &u.nodes[ni];
                subtitle = "galaxy cluster / attractor".into();
                rows.push(("distance".into(), format!("{:.0} Mpc", n.dist_mpc)));
                rows.push(("size".into(), format!("~{:.0} Mpc", n.size_mpc)));
                provs.push(("position & distance: compiled".into(), Prov::Measured));
                provs.push(("web filaments".into(), Prov::Procedural));
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
    #[cfg(windows)]
    ambience: Option<crate::audio::Ambience>,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let rs = cc.wgpu_render_state.as_ref().expect("wgpu backend required").clone();
        let gpu = GpuContext::from_eframe(&rs);
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let app = Pantheon::new(gpu);
        #[cfg(windows)]
        let ambience = crate::audio::Ambience::start(if app.ambience_on {
            app.ambience_vol
        } else {
            0.0
        });
        Self {
            app,
            rs,
            canvas: None,
            last: Instant::now(),
            #[cfg(windows)]
            ambience,
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dt = self.last.elapsed().as_secs_f64().min(0.1);
        self.last = Instant::now();
        #[cfg(windows)]
        if let Some(a) = &self.ambience {
            a.set_gain(if self.app.ambience_on { self.app.ambience_vol } else { 0.0 });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let (w, h) = (rect.width().max(64.0) as u32, rect.height().max(64.0) as u32);
                // (re)create canvas target on resize
                let recreate = self
                    .canvas
                    .as_ref()
                    .map(|(t, _)| t.width != w || t.height != h)
                    .unwrap_or(true);
                if recreate {
                    let target = ViewTarget::new(&self.app.gpu, w, h);
                    let mut rend = self.rs.renderer.write();
                    if let Some((_, old)) = self.canvas.take() {
                        rend.free_texture(&old);
                    }
                    let id = rend.register_native_texture(
                        &self.app.gpu.device,
                        &target.resolve_view,
                        wgpu::FilterMode::Linear,
                    );
                    self.canvas = Some((target, id));
                }
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
        ctx.request_repaint();
    }
}

pub fn run_gui() -> anyhow::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1680.0, 980.0])
        .with_title("Pantheon — An Infinite Vector Atlas of the Living Universe");
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
        Ok(Self { app: Pantheon::new(gpu), ctx, rend, canvas: None, final_target: None })
    }

    /// Advance dt and render one full frame (canvas + UI) at (w, h).
    /// `read = false` skips the GPU readback (settle frames).
    pub fn frame(&mut self, w: u32, h: u32, dt: f64, read: bool) -> anyhow::Result<Vec<u8>> {
        self.app.tick(dt);
        let recreate = self
            .canvas
            .as_ref()
            .map(|(t, _)| t.width != w || t.height != h)
            .unwrap_or(true);
        if recreate {
            let target = ViewTarget::new(&self.app.gpu, w, h);
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
            let mut pass = final_target
                .begin(&mut encoder, Some(wgpu::Color::BLACK))
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
        // two settle frames (font atlas, band fades) without readback
        for _ in 0..2 {
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
        desc: "Earth close-up: vector coastlines, terminator, clouds",
        setup: |a| {
            let p = sim::planet_pos(2, a.clock.jd);
            view(a, p, 3.4e7, Some(ObjectRef::Planet(2)));
            a.cam.tilt = 0.35;
        },
    },
    Shot {
        name: "earth_moon",
        desc: "Earth–Moon system with measured lunar arc",
        setup: |a| {
            let p = sim::planet_pos(2, a.clock.jd);
            view(a, p, 9.5e8, Some(ObjectRef::Planet(2)));
        },
    },
    Shot {
        name: "gravity_moon",
        desc: "Gravity view: Earth–Moon Lagrange points and Hill sphere",
        setup: |a| {
            let p = sim::planet_pos(2, a.clock.jd);
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
            a.cam.tilt = 0.3;
        },
    },
    Shot {
        name: "luna",
        desc: "The Moon close-up: maria and craters",
        setup: |a| {
            let jd = a.clock.jd;
            let p = sim::body_pos(2, jd) + sim::luna_rel(jd);
            view(a, p, 1.0e7, Some(ObjectRef::Moon(0)));
        },
    },
    Shot {
        name: "galaxy_close",
        desc: "Galaxy at 3 kpc: fog resolving into stellar sparkle",
        setup: |a| {
            let gc = crate::frames::galactic_frame().center;
            view(a, gc * 0.5, 2.0e20, None);
        },
    },
    Shot {
        name: "jupiter",
        desc: "Jupiter and the Galilean moons",
        setup: |a| {
            let p = sim::planet_pos(4, a.clock.jd);
            view(a, p, 8.5e9, Some(ObjectRef::Planet(4)));
        },
    },
    Shot {
        name: "saturn",
        desc: "Saturn: engraved rings and moon system",
        setup: |a| {
            let p = sim::planet_pos(5, a.clock.jd);
            view(a, p, 3.2e9, Some(ObjectRef::Planet(5)));
            a.cam.tilt = 0.5;
        },
    },
    Shot {
        name: "stars",
        desc: "Gaia stellar neighbourhood (~20 pc)",
        setup: |a| view(a, DVec3::ZERO, 6.2e17, None),
    },
    Shot {
        name: "starfield",
        desc: "Deep star field with exoplanet portals",
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
    let mut done = 0;
    let mut failures: Vec<String> = Vec::new();
    for shot in SHOTS {
        if which != "all" && which != shot.name {
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
