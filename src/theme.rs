//! Visual language: true-black space, an old star atlas's ink-blue panels,
//! brass for the instrument (selection, scale needle), vellum for reading.
//! Cormorant for place names, Spectral for map labels, Inter for controls.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke};

pub const INK_GOLD: [f32; 3] = [0.83, 0.68, 0.38];
pub const INK_GOLD_DIM: [f32; 3] = [0.55, 0.45, 0.26];
pub const INK_BLUE: [f32; 3] = [0.49, 0.64, 1.0];
pub const INK_WHITE: [f32; 3] = [0.91, 0.93, 0.96];
pub const INK_FAINT: [f32; 3] = [0.46, 0.50, 0.58];

pub fn rgba(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

pub fn col32(c: [f32; 3], a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (a.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Ink-blue panel fill, translucent so space still reads through it.
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(8, 13, 26, 226);
pub const PANEL_LINE: Color32 = Color32::from_rgba_premultiplied(66, 62, 52, 120);
pub const VELLUM: Color32 = Color32::from_rgb(233, 228, 212);
pub const TEXT: Color32 = Color32::from_rgb(214, 214, 208);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 158, 172);
pub const BRASS: Color32 = Color32::from_rgb(214, 180, 112);
pub const BRASS_DIM: Color32 = Color32::from_rgb(138, 116, 74);
pub const BLUE: Color32 = Color32::from_rgb(139, 176, 255);

/// Text set on open canvas needs a soft dark halo to survive any background.
pub const HALO: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 215);

pub fn heading(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("heading".into()))
}

pub fn serif(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("serif".into()))
}

pub fn ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install_fonts(ctx: &egui::Context) {
    let root = crate::data::asset_root();
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str| -> bool {
        let path = root.join("fonts").join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            fonts.font_data.insert(name.to_string(), std::sync::Arc::new(FontData::from_owned(bytes)));
            true
        } else {
            log::warn!("font missing: {}", path.display());
            false
        }
    };
    let inter = add("inter", "Inter.ttf");
    let spectral = add("spectral", "Spectral.ttf");
    let cormorant = add("cormorant", "CormorantGaramond.ttf");
    let math = add("math", "NotoSansMath.ttf");
    {
        let prop = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
        if inter {
            prop.insert(0, "inter".into());
        }
        if math {
            prop.push("math".into());
        }
    }
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let with_first = |first: Option<&str>, more: Option<&str>| -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        v.extend(first.map(String::from));
        v.extend(more.map(String::from));
        v.extend(fallback.iter().cloned());
        v
    };
    fonts.families.insert(
        FontFamily::Name("serif".into()),
        with_first(spectral.then_some("spectral"), None),
    );
    fonts.families.insert(
        FontFamily::Name("heading".into()),
        with_first(cormorant.then_some("cormorant"), spectral.then_some("spectral")),
    );
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = Color32::from_rgb(0, 0, 0);
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, PANEL_LINE);
    v.window_corner_radius = CornerRadius::same(6);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 22,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.popup_shadow = v.window_shadow;
    v.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_LINE);
    v.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(26, 32, 50, 190);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgba_premultiplied(26, 32, 50, 190);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = CornerRadius::same(4);
    v.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(44, 50, 72, 225);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgba_premultiplied(44, 50, 72, 225);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, VELLUM);
    v.widgets.hovered.corner_radius = CornerRadius::same(4);
    v.widgets.active.bg_fill = Color32::from_rgba_premultiplied(70, 58, 34, 235);
    v.widgets.active.weak_bg_fill = Color32::from_rgba_premultiplied(70, 58, 34, 235);
    v.widgets.active.fg_stroke = Stroke::new(1.2, BRASS);
    v.widgets.active.corner_radius = CornerRadius::same(4);
    v.selection.bg_fill = Color32::from_rgba_premultiplied(84, 68, 34, 215);
    v.selection.stroke = Stroke::new(1.0, BRASS);
    v.hyperlink_color = BLUE;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    style.text_styles.insert(egui::TextStyle::Body, ui(13.0));
    style.text_styles.insert(egui::TextStyle::Button, ui(13.0));
    style.text_styles.insert(egui::TextStyle::Small, ui(11.0));
    ctx.set_style(style);
}
