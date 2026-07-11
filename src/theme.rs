//! Visual language: absolute black canvas, luminous vector ink, sparse glass.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke};

pub const INK_GOLD: [f32; 3] = [0.83, 0.68, 0.38];
pub const INK_GOLD_DIM: [f32; 3] = [0.55, 0.45, 0.26];
pub const INK_BLUE: [f32; 3] = [0.49, 0.64, 1.0];
pub const INK_WHITE: [f32; 3] = [0.91, 0.93, 0.96];
pub const INK_FAINT: [f32; 3] = [0.35, 0.38, 0.44];

pub fn rgba(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

pub fn col32(c: [f32; 3], a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (a * 255.0) as u8,
    )
}

pub const GLASS_FILL: Color32 = Color32::from_rgba_premultiplied(9, 11, 16, 216);
pub const GLASS_STROKE: Color32 = Color32::from_rgba_premultiplied(58, 62, 74, 90);
pub const TEXT: Color32 = Color32::from_rgb(216, 221, 230);
pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 145, 158);
pub const GOLD: Color32 = Color32::from_rgb(212, 175, 105);
pub const GOLD_DIM: Color32 = Color32::from_rgb(140, 116, 70);
pub const BLUE: Color32 = Color32::from_rgb(125, 163, 255);

pub fn install_fonts(ctx: &egui::Context) {
    let root = crate::data::asset_root();
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str| -> bool {
        let path = root.join("fonts").join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            fonts
                .font_data
                .insert(name.to_string(), std::sync::Arc::new(FontData::from_owned(bytes)));
            true
        } else {
            false
        }
    };
    let spectral = add("spectral", "Spectral.ttf");
    let cormorant = add("cormorant", "CormorantGaramond.ttf");
    if spectral {
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "spectral".into());
    }
    if cormorant {
        fonts
            .families
            .insert(FontFamily::Name("heading".into()), vec![
                "cormorant".into(),
                "spectral".into(),
            ]);
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = Color32::from_rgb(4, 5, 8);
    v.window_fill = GLASS_FILL;
    v.window_stroke = Stroke::new(1.0, GLASS_STROKE);
    v.window_corner_radius = CornerRadius::same(8);
    v.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    v.widgets.inactive.bg_fill = Color32::from_rgba_premultiplied(22, 25, 33, 160);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = Color32::from_rgba_premultiplied(36, 40, 52, 200);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, GOLD);
    v.widgets.active.bg_fill = Color32::from_rgba_premultiplied(48, 44, 30, 220);
    v.widgets.active.fg_stroke = Stroke::new(1.2, GOLD);
    v.selection.bg_fill = Color32::from_rgba_premultiplied(70, 58, 30, 180);
    v.selection.stroke = Stroke::new(1.0, GOLD);
    v.hyperlink_color = BLUE;
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(10);
    ctx.set_style(style);
}
