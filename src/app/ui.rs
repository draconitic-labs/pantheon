//! Everything drawn over the canvas: where-am-I readout, toolbar and search, the
//! scale rail, selection card, time bar, view menu, help, first-run hint.
//!
//! The layout is a pure function of the window size, so panels can be tested not
//! to collide, and each panel simply fills the rectangle it was given.

use egui::{Align2, Color32, Rect, RichText, Sense, Stroke};
use glam::DVec3;

use super::Pantheon;
use crate::atlas::{self, obj_name, obj_pos, ObjectRef};
use crate::data::Prov;
use crate::frames::{fmt_len, AU, LY};
use crate::sim::{fmt_jd, jd_now, J2000, PLANETS};
use crate::theme;

// --------------------------------------------------------------- layout

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub toolbar: Rect,
    pub search: Rect,
    pub title: Rect,
    pub rail: Option<Rect>,
    pub rail_labels: bool,
    pub bar: Rect,
    pub bar_level: u8,
    pub left_col: Rect,
    pub right_col: Rect,
    pub show_caption: bool,
}

const TOOL: f32 = 34.0;
const TOOL_GAP: f32 = 6.0;
const CARD_W: f32 = 300.0;
const RIGHT_W: f32 = 244.0;

pub fn layout(w: f32, h: f32) -> Layout {
    let m = if w < 700.0 { 10.0 } else { 14.0 };
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    };

    // toolbar, top right: search + back, home, view, help
    let search_w = if w >= 1040.0 {
        290.0
    } else if w >= 780.0 {
        200.0
    } else {
        TOOL
    };
    let tools_w = 4.0 * TOOL + 3.0 * TOOL_GAP;
    let toolbar_x0 = w - m - tools_w - TOOL_GAP - search_w;
    let toolbar = r(toolbar_x0, m, w - m, m + TOOL);
    let search = r(toolbar_x0, m, toolbar_x0 + search_w, m + TOOL);

    // title block, top left
    let tall = h >= 520.0;
    let title_h = if tall { 136.0 } else { 62.0 };
    let title = r(m, m, (toolbar_x0 - 14.0).max(m + 60.0), m + title_h);

    // scale rail, right edge, under the toolbar
    let rail_labels = w >= 760.0 && h >= 420.0;
    let rail_w = if rail_labels { 158.0 } else { 26.0 };
    let rail_top = toolbar.bottom() + 14.0;
    let rail = (h - m - rail_top >= 180.0).then(|| r(w - m - rail_w, rail_top, w - m, h - m));
    let content_right = rail.map(|x| x.left() - 10.0).unwrap_or(w - m);

    // time bar, bottom: wide, compact, or just the essentials
    let bar_level = if w >= 1160.0 {
        2
    } else if w >= 780.0 {
        1
    } else {
        0
    };
    let bar_w = match bar_level {
        2 => 650.0,
        1 => 430.0,
        _ => 214.0f32.min(content_right - m),
    };
    let bar_x = ((w - bar_w) * 0.5).clamp(m, (content_right - bar_w).max(m));
    let bar = r(bar_x, h - m - 40.0, bar_x + bar_w, h - m);

    // the card column grows upward from the bottom left, above the bar if needed
    let card_w = CARD_W.min(content_right - m);
    let mut left_col = r(m, title.bottom() + 8.0, m + card_w, h - m);
    if left_col.right() > bar.left() - 8.0 {
        left_col.max.y = bar.top() - 8.0;
    }
    let right_w = RIGHT_W.min(content_right - m);
    let mut right_col = r(
        content_right - right_w,
        toolbar.bottom() + 10.0,
        content_right,
        bar.top() - 8.0,
    );
    if right_col.left() < title.right() + 8.0 {
        right_col.min.y = title.bottom() + 8.0;
    }
    if right_col.left() < left_col.right() + 8.0 {
        // too narrow for two columns: share the height
        let split = (left_col.top().max(right_col.top()) + left_col.bottom()) * 0.5;
        right_col.max.y = split - 4.0;
        left_col.min.y = split + 4.0;
    }
    Layout {
        toolbar,
        search,
        title,
        rail,
        rail_labels,
        bar,
        bar_level,
        left_col,
        right_col,
        show_caption: tall,
    }
}

// -------------------------------------------------------------- content

pub const RUNGS: [(f64, &str); 11] = [
    (7.45, "Planet"),
    (9.4, "Planet and moons"),
    (11.5, "Inner planets"),
    (13.1, "Outer planets"),
    (14.4, "Heliosphere"),
    (16.0, "Oort cloud"),
    (17.5, "Nearest stars"),
    (19.0, "Stellar neighbourhood"),
    (21.2, "Milky Way"),
    (23.0, "Local Group"),
    (24.9, "Cosmic web"),
];
const RAIL_LO: f64 = 6.7;
const RAIL_HI: f64 = 25.7;

pub const RATES: [(&str, f64); 8] = [
    ("real time", 1.0),
    ("1 minute / s", 60.0),
    ("1 hour / s", 3_600.0),
    ("1 day / s", 86_400.0),
    ("1 month / s", 2.63e6),
    ("1 year / s", 3.156e7),
    ("1,000 years / s", 3.156e10),
    ("1 million years / s", 3.156e13),
];

/// A distance in words people use.
pub fn fmt_words(m: f64) -> String {
    let km = m / 1e3;
    if km < 1.0 {
        format!("{m:.0} m")
    } else if km < 1e6 {
        format!("{} km", group_digits(sig2(km)))
    } else if m < 0.05 * AU {
        format!("{} million km", trim_num(sig2(km / 1e6)))
    } else if m < 0.05 * LY {
        let au = m / AU;
        format!("{} AU", group_digits(sig2(au)))
    } else if m < 1e3 * LY {
        format!("{} light-years", trim_num(sig2(m / LY)))
    } else if m < 1e6 * LY {
        format!("{} thousand light-years", trim_num(sig2(m / LY / 1e3)))
    } else if m < 1e9 * LY {
        format!("{} million light-years", trim_num(sig2(m / LY / 1e6)))
    } else {
        format!("{} billion light-years", trim_num(sig2(m / LY / 1e9)))
    }
}

fn sig2(v: f64) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let mag = 10f64.powf(v.abs().log10().floor() - 1.0);
    (v / mag).round() * mag
}

/// Two significant figures, without trailing zeros.
fn trim_num(v: f64) -> String {
    if v <= 0.0 {
        return "0".into();
    }
    let decimals = (1.0 - v.log10().floor()).clamp(0.0, 6.0) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn group_digits(v: f64) -> String {
    if v < 10.0 {
        return trim_num(v);
    }
    let raw = format!("{:.0}", v);
    let mut out = String::new();
    for (i, c) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// How long light takes to cross a distance, in words.
pub fn fmt_light_time(m: f64) -> String {
    const C: f64 = 299_792_458.0;
    const YEAR: f64 = 365.25 * 86_400.0;
    let s = m / C;
    let n = |v: f64, unit: &str| -> String {
        let t = trim_num(sig2(v));
        let singular = t == "1";
        format!("{t} {unit}{}", if singular { "" } else { "s" })
    };
    if s < 1.0 {
        n(s * 1e3, "millisecond")
    } else if s < 120.0 {
        n(s, "second")
    } else if s < 7200.0 {
        n(s / 60.0, "minute")
    } else if s < 172_800.0 {
        n(s / 3600.0, "hour")
    } else if s < YEAR * 1.5 {
        n(s / 86_400.0, "day")
    } else if s < YEAR * 1e3 {
        n(s / YEAR, "year")
    } else if s < YEAR * 1e6 {
        format!("{} thousand years", trim_num(sig2(s / YEAR / 1e3)))
    } else if s < YEAR * 1e9 {
        format!("{} million years", trim_num(sig2(s / YEAR / 1e6)))
    } else {
        format!("{} billion years", trim_num(sig2(s / YEAR / 1e9)))
    }
}

/// The largest round length (1-2-5) that fits in `max_px`, and its width in px.
pub fn scale_bar(mpp: f64, max_px: f64) -> (f64, f64) {
    let max_m = mpp * max_px;
    // round in the unit a person would read it in
    let unit = if max_m < 3.0e9 {
        1e3
    } else if max_m < 0.3 * LY {
        AU
    } else if max_m < 1e3 * LY {
        LY
    } else if max_m < 1e6 * LY {
        1e3 * LY
    } else {
        1e6 * LY
    };
    let v = max_m / unit;
    let base = 10f64.powf(v.log10().floor());
    let nice = [5.0, 2.0, 1.0]
        .iter()
        .map(|k| k * base)
        .find(|x| *x <= v)
        .unwrap_or(base);
    let len = nice * unit;
    (len, len / mpp)
}

fn fmt_bar(len: f64) -> String {
    if len < 3.0e9 {
        let km = len / 1e3;
        if km >= 1e6 {
            format!("{} million km", trim_num(km / 1e6))
        } else {
            format!("{} km", group_digits(km))
        }
    } else if len < 0.3 * LY {
        format!("{} AU", group_digits(len / AU))
    } else if len < 1e3 * LY {
        format!("{} light-years", trim_num(len / LY))
    } else if len < 1e6 * LY {
        format!("{} thousand light-years", trim_num(len / (1e3 * LY)))
    } else {
        format!("{} million light-years", trim_num(len / (1e6 * LY)))
    }
}

/// Text on open canvas: a soft dark halo keeps it readable over anything.
pub fn halo_text(
    p: &egui::Painter,
    pos: egui::Pos2,
    align: Align2,
    text: &str,
    font: egui::FontId,
    col: Color32,
    alpha: f32,
) {
    let halo = Color32::from_black_alpha((190.0 * alpha.clamp(0.0, 1.0)) as u8);
    for (dx, dy) in [
        (-1.0, 0.0),
        (1.0, 0.0),
        (0.0, -1.0),
        (0.0, 1.0),
        (0.8, 0.8),
        (-0.8, 0.8),
    ] {
        p.text(pos + egui::vec2(dx, dy), align, text, font.clone(), halo);
    }
    p.text(pos, align, text, font, col);
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Search,
    Home,
    Back,
    Layers,
    Help,
    Play,
    Pause,
    Close,
    Minus,
    Plus,
}

fn paint_icon(p: &egui::Painter, c: egui::Pos2, icon: Icon, col: Color32, s: f32) {
    let st = Stroke::new(1.5, col);
    let pt = |x: f32, y: f32| egui::pos2(c.x + x * s, c.y + y * s);
    match icon {
        Icon::Search => {
            p.circle_stroke(pt(-0.14, -0.14), 0.42 * s, st);
            p.line_segment([pt(0.18, 0.18), pt(0.55, 0.55)], st);
        }
        Icon::Home => {
            p.add(egui::Shape::line(
                vec![pt(-0.62, 0.0), pt(0.0, -0.58), pt(0.62, 0.0)],
                st,
            ));
            p.add(egui::Shape::line(
                vec![
                    pt(-0.42, -0.12),
                    pt(-0.42, 0.56),
                    pt(0.42, 0.56),
                    pt(0.42, -0.12),
                ],
                st,
            ));
        }
        Icon::Back => {
            p.line_segment([pt(-0.6, 0.0), pt(0.6, 0.0)], st);
            p.add(egui::Shape::line(
                vec![pt(-0.1, -0.5), pt(-0.6, 0.0), pt(-0.1, 0.5)],
                st,
            ));
        }
        Icon::Layers => {
            for dy in [-0.3f32, 0.0, 0.3] {
                p.add(egui::Shape::closed_line(
                    vec![
                        pt(0.0, dy - 0.22),
                        pt(0.6, dy),
                        pt(0.0, dy + 0.22),
                        pt(-0.6, dy),
                    ],
                    st,
                ));
            }
        }
        Icon::Help => {
            p.text(c, Align2::CENTER_CENTER, "?", theme::ui(s * 1.45), col);
        }
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![pt(-0.38, -0.58), pt(0.58, 0.0), pt(-0.38, 0.58)],
                col,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            let w = 0.34 * s;
            for x in [-0.34f32, 0.34] {
                p.rect_filled(
                    Rect::from_center_size(pt(x, 0.0), egui::vec2(w, 1.2 * s)),
                    1.0,
                    col,
                );
            }
        }
        Icon::Close => {
            p.line_segment([pt(-0.46, -0.46), pt(0.46, 0.46)], st);
            p.line_segment([pt(-0.46, 0.46), pt(0.46, -0.46)], st);
        }
        Icon::Minus => {
            p.line_segment([pt(-0.5, 0.0), pt(0.5, 0.0)], st);
        }
        Icon::Plus => {
            p.line_segment([pt(-0.5, 0.0), pt(0.5, 0.0)], st);
            p.line_segment([pt(0.0, -0.5), pt(0.0, 0.5)], st);
        }
    }
}

fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    enabled: bool,
    active: bool,
    tip: &str,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    let hovered = enabled && resp.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            5.0,
            Stroke::new(1.5, theme::BRASS),
            egui::StrokeKind::Outside,
        );
    }
    let fill = if active {
        Color32::from_rgba_premultiplied(70, 58, 34, 240)
    } else if hovered {
        Color32::from_rgba_premultiplied(40, 46, 68, 240)
    } else {
        theme::PANEL
    };
    ui.painter().rect_filled(rect, 5.0, fill);
    ui.painter().rect_stroke(
        rect,
        5.0,
        Stroke::new(1.0, theme::PANEL_LINE),
        egui::StrokeKind::Inside,
    );
    let col = if !enabled {
        Color32::from_gray(90)
    } else if active || hovered {
        theme::BRASS
    } else {
        theme::VELLUM
    };
    paint_icon(ui.painter(), rect.center(), icon, col, size * 0.3);
    resp.on_hover_text(tip)
}

fn panel_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.style())
}

impl Pantheon {
    /// What to call the place the camera is, right now.
    fn place_title(&self) -> (String, f64) {
        let lw = self.cam.log_width(self.view_w);
        let band = self
            .fo
            .as_ref()
            .map(|f| f.band_name.clone())
            .unwrap_or_default();
        if self.scene_exo.is_some() {
            return (band, lw);
        }
        if lw < 10.4 {
            if let Some(obj) = self.near_subject() {
                return (obj_name(&self.u, obj), lw);
            }
        }
        // anything selected that the view is plainly about
        if let Some(obj) = self.focus {
            let view = self.cam.mpp * self.view_w;
            let d = (obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref())
                - self.cam.center_root())
            .length();
            if !matches!(
                obj,
                ObjectRef::Sun | ObjectRef::Planet(_) | ObjectRef::Moon(_)
            ) && d < 0.25 * view
                && view < 40.0 * atlas::obj_half_extent(&self.u, obj)
            {
                return (obj_name(&self.u, obj), lw);
            }
        }
        (band, lw)
    }

    /// The solid body the view is about, if the view is near one.
    fn near_subject(&self) -> Option<ObjectRef> {
        let c = self.cam.center_root();
        let view = self.cam.mpp * self.view_w;
        let jd = self.clock.jd;
        let isl = self.island.as_ref();
        if let Some(obj) = self.focus {
            if matches!(
                obj,
                ObjectRef::Sun | ObjectRef::Planet(_) | ObjectRef::Moon(_)
            ) && (obj_pos(&self.u, obj, jd, isl) - c).length() < view
            {
                return Some(obj);
            }
        }
        let mut best: Option<(f64, ObjectRef)> = None;
        let candidates = std::iter::once(ObjectRef::Sun)
            .chain((0..PLANETS.len()).map(ObjectRef::Planet))
            .chain((0..crate::sim::MOONS.len()).map(ObjectRef::Moon));
        for obj in candidates {
            let d = (obj_pos(&self.u, obj, jd, isl) - c).length();
            if d < view * 0.8 && best.map(|(b, _)| d < b).unwrap_or(true) {
                best = Some((d, obj));
            }
        }
        best.map(|(_, o)| o)
    }

    pub fn draw_panels(&mut self, ctx: &egui::Context, rect: Rect) {
        if self.cinema {
            return;
        }
        let l = layout(rect.width(), rect.height());
        self.panel_rects.clear();
        self.panel_rects.push(l.toolbar);
        self.panel_rects.push(Rect::from_min_size(
            l.title.min,
            egui::vec2(l.title.width().min(430.0), l.title.height()),
        ));
        self.panel_rects.push(l.bar);
        if let (Some(r), true) = (l.rail, self.modes.ladder) {
            if l.rail_labels {
                self.panel_rects.push(r);
            }
        }
        self.title_block(ctx, &l);
        self.edge_markers(ctx, &l, rect);
        self.toolbar(ctx, &l);
        if let Some(r) = l.rail {
            if self.modes.ladder {
                self.scale_rail(ctx, r, l.rail_labels);
            }
        }
        self.time_bar(ctx, &l);
        if self.focus.is_some() {
            self.object_card(ctx, &l);
        }
        if self.modes.truth || self.show_capture {
            self.right_column(ctx, &l);
        }
        self.hover_tip(ctx);
        if self.show_layers {
            self.layers_menu(ctx, &l);
        }
        if self.show_help {
            self.help_panel(ctx, rect);
        }
        if self.intro {
            self.intro_panel(ctx, &l);
        } else if let Some((msg, _)) = &self.status {
            let msg = msg.clone();
            egui::Area::new(egui::Id::new("status"))
                .order(egui::Order::Foreground)
                .interactable(false)
                .pivot(Align2::CENTER_BOTTOM)
                .fixed_pos(egui::pos2(l.bar.center().x, l.bar.top() - 10.0))
                .show(ctx, |ui| {
                    panel_frame(ctx).show(ui, |ui| {
                        ui.label(RichText::new(msg).color(theme::VELLUM).size(13.0));
                    });
                });
        }
        if self.search.open {
            self.search_palette(ctx, &l);
        }
    }

    // ---- where am I -------------------------------------------------------

    fn title_block(&self, ctx: &egui::Context, l: &Layout) {
        let Some(fo) = &self.fo else { return };
        let (title, lw) = self.place_title();
        let view_m = self.cam.mpp * self.view_w;
        let caption = fo.caption;
        let turning = if (18.3..19.3).contains(&lw) {
            Some("The map is turning to lie in the plane of the Galaxy")
        } else if (22.3..23.2).contains(&lw) {
            Some("The map is turning to lie in the plane of nearby superclusters")
        } else {
            None
        };
        let r = l.title;
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("title"),
        ));
        let x = r.left();
        let mut y = r.top();
        let title_size = if r.width() < 260.0 { 22.0 } else { 34.0 };
        halo_text(
            &painter,
            egui::pos2(x, y - 3.0),
            Align2::LEFT_TOP,
            &title,
            theme::heading(title_size),
            theme::VELLUM,
            1.0,
        );
        y += title_size + 3.0;
        // how big is this view, in words and in light
        let width_line = format!(
            "{} across, {} for light",
            fmt_words(view_m),
            fmt_light_time(view_m)
        );
        halo_text(
            &painter,
            egui::pos2(x, y),
            Align2::LEFT_TOP,
            &width_line,
            theme::ui(13.0),
            theme::TEXT,
            1.0,
        );
        y += 19.0;
        // a true scale bar
        let (len, px) = scale_bar(self.cam.mpp, (r.width() as f64 * 0.42).clamp(70.0, 180.0));
        let px = px as f32;
        let bar_y = y + 7.0;
        for (o, c) in [(1.0, Color32::from_black_alpha(190)), (0.0, theme::VELLUM)] {
            let st = Stroke::new(if o > 0.0 { 3.2 } else { 1.2 }, c);
            painter.line_segment([egui::pos2(x, bar_y), egui::pos2(x + px, bar_y)], st);
            painter.line_segment([egui::pos2(x, bar_y - 4.0), egui::pos2(x, bar_y + 4.0)], st);
            painter.line_segment(
                [
                    egui::pos2(x + px, bar_y - 4.0),
                    egui::pos2(x + px, bar_y + 4.0),
                ],
                st,
            );
        }
        halo_text(
            &painter,
            egui::pos2(x + px + 9.0, bar_y),
            Align2::LEFT_CENTER,
            &fmt_bar(len),
            theme::ui(12.0),
            theme::TEXT,
            1.0,
        );
        y += 21.0;
        if l.show_caption {
            // say what this view is made of: the project's labels, in plain words
            let (text, prov) = caption;
            let wrap = (r.width() - 14.0).max(120.0);
            let galley = ctx.fonts(|f| {
                f.layout(
                    text.to_string(),
                    theme::ui(11.5),
                    theme::TEXT_DIM,
                    wrap.min(420.0),
                )
            });
            painter.circle_filled(
                egui::pos2(x + 3.5, y + 7.0),
                3.5,
                theme::col32(prov.rgb(), 1.0),
            );
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let halo = ctx.fonts(|f| {
                    f.layout(
                        text.to_string(),
                        theme::ui(11.5),
                        Color32::from_black_alpha(190),
                        wrap.min(420.0),
                    )
                });
                painter.galley(
                    egui::pos2(x + 13.0 + dx, y + dy),
                    halo,
                    Color32::from_black_alpha(190),
                );
            }
            let h = galley.size().y;
            painter.galley(egui::pos2(x + 13.0, y), galley, theme::TEXT_DIM);
            y += h + 3.0;
            if let Some(t) = turning {
                halo_text(
                    &painter,
                    egui::pos2(x + 13.0, y),
                    Align2::LEFT_TOP,
                    t,
                    theme::ui(11.5),
                    theme::BRASS,
                    1.0,
                );
                y += 16.0;
            }
            // the image credits their licenses require, shown while they're on screen
            let on = |e: crate::evidence::Ev| fo.evidence.iter().any(|(a, _)| *a == e);
            let credit = match (
                on(crate::evidence::Ev::SkyPanorama),
                on(crate::evidence::Ev::SurfaceImagery),
            ) {
                (true, true) => Some("Sky: ESO/S. Brunier. Maps: Solar System Scope."),
                (true, false) => Some("Sky: ESO/S. Brunier."),
                (false, true) => Some("Maps: Solar System Scope."),
                _ => None,
            };
            if let Some(c) = credit {
                halo_text(
                    &painter,
                    egui::pos2(x + 13.0, y),
                    Align2::LEFT_TOP,
                    c,
                    theme::ui(10.5),
                    theme::TEXT_DIM,
                    0.9,
                );
            }
        }
    }

    /// Arrows at the edge of the view pointing to things worth finding again.
    fn edge_markers(&mut self, ctx: &egui::Context, l: &Layout, rect: Rect) {
        let lw = self.cam.log_width(self.view_w);
        let mut targets: Vec<(String, DVec3, Option<ObjectRef>)> = Vec::new();
        if let Some(obj) = self.focus {
            let p = obj_pos(&self.u, obj, self.clock.jd, self.island.as_ref());
            targets.push((obj_name(&self.u, obj), p, Some(obj)));
        }
        if self.scene_exo.is_none() && lw > 9.8 && lw < 23.8 && self.focus != Some(ObjectRef::Sun) {
            targets.push(("Sun".into(), DVec3::ZERO, None));
        }
        let proj = crate::frames::Proj::new(
            &self.cam,
            rect.width() as f64,
            rect.height() as f64,
            &self.u.gal,
        );
        let inset = Rect::from_min_max(
            egui::pos2(l.left_col.left() + 6.0, l.toolbar.bottom() + 12.0),
            egui::pos2(
                l.rail
                    .map(|r| r.left() - 16.0)
                    .unwrap_or(rect.right() - 16.0),
                l.bar.top() - 14.0,
            ),
        );
        let (cx, cy) = (rect.center().x as f64, rect.center().y as f64);
        let mut go: Option<(Option<ObjectRef>, DVec3)> = None;
        for (i, (name, p, obj)) in targets.iter().enumerate() {
            let s = proj.screen(*p);
            let (sx, sy) = (s[0] + rect.left() as f64, s[1] + rect.top() as f64);
            let margin = 2.0;
            if sx > rect.left() as f64 - margin
                && sx < rect.right() as f64 + margin
                && sy > rect.top() as f64 - margin
                && sy < rect.bottom() as f64 + margin
            {
                continue;
            }
            // where the line from the middle of the view leaves the free area
            let (dx, dy) = (sx - cx, sy - cy);
            let len = dx.hypot(dy).max(1.0);
            let (ux, uy) = (dx / len, dy / len);
            let tx = if ux > 0.0 {
                (inset.right() as f64 - cx) / ux
            } else if ux < 0.0 {
                (inset.left() as f64 - cx) / ux
            } else {
                f64::MAX
            };
            let ty = if uy > 0.0 {
                (inset.bottom() as f64 - cy) / uy
            } else if uy < 0.0 {
                (inset.top() as f64 - cy) / uy
            } else {
                f64::MAX
            };
            let t = tx.min(ty);
            let mut tip = egui::pos2((cx + ux * t) as f32, (cy + uy * t) as f32);
            // never on top of the card: slide along the edge of it
            if let Some(card) = self
                .panel_rects
                .iter()
                .find(|r| r.left() <= l.left_col.left() + 1.0 && r.top() > l.title.bottom())
            {
                if card.expand(16.0).contains(tip) {
                    tip.x = card.right() + 22.0;
                }
            }
            let dist = (*p - self.cam.center_root()).length();
            let text = format!("{name}, {}", fmt_len(dist));
            let d = egui::vec2(ux as f32, uy as f32);
            let n = egui::vec2(-d.y, d.x);
            let col = if obj.is_some() {
                theme::BRASS
            } else {
                theme::VELLUM
            };
            let id = egui::Id::new(("edge", i));
            // label sits inward of the arrowhead
            let label_anchor = if ux > 0.3 {
                Align2::RIGHT_CENTER
            } else if ux < -0.3 {
                Align2::LEFT_CENTER
            } else if uy > 0.0 {
                Align2::CENTER_BOTTOM
            } else {
                Align2::CENTER_TOP
            };
            let label_pos = tip - d * 16.0;
            let galley_w = text.chars().count() as f32 * 6.4 + 14.0;
            let hit = label_anchor
                .anchor_size(label_pos, egui::vec2(galley_w, 20.0))
                .union(Rect::from_center_size(tip, egui::vec2(18.0, 18.0)));
            let resp = egui::Area::new(id)
                .order(egui::Order::Middle)
                .fixed_pos(hit.min)
                .show(ctx, |ui| ui.allocate_response(hit.size(), Sense::click()))
                .inner;
            let hovered = resp.hovered();
            if hovered {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let paint = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, id));
            let shade = if hovered { theme::VELLUM } else { col };
            paint.add(egui::Shape::convex_polygon(
                vec![
                    tip + d * 7.0,
                    tip - d * 4.0 + n * 5.5,
                    tip - d * 4.0 - n * 5.5,
                ],
                shade,
                Stroke::new(1.0, Color32::from_black_alpha(170)),
            ));
            halo_text(
                &paint,
                label_pos,
                label_anchor,
                &text,
                theme::ui(12.0),
                shade,
                1.0,
            );
            if resp.on_hover_text("Go there").clicked() {
                go = Some((*obj, *p));
            }
        }
        if let Some((obj, p)) = go {
            match obj {
                Some(o) => self.dive_to(o),
                None => {
                    let w = self.cam.mpp * self.view_w;
                    self.fly_to_place(p, w, "the Sun");
                }
            }
        }
    }

    // ---- toolbar and search ---------------------------------------------------

    fn toolbar(&mut self, ctx: &egui::Context, l: &Layout) {
        let compact_search = l.search.width() < 60.0;
        if !self.search.open {
            egui::Area::new(egui::Id::new("search-pill"))
                .fixed_pos(l.search.min)
                .show(ctx, |ui| {
                    if compact_search {
                        if icon_button(ui, Icon::Search, TOOL, true, false, "Search (/)").clicked()
                        {
                            self.search.open_palette();
                        }
                    } else {
                        let (r, resp) = ui.allocate_exact_size(l.search.size(), Sense::click());
                        let hov = resp.hovered();
                        if hov {
                            ctx.set_cursor_icon(egui::CursorIcon::Text);
                        }
                        let round = r.height() * 0.5;
                        ui.painter().rect_filled(r, round, theme::PANEL);
                        ui.painter().rect_stroke(
                            r,
                            round,
                            Stroke::new(
                                1.0,
                                if hov {
                                    theme::BRASS_DIM
                                } else {
                                    theme::PANEL_LINE
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        paint_icon(
                            ui.painter(),
                            r.left_center() + egui::vec2(17.0, 0.0),
                            Icon::Search,
                            theme::TEXT_DIM,
                            10.0,
                        );
                        ui.painter().text(
                            r.left_center() + egui::vec2(31.0, 0.0),
                            Align2::LEFT_CENTER,
                            "Search the universe",
                            theme::ui(13.0),
                            theme::TEXT_DIM,
                        );
                        ui.painter().text(
                            r.right_center() - egui::vec2(15.0, 0.0),
                            Align2::RIGHT_CENTER,
                            "/",
                            theme::ui(13.0),
                            theme::BRASS_DIM,
                        );
                        if resp.clicked() {
                            self.search.open_palette();
                        }
                    }
                });
        }
        let mut back = false;
        let mut home = false;
        egui::Area::new(egui::Id::new("toolbar"))
            .fixed_pos(egui::pos2(l.search.right() + TOOL_GAP, l.toolbar.top()))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TOOL_GAP;
                    let can = self.can_go_back();
                    back = icon_button(
                        ui,
                        Icon::Back,
                        TOOL,
                        can,
                        false,
                        "Back to the previous view (Backspace)",
                    )
                    .clicked();
                    home = icon_button(ui, Icon::Home, TOOL, true, false, "Fly home to Earth (H)")
                        .clicked();
                    if icon_button(
                        ui,
                        Icon::Layers,
                        TOOL,
                        true,
                        self.show_layers,
                        "View options",
                    )
                    .clicked()
                    {
                        self.show_layers = !self.show_layers;
                    }
                    if icon_button(
                        ui,
                        Icon::Help,
                        TOOL,
                        true,
                        self.show_help,
                        "How to move (?)",
                    )
                    .clicked()
                    {
                        self.show_help = !self.show_help;
                    }
                });
            });
        if back {
            self.go_back();
        }
        if home {
            self.go_home();
        }
    }

    fn search_palette(&mut self, ctx: &egui::Context, l: &Layout) {
        let (enter, up, down, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if esc {
            self.search.close();
            return;
        }
        let width = 380.0f32.min(ctx.screen_rect().width() - 20.0);
        let x = (l.search.left())
            .min(ctx.screen_rect().right() - width - 10.0)
            .max(10.0);
        let mut picked: Option<super::Dest> = None;
        let mut close = false;
        let n = self.search.results().len();
        if n > 0 {
            if down {
                self.search.selected = (self.search.selected + 1) % n;
            }
            if up {
                self.search.selected = (self.search.selected + n - 1) % n;
            }
        }
        let mut selected = self.search.selected.min(n.saturating_sub(1));
        let mut query = std::mem::take(&mut self.search.query);
        let results: Vec<(String, String, super::Dest)> = {
            self.search.query.clone_from(&query);
            self.search
                .results()
                .iter()
                .map(|e| (e.name.clone(), e.detail.clone(), e.dest))
                .collect()
        };
        if enter {
            picked = results.get(selected).map(|r| r.2);
        }
        egui::Area::new(egui::Id::new("search"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(x, l.search.top()))
            .show(ctx, |ui| {
                panel_frame(ctx).inner_margin(8.0).show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                        paint_icon(ui.painter(), r.center(), Icon::Search, theme::BRASS, 10.0);
                        let te = ui.add(
                            egui::TextEdit::singleline(&mut query)
                                .hint_text("Planets, moons, stars, galaxies, places")
                                .frame(false)
                                .font(theme::ui(14.0))
                                .desired_width(ui.available_width() - 30.0),
                        );
                        te.request_focus();
                        if icon_button(ui, Icon::Close, 22.0, true, false, "Close (Esc)").clicked()
                        {
                            close = true;
                        }
                    });
                    ui.add_space(2.0);
                    if results.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "Nothing named \"{}\" in this atlas",
                                query.trim()
                            ))
                            .color(theme::TEXT_DIM)
                            .size(12.5),
                        );
                    } else if query.trim().is_empty() {
                        ui.label(
                            RichText::new("Try one of these")
                                .color(theme::TEXT_DIM)
                                .size(11.5),
                        );
                    }
                    for (i, (name, detail, dest)) in results.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 27.0),
                            Sense::click(),
                        );
                        if resp.hovered() {
                            selected = i;
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if i == selected {
                            ui.painter().rect_filled(
                                r,
                                4.0,
                                Color32::from_rgba_premultiplied(66, 56, 34, 225),
                            );
                        }
                        ui.painter().text(
                            r.left_center() + egui::vec2(7.0, 0.0),
                            Align2::LEFT_CENTER,
                            name,
                            theme::ui(13.5),
                            if i == selected {
                                theme::VELLUM
                            } else {
                                theme::TEXT
                            },
                        );
                        ui.painter().text(
                            r.right_center() - egui::vec2(7.0, 0.0),
                            Align2::RIGHT_CENTER,
                            detail,
                            theme::ui(11.0),
                            theme::TEXT_DIM,
                        );
                        if resp.clicked() {
                            picked = Some(*dest);
                        }
                    }
                });
            });
        self.search.query = query;
        self.search.selected = selected;
        if close {
            self.search.close();
        }
        if let Some(dest) = picked {
            self.search.close();
            self.go_to(dest);
        }
    }

    // ---- the scale rail -----------------------------------------------------------

    fn rail_go(&mut self, lw: f64) {
        let jd = self.clock.jd;
        // rungs carry destinations, not just a scale (AUDIT FND-021)
        if lw < 10.4 {
            let subject = self.near_subject().unwrap_or(ObjectRef::Planet(2));
            let parent = match subject {
                ObjectRef::Moon(mi) => ObjectRef::Planet(crate::sim::MOONS[mi].planet),
                o => o,
            };
            if lw < 8.3 {
                self.dive_to(subject);
                return;
            }
            if let ObjectRef::Planet(pi) = parent {
                if atlas::moon_system_extent(pi).is_some() {
                    self.show_moons(pi);
                    return;
                }
            }
            let p = obj_pos(&self.u, subject, jd, self.island.as_ref());
            self.fly_to_place(p, 10f64.powf(lw), "");
            return;
        }
        let center = if (20.0..22.4).contains(&lw) {
            self.u.gal.center
        } else {
            DVec3::ZERO
        };
        self.fly_to_place(center, 10f64.powf(lw), "");
    }

    fn scale_rail(&mut self, ctx: &egui::Context, r: Rect, labels: bool) {
        let lw_now = self.cam.log_width(self.view_w);
        let mut go: Option<f64> = None;
        let mut scrub: Option<f64> = None;
        egui::Area::new(egui::Id::new("rail"))
            .fixed_pos(r.min)
            .show(ctx, |ui| {
                let (resp, p) = ui.allocate_painter(r.size(), Sense::click_and_drag());
                let r = resp.rect;
                let rule_x = r.right() - 5.0;
                let (top, bot) = (r.top() + 8.0, r.bottom() - 8.0);
                let y_of =
                    |v: f64| bot - ((v - RAIL_LO) / (RAIL_HI - RAIL_LO)) as f32 * (bot - top);
                let v_of =
                    |y: f32| RAIL_LO + ((bot - y) / (bot - top)) as f64 * (RAIL_HI - RAIL_LO);
                // the rule, with a tick per decade: this is a logarithmic instrument
                p.line_segment(
                    [egui::pos2(rule_x, top), egui::pos2(rule_x, bot)],
                    Stroke::new(1.0, theme::BRASS_DIM),
                );
                let mut d = RAIL_LO.ceil();
                while d <= RAIL_HI {
                    let y = y_of(d);
                    p.line_segment(
                        [egui::pos2(rule_x - 4.0, y), egui::pos2(rule_x, y)],
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(170, 145, 95, 110)),
                    );
                    d += 1.0;
                }
                let hover_y = resp.hover_pos().map(|h| h.y);
                let dragging = resp.dragged();
                let min_gap = 15.0;
                let mut last_y = f32::MAX;
                let nearest = RUNGS
                    .iter()
                    .min_by(|a, b| {
                        (a.0 - lw_now)
                            .abs()
                            .partial_cmp(&(b.0 - lw_now).abs())
                            .unwrap()
                    })
                    .map(|x| x.1);
                for (v, name) in RUNGS {
                    let y = y_of(v);
                    if last_y - y < min_gap {
                        continue;
                    }
                    last_y = y;
                    let here = nearest == Some(name);
                    let hov = hover_y.map(|h| (h - y).abs() < 9.0).unwrap_or(false);
                    let col = if here {
                        theme::BRASS
                    } else if hov {
                        theme::VELLUM
                    } else {
                        theme::TEXT
                    };
                    p.line_segment(
                        [egui::pos2(rule_x - 9.0, y), egui::pos2(rule_x, y)],
                        Stroke::new(1.2, if here { theme::BRASS } else { theme::BRASS_DIM }),
                    );
                    if labels {
                        halo_text(
                            &p,
                            egui::pos2(rule_x - 15.0, y),
                            Align2::RIGHT_CENTER,
                            name,
                            theme::ui(if here { 13.0 } else { 12.0 }),
                            col,
                            1.0,
                        );
                    }
                }
                // the needle: where the camera is on the ladder of scales
                let ny = y_of(lw_now.clamp(RAIL_LO, RAIL_HI));
                let tip = egui::pos2(rule_x - 1.0, ny);
                p.add(egui::Shape::convex_polygon(
                    vec![
                        tip,
                        tip + egui::vec2(-10.0, -5.5),
                        tip + egui::vec2(-10.0, 5.5),
                    ],
                    theme::BRASS,
                    Stroke::new(1.0, Color32::from_black_alpha(190)),
                ));
                if resp.hovered() || dragging {
                    ctx.set_cursor_icon(if dragging {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::PointingHand
                    });
                }
                if dragging {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        scrub = Some(v_of(pos.y).clamp(RAIL_LO, RAIL_HI));
                    }
                } else if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let click = v_of(pos.y);
                        // a click near a rung means that rung; between them, that scale
                        let rung = RUNGS
                            .iter()
                            .find(|(v, _)| (y_of(*v) - pos.y).abs() < 9.0)
                            .map(|x| x.0);
                        go = Some(rung.unwrap_or(click));
                    }
                }
                let hint = hover_y.map(|y| fmt_words(10f64.powf(v_of(y).clamp(RAIL_LO, RAIL_HI))));
                if let Some(h) = hint {
                    resp.on_hover_text(format!("{h} across. Click to go, drag to scrub."));
                }
            });
        self.scrub_held = scrub.is_some();
        if let Some(v) = scrub {
            let subject = self
                .scrub
                .map(|(_, o)| o)
                .or_else(|| self.near_subject())
                .unwrap_or(ObjectRef::Planet(2));
            if self.scrub.is_none() {
                self.push_history();
                self.stop_motion();
                self.scene_exo = None;
            }
            self.scrub = Some((v, subject));
            self.dismiss_intro();
        }
        if let Some(v) = go {
            self.rail_go(v);
            self.dismiss_intro();
        }
    }

    // ---- time ---------------------------------------------------------------------

    fn time_bar(&mut self, ctx: &egui::Context, l: &Layout) {
        egui::Area::new(egui::Id::new("timebar"))
            .fixed_pos(l.bar.min)
            .show(ctx, |ui| {
                let frame = panel_frame(ctx).inner_margin(egui::Margin::symmetric(8, 5));
                frame.show(ui, |ui| {
                    ui.set_width(l.bar.width() - 16.0);
                    ui.set_height(l.bar.height() - 12.0);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let playing = self.clock.playing;
                        let (icon, tip) = if playing {
                            (Icon::Pause, "Pause time (Space)")
                        } else {
                            (Icon::Play, "Run time (Space)")
                        };
                        if icon_button(ui, icon, 28.0, true, playing, tip).clicked() {
                            self.clock.playing = !playing;
                            self.modes.living = self.clock.playing;
                        }
                        // the date, the thing you're controlling
                        let date = fmt_jd(self.clock.jd);
                        let date_short = if l.bar_level == 0 {
                            date[..11].to_string()
                        } else {
                            date
                        };
                        ui.label(RichText::new(date_short).size(13.0).color(theme::VELLUM));
                        if l.bar_level >= 1 {
                            ui.separator();
                            if icon_button(ui, Icon::Minus, 22.0, true, false, "Slower ([)")
                                .clicked()
                            {
                                self.step_rate(-1);
                            }
                            let rate = self.clock.rate;
                            let name = RATES
                                .iter()
                                .find(|(_, r)| (rate - r).abs() / r < 0.01)
                                .map(|x| x.0)
                                .unwrap_or("1 day / s");
                            ui.add_sized(
                                [if l.bar_level == 2 { 110.0 } else { 88.0 }, 22.0],
                                egui::Label::new(RichText::new(name).size(12.0).color(theme::TEXT))
                                    .truncate(),
                            );
                            if icon_button(ui, Icon::Plus, 22.0, true, false, "Faster (])")
                                .clicked()
                            {
                                self.step_rate(1);
                            }
                        }
                        if l.bar_level >= 2 {
                            ui.separator();
                            // writes back only on real interaction: slider rounding
                            // must never move time on its own
                            let mut yr = 2000.0 + (self.clock.jd - J2000) / 365.25;
                            let resp = ui.add(
                                egui::Slider::new(&mut yr, 1600.0..=2500.0)
                                    .show_value(false)
                                    .trailing_fill(true),
                            );
                            if resp.changed()
                                && (resp.dragged() || resp.clicked() || resp.has_focus())
                            {
                                self.clock.jd = J2000 + (yr - 2000.0) * 365.25;
                            }
                            resp.on_hover_text(format!("{yr:.1}: drag to travel through time"));
                        }
                        let now_gap = (self.clock.jd - jd_now()).abs() > 1.0 / 24.0;
                        ui.add_enabled_ui(now_gap || l.bar_level >= 1, |ui| {
                            if ui
                                .small_button("Now")
                                .on_hover_text("Return to the present (N)")
                                .clicked()
                            {
                                self.clock.jd = jd_now();
                            }
                        });
                    });
                });
            });
    }

    // ---- selection card ----------------------------------------------------------

    fn object_card(&mut self, ctx: &egui::Context, l: &Layout) {
        let Some(obj) = self.focus else { return };
        let (title, subtitle, rows, provs, can_awaken) = self.card_data(obj);
        let col = l.left_col;
        let max_h = (col.height() - 28.0).max(60.0);
        let moons_of = match obj {
            ObjectRef::Planet(i) if atlas::moon_system_extent(i).is_some() => Some(i),
            _ => None,
        };
        let mut dive = false;
        let mut moons = false;
        let mut close = false;
        let mut awaken = false;
        let expand_id = egui::Id::new("card-sources");
        let mut sources = ctx.data_mut(|d| *d.get_temp_mut_or::<bool>(expand_id, false));
        let card_rect = egui::Area::new(egui::Id::new("card"))
            .default_size(col.size())
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(col.left_bottom())
            .show(ctx, |ui| {
                panel_frame(ctx).show(ui, |ui| {
                    ui.set_width(col.width() - 24.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_max_width(col.width() - 58.0);
                            ui.label(RichText::new(&title).font(theme::heading(25.0)).color(theme::VELLUM));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            close = icon_button(ui, Icon::Close, 22.0, true, false, "Clear selection (Esc)").clicked();
                        });
                    });
                    if !subtitle.is_empty() {
                        ui.label(RichText::new(&subtitle).size(12.0).color(theme::TEXT_DIM));
                    }
                    let actions_h = 38.0;
                    let body_h = (max_h - 60.0 - actions_h).max(0.0);
                    if body_h > 30.0 {
                        ui.add_space(3.0);
                        egui::ScrollArea::vertical()
                            .max_height(body_h)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width() - 4.0);
                                for (k, v) in &rows {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(k).size(12.0).color(theme::TEXT_DIM));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(RichText::new(v).size(12.5).color(theme::TEXT));
                                        });
                                    });
                                }
                                ui.add_space(2.0);
                                let label = if sources { "Hide data sources" } else { "Where this comes from" };
                                if ui.link(RichText::new(label).size(11.5)).clicked() {
                                    sources = !sources;
                                }
                                if sources {
                                    for (what, e) in &provs {
                                        // PROV-003: chips resolve evidence records
                                        let r = crate::evidence::rec(*e);
                                        ui.horizontal(|ui| {
                                            let (rc, _) = ui.allocate_exact_size(egui::vec2(8.0, 12.0), Sense::hover());
                                            ui.painter().circle_filled(rc.center(), 3.4, theme::col32(r.class.rgb(), 1.0));
                                            let retrieved = r
                                                .dataset
                                                .and_then(|d| self.u.retrieved.get(d))
                                                .map_or(String::new(), |date| format!("\nretrieved: {date}"));
                                            ui.label(RichText::new(what).size(11.5).color(theme::TEXT))
                                                .on_hover_text(format!(
                                                    "{}\n{}\nframe: {}\n{}{}",
                                                    r.source, r.citation, r.frame, r.note, retrieved
                                                ));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(RichText::new(r.class.label()).size(11.0).color(theme::TEXT_DIM));
                                            });
                                        });
                                    }
                                }
                            });
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Fly here").on_hover_text("F").clicked() {
                            dive = true;
                        }
                        if moons_of.is_some() && ui.button("See moons").clicked() {
                            moons = true;
                        }
                        if can_awaken {
                            let lbl = if self.island.is_some() { "Stop simulation" } else { "Simulate" };
                            if ui
                                .button(lbl)
                                .on_hover_text("Run this planet and its moons as a live N-body gravity simulation")
                                .clicked()
                            {
                                awaken = true;
                            }
                        }
                    });
                    if self.island_paused {
                        ui.label(
                            RichText::new("Simulation paused: time is running too fast for it")
                                .size(11.0)
                                .color(theme::col32(Prov::Inferred.rgb(), 1.0)),
                        );
                    }
                });
            })
            .response
            .rect;
        ctx.data_mut(|d| d.insert_temp(expand_id, sources));
        self.panel_rects.push(card_rect);
        if close {
            self.focus = None;
            self.follow = None;
        }
        if awaken {
            if self.island.is_some() {
                self.island = None;
            } else {
                self.awaken();
            }
        }
        if dive {
            self.dive_to(obj);
        }
        if moons {
            if let Some(i) = moons_of {
                self.show_moons(i);
            }
        }
    }

    // ---- right column: legend and capture ---------------------------------------

    fn right_column(&mut self, ctx: &egui::Context, l: &Layout) {
        let col = l.right_col;
        egui::Area::new(egui::Id::new("right-col"))
            .default_size(col.size())
            .fixed_pos(col.min)
            .show(ctx, |ui| {
                ui.set_width(col.width());
                egui::ScrollArea::vertical()
                    .max_height(col.height())
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(col.width() - 6.0);
                        if self.modes.truth {
                            self.truth_legend(ctx, ui);
                        }
                        if self.show_capture {
                            ui.add_space(6.0);
                            self.capture_panel(ctx, ui);
                        }
                    });
            });
    }

    fn truth_legend(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        panel_frame(ctx).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Ink by evidence")
                    .font(theme::heading(19.0))
                    .color(theme::VELLUM),
            );
            // TRUTH-001: generated from what this frame actually drew; absent
            // classes never appear. Hover an entry for its record.
            let drawn: Vec<(crate::evidence::Ev, &'static str)> = self
                .fo
                .as_ref()
                .map(|f| f.evidence.clone())
                .unwrap_or_default();
            for p in [
                Prov::Measured,
                Prov::Modeled,
                Prov::Inferred,
                Prov::Procedural,
                Prov::Illustrative,
                Prov::Unknown,
            ] {
                let here: Vec<_> = drawn
                    .iter()
                    .filter(|(e, _)| crate::evidence::rec(*e).class == p)
                    .collect();
                if here.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    let (rc, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                    ui.painter()
                        .circle_filled(rc.center(), 4.0, theme::col32(p.rgb(), 1.0));
                    ui.label(RichText::new(p.label()).size(12.0));
                });
                for (e, what) in here {
                    let r = crate::evidence::rec(*e);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        let retrieved = r
                            .dataset
                            .and_then(|d| self.u.retrieved.get(d))
                            .map_or(String::new(), |date| format!("\nretrieved: {date}"));
                        ui.label(RichText::new(*what).size(11.0).color(theme::TEXT_DIM))
                            .on_hover_text(format!(
                                "{}\n{}\nframe: {}\n{}{}",
                                r.source, r.citation, r.frame, r.note, retrieved
                            ));
                    });
                }
            }
        });
    }

    fn capture_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        panel_frame(ctx).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Record a flight")
                    .font(theme::heading(19.0))
                    .color(theme::VELLUM),
            );
            ui.label(
                RichText::new(format!("{} keyframes", self.capture.len()))
                    .size(11.5)
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal(|ui| {
                if ui.button("Add view").clicked() {
                    let host = match self.focus {
                        Some(ObjectRef::ExoHost(i)) | Some(ObjectRef::ExoPlanet(i, _)) => {
                            Some(self.u.exos[i].host.clone())
                        }
                        _ => None,
                    };
                    let c = self.cam.center_root();
                    self.capture.push(super::PathKey {
                        center: [c.x, c.y, c.z],
                        mpp: self.cam.mpp,
                        tilt: self.cam.tilt,
                        yaw: self.cam.yaw,
                        jd: self.clock.jd,
                        dur: 4.0,
                        hold: 1.0,
                        at: None,
                        host,
                    });
                }
                if ui.button("Clear").clicked() {
                    self.capture.clear();
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Play").clicked() && !self.capture.is_empty() {
                    let keys = self.capture.clone();
                    self.apply_key(&keys[0]);
                    self.play = Some(super::PathPlay { keys, k: 0, t: 0.0 });
                }
                if ui.button("Save").clicked() {
                    let dir = std::path::PathBuf::from("exports/paths");
                    let stamp = fmt_jd(jd_now()).replace([' ', ':'], "-");
                    let path = dir.join(format!("path_{stamp}.json"));
                    match std::fs::create_dir_all(&dir)
                        .map_err(anyhow::Error::from)
                        .and_then(|_| Ok(serde_json::to_string_pretty(&self.capture)?))
                        .and_then(|js| Ok(std::fs::write(&path, js)?))
                    {
                        Ok(()) => self.note(format!("Saved {}", path.display())),
                        Err(e) => self.note(format!("Could not save the path: {e}")),
                    }
                }
            });
            ui.label(
                RichText::new("Render it with: pantheon --path exports/paths/<file>.json")
                    .size(10.5)
                    .color(theme::TEXT_DIM),
            );
        });
    }

    // ---- view menu -------------------------------------------------------------------

    fn layers_menu(&mut self, ctx: &egui::Context, l: &Layout) {
        let width = 268.0f32.min(ctx.screen_rect().width() - 20.0);
        let right = l.toolbar.right();
        let area = egui::Area::new(egui::Id::new("layers"))
            .default_size(egui::vec2(width, ctx.screen_rect().height()))
            .order(egui::Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(egui::pos2(right, l.toolbar.bottom() + 6.0))
            .show(ctx, |ui| {
                panel_frame(ctx).show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    let max_h = ctx.screen_rect().height() - l.toolbar.bottom() - 40.0;
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        ui.set_width(width - 34.0);
                        ui.label(RichText::new("On the map").size(11.5).color(theme::TEXT_DIM));
                        ui.checkbox(&mut self.modes.labels, "Names");
                        ui.checkbox(&mut self.modes.orbits, "Orbits");
                        if !self.u.craft.is_empty() {
                            ui.checkbox(&mut self.modes.trails, "Spacecraft paths");
                        }
                        ui.checkbox(&mut self.modes.ladder, "Scale rail");
                        ui.add_space(4.0);
                        ui.label(RichText::new("Modes").size(11.5).color(theme::TEXT_DIM));
                        ui.checkbox(&mut self.modes.truth, "Colour by evidence (T)")
                            .on_hover_text("Re-ink everything by how we know it: measured, modeled, inferred, procedural, illustrative");
                        let grav_ok = matches!(self.focus, Some(ObjectRef::Planet(_)) | Some(ObjectRef::Moon(_)));
                        ui.add_enabled(grav_ok, egui::Checkbox::new(&mut self.modes.gravity, "Gravity field (G)"))
                            .on_disabled_hover_text("Select a planet or moon first")
                            .on_hover_text("Lagrange points, Hill sphere, potential contours");
                        let sys_ok = self.scene_exo.is_some();
                        ui.add_enabled(sys_ok, egui::Checkbox::new(&mut self.modes.system_view, "System view (Y)"))
                            .on_disabled_hover_text("Fly to an exoplanet system first");
                        ui.checkbox(&mut self.show_capture, "Record a flight");
                        ui.add_space(4.0);
                        ui.label(RichText::new("Sound").size(11.5).color(theme::TEXT_DIM));
                        if cfg!(feature = "audio") && self.ambience_available != Some(false) {
                            let mut on = self.settings.ambience_on;
                            if ui.checkbox(&mut on, "Ambience (M)").changed() {
                                self.toggle_ambience();
                            }
                            if self.settings.ambience_on {
                                let r = ui.add(egui::Slider::new(&mut self.settings.ambience_vol, 0.0..=1.0).show_value(false));
                                if r.drag_stopped() {
                                    self.save_settings();
                                }
                            }
                        } else {
                            ui.label(RichText::new("No audio output available").size(12.0).color(theme::TEXT_DIM));
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new("Data on this computer").size(11.5).color(theme::TEXT_DIM));
                        if self.data_gaps.is_empty() {
                            ui.label(RichText::new("All datasets are present.").size(12.0));
                        } else {
                            for g in &self.data_gaps {
                                ui.label(RichText::new(g).size(12.0));
                            }
                            ui.label(
                                RichText::new("Run tools/fetch_data.py to download what's missing or damaged (about 3 GB with kernels).")
                                    .size(12.0)
                                    .color(theme::BRASS),
                            );
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new("Credits").size(11.5).color(theme::TEXT_DIM));
                        for line in [
                            "Stars: ESA/Gaia/DPAC, CC BY-NC 3.0 IGO",
                            "Planet maps: Solar System Scope, CC BY 4.0",
                            "Milky Way panorama: ESO/S. Brunier, CC BY 4.0",
                            "Positions: NASA/JPL ephemerides",
                            "Exoplanets: NASA Exoplanet Archive",
                            "Coastlines: Natural Earth",
                        ] {
                            ui.label(RichText::new(line).size(11.5));
                        }
                    });
                });
            });
        // a click anywhere else dismisses the menu
        if ctx.input(|i| i.pointer.any_pressed()) {
            let inside = ctx
                .input(|i| i.pointer.interact_pos())
                .map(|p| area.response.rect.contains(p) || l.toolbar.contains(p))
                .unwrap_or(true);
            if !inside {
                self.show_layers = false;
            }
        }
    }

    // ---- help + first run -----------------------------------------------------------

    fn help_panel(&mut self, ctx: &egui::Context, rect: Rect) {
        let mut close = false;
        let w = 440.0f32.min(rect.width() - 20.0);
        egui::Area::new(egui::Id::new("help"))
            .default_size(egui::vec2(w, rect.height()))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(rect.center())
            .show(ctx, |ui| {
                panel_frame(ctx).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("How to move")
                                .font(theme::heading(26.0))
                                .color(theme::VELLUM),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = icon_button(ui, Icon::Close, 24.0, true, false, "Close (Esc)")
                                .clicked();
                        });
                    });
                    let rows = [
                        ("Scroll, or pinch", "zoom at the pointer (Shift for fine)"),
                        ("Drag", "move; a flick keeps drifting"),
                        ("Right-drag, or Shift-drag", "tilt and turn the view"),
                        ("Click", "select; click a name too"),
                        ("Double-click", "fly there"),
                        ("/  or  Ctrl K", "search everything"),
                        ("H", "fly home to Earth"),
                        ("Backspace", "back to the previous view"),
                        ("+  -  and arrow keys", "zoom and move from the keyboard"),
                        ("F", "fly to the selection"),
                        ("0", "level the view"),
                        ("Space", "run or pause time"),
                        ("[  ]", "slower, faster"),
                        ("N", "back to now"),
                        ("T  G  Y", "evidence colours, gravity, system view"),
                        ("M", "ambience"),
                        ("Esc", "close, or clear the selection"),
                    ];
                    let avail = rect.height() - 110.0;
                    egui::ScrollArea::vertical()
                        .max_height(avail)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("help-grid")
                                .num_columns(2)
                                .spacing([16.0, 5.0])
                                .show(ui, |ui| {
                                    for (k, v) in rows {
                                        ui.label(RichText::new(k).size(12.5).color(theme::BRASS));
                                        ui.label(RichText::new(v).size(12.5).color(theme::TEXT));
                                        ui.end_row();
                                    }
                                });
                        });
                });
            });
        if close {
            self.show_help = false;
        }
    }

    fn intro_panel(&mut self, ctx: &egui::Context, l: &Layout) {
        // gets out of the way by itself too
        if self.anim_t - self.intro_born > 40.0 {
            self.dismiss_intro();
            return;
        }
        let mut done = false;
        let w = 400.0f32.min(l.bar.width().max(340.0));
        egui::Area::new(egui::Id::new("intro"))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(egui::pos2(l.bar.center().x, l.bar.top() - 10.0))
            .show(ctx, |ui| {
                panel_frame(ctx).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.label(
                        RichText::new("This is Earth, where it is right now.")
                            .font(theme::heading(22.0))
                            .color(theme::VELLUM),
                    );
                    ui.label(RichText::new("Scroll to zoom out, all the way to the cosmic web. Drag to move. Double-click anything to fly there.").size(13.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("/ searches, H brings you home.").size(13.0).color(theme::TEXT_DIM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            done = ui.button("Got it").clicked();
                        });
                    });
                    if let Some(problem) = crate::spice::problems().first() {
                        ui.label(
                            RichText::new(format!("{problem} Run tools/fetch_data.py to replace it."))
                                .size(11.5)
                                .color(theme::BRASS),
                        );
                    } else if !self.data_gaps.is_empty() {
                        ui.label(
                            RichText::new("Real planet maps and spacecraft tracks aren't downloaded yet: run tools/fetch_data.py.")
                                .size(11.5)
                                .color(theme::BRASS),
                        );
                    }
                });
            });
        if done {
            self.dismiss_intro();
        }
    }

    fn hover_tip(&self, ctx: &egui::Context) {
        let Some(obj) = self.hover else { return };
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        // don't obscure the thing being pointed at when it already has a big card
        if self.focus == Some(obj) {
            return;
        }
        let name = obj_name(&self.u, obj);
        egui::Area::new(egui::Id::new("hover-tip"))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .fixed_pos(p + egui::vec2(14.0, 14.0))
            .show(ctx, |ui| {
                panel_frame(ctx)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.label(RichText::new(name).size(13.0).color(theme::VELLUM));
                        if !self.learned_click {
                            ui.label(
                                RichText::new("Click to select, double-click to fly")
                                    .size(11.0)
                                    .color(theme::TEXT_DIM),
                            );
                        }
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_rects(l: &Layout) -> Vec<(&'static str, Rect)> {
        let mut v = vec![
            ("toolbar", l.toolbar),
            ("title", l.title),
            ("bar", l.bar),
            ("left", l.left_col),
            ("right", l.right_col),
        ];
        if let Some(r) = l.rail {
            v.push(("rail", r));
        }
        v
    }

    #[test]
    fn panels_never_collide_at_any_window_size() {
        for (w, h) in [
            (560.0, 380.0),
            (640.0, 420.0),
            (800.0, 500.0),
            (900.0, 560.0),
            (1024.0, 600.0),
            (1280.0, 720.0),
            (1600.0, 900.0),
            (1680.0, 980.0),
            (2560.0, 1440.0),
            (3840.0, 2160.0),
        ] {
            let l = layout(w, h);
            let win = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h));
            let rects = all_rects(&l);
            for (i, (na, a)) in rects.iter().enumerate() {
                assert!(win.contains_rect(*a), "{na} leaves a {w}x{h} window: {a:?}");
                assert!(
                    a.width() > 0.0 && a.height() > 0.0,
                    "{na} is empty at {w}x{h}: {a:?}"
                );
                for (nb, b) in &rects[i + 1..] {
                    // the left and right columns only share space when told to
                    assert!(
                        !a.shrink(0.5).intersects(b.shrink(0.5)),
                        "{na} and {nb} collide at {w}x{h}: {a:?} {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn scale_bar_is_round_and_fits() {
        for mpp in [
            1.0e3, 2.0e4, 1.0e6, 3.3e7, 1.0e10, 1.0e13, 1.0e16, 1.0e19, 5.0e21,
        ] {
            let (len, px) = scale_bar(mpp, 180.0);
            assert!(px <= 180.0 + 1e-6, "bar is {px} px at mpp {mpp}");
            assert!(px >= 180.0 / 5.1, "bar is only {px} px at mpp {mpp}");
            assert!(!fmt_bar(len).is_empty());
        }
    }

    #[test]
    fn distances_read_like_a_person_would_say_them() {
        assert_eq!(fmt_words(3.6e7), "36,000 km");
        assert_eq!(fmt_words(3.0e9), "3 million km");
        assert_eq!(fmt_words(1.0e10), "0.067 AU");
        assert_eq!(fmt_words(1.5e13), "100 AU");
        assert_eq!(fmt_words(9.46e15 * 4.2), "4.2 light-years");
        assert_eq!(fmt_words(9.46e15 * 1.0e5), "100 thousand light-years");
        assert_eq!(fmt_words(9.46e15 * 2.5e6), "2.5 million light-years");
    }

    #[test]
    fn light_times_read_naturally() {
        const C: f64 = 299_792_458.0;
        assert_eq!(fmt_light_time(C * 500.0), "8.3 minutes");
        assert_eq!(fmt_light_time(C * 1.0), "1 second");
        assert_eq!(fmt_light_time(9.4607e15 * 4.0), "4 years");
        assert_eq!(fmt_light_time(9.4607e15 * 2.5e6), "2.5 million years");
    }
}
