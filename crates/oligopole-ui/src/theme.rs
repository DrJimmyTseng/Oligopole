use egui::{Color32, Context, FontFamily, FontId, TextStyle, Visuals};

pub const BG_DARK_MATRIX: Color32 = Color32::from_rgb(10, 12, 16);
pub const BG_PANEL_HEADER: Color32 = Color32::from_rgb(18, 22, 30);
pub const AMBER_PRIMARY: Color32 = Color32::from_rgb(255, 176, 0);
pub const CYAN_ACCENT: Color32 = Color32::from_rgb(0, 212, 255);
pub const EMERALD_ACCENT: Color32 = Color32::from_rgb(0, 230, 118);
pub const BID_GREEN: Color32 = Color32::from_rgb(38, 166, 91);
pub const ASK_RED: Color32 = Color32::from_rgb(235, 77, 75);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 163, 184);

/// Applies the high-contrast Bloomberg Terminal dark-matrix visual theme to `egui::Context`.
pub fn apply_bloomberg_theme(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG_DARK_MATRIX;
    visuals.window_fill = BG_DARK_MATRIX;
    visuals.extreme_bg_color = Color32::from_rgb(6, 8, 12);
    visuals.override_text_color = Some(Color32::from_rgb(230, 235, 242));
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(13.0, FontFamily::Monospace),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(12.5, FontFamily::Monospace),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(15.0, FontFamily::Monospace),
    );
    ctx.set_style(style);
}
