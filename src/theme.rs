use egui::{Color32, CornerRadius, Style, Theme, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x60, 0x9B, 0xF8);
pub const OK_GREEN: Color32 = Color32::from_rgb(0x22, 0xC5, 0x5E);
pub const WARN_YELLOW: Color32 = Color32::from_rgb(0xEA, 0xB3, 0x08);
pub const ERR_RED: Color32 = Color32::from_rgb(0xEF, 0x44, 0x44);

/// Style used for both light and dark OS themes: the app always looks dark.
pub fn style() -> Style {
    let mut style = Style::default();
    let v = &mut style.visuals;
    *v = Visuals::dark();

    v.panel_fill = Color32::from_rgb(0x14, 0x17, 0x1F);
    v.window_fill = Color32::from_rgb(0x1A, 0x1E, 0x27);
    v.extreme_bg_color = Color32::from_rgb(0x0D, 0x0F, 0x14);
    v.faint_bg_color = Color32::from_rgb(0x21, 0x26, 0x33);

    v.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x2A, 0x30, 0x3D));
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x2A, 0x30, 0x3D));

    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);

    let interactive = [
        (&mut v.widgets.inactive, (0x24, 0x2A, 0x38)),
        (&mut v.widgets.hovered, (0x2D, 0x35, 0x47)),
        (&mut v.widgets.active, (0x1E, 0x23, 0x2F)),
    ];
    for (w, (r, g, b)) in interactive {
        w.bg_fill = Color32::from_rgb(r, g, b);
        w.weak_bg_fill = Color32::from_rgb(r, g, b);
        w.corner_radius = CornerRadius::same(6);
        w.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0xD7, 0xDD, 0xE8));
    }
    v.widgets.hovered.bg_fill = ACCENT.linear_multiply(0.28);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);

    v.override_text_color = Some(Color32::from_rgb(0xE2, 0xE7, 0xF0));

    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(6);
    style
}

/// Apply the application-wide visual style: modern dark theme with an accent
/// color, rounded corners and comfortable spacing.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(Theme::Dark);
    let s = style();
    ctx.set_style_of(Theme::Dark, s.clone());
    ctx.set_style_of(Theme::Light, s);
}
