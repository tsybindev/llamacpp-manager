use egui::{Color32, CornerRadius, Style, Theme, ThemePreference, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x60, 0x9B, 0xF8);
pub const OK_GREEN: Color32 = Color32::from_rgb(0x16, 0xA3, 0x4A);
pub const WARN_YELLOW: Color32 = Color32::from_rgb(0xD9, 0x94, 0x06);
pub const ERR_RED: Color32 = Color32::from_rgb(0xDC, 0x2C, 0x36);

/// User-selectable UI theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThemeMode {
    Dark,
    Light,
    #[default]
    System,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Dark, ThemeMode::Light, ThemeMode::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "🌙  Тёмная",
            ThemeMode::Light => "☀️  Светлая",
            ThemeMode::System => "💻  Системная",
        }
    }

    fn preference(self) -> ThemePreference {
        match self {
            ThemeMode::Dark => ThemePreference::Dark,
            ThemeMode::Light => ThemePreference::Light,
            ThemeMode::System => ThemePreference::System,
        }
    }
}

/// Style shared by both themes: accent color, rounded corners, spacing.
fn common(style: &mut Style) {
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.icon_width_inner = 16.0;
    style.spacing.icon_width = 20.0;

    for w in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        w.corner_radius = CornerRadius::same(6);
    }

    style.visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
}

/// Modern dark theme with an accent color.
pub fn dark_style() -> Style {
    let mut style = Style::default();
    let v = &mut style.visuals;
    *v = Visuals::dark();

    v.panel_fill = Color32::from_rgb(0x14, 0x17, 0x1F);
    v.window_fill = Color32::from_rgb(0x1A, 0x1E, 0x27);
    v.extreme_bg_color = Color32::from_rgb(0x0D, 0x0F, 0x14);
    v.faint_bg_color = Color32::from_rgb(0x21, 0x26, 0x33);

    v.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x2A, 0x30, 0x3D));
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x2A, 0x30, 0x3D));

    let interactive = [
        (&mut v.widgets.inactive, (0x24, 0x2A, 0x38)),
        (&mut v.widgets.hovered, (0x2D, 0x35, 0x47)),
        (&mut v.widgets.active, (0x1E, 0x23, 0x2F)),
    ];
    for (w, (r, g, b)) in interactive {
        w.bg_fill = Color32::from_rgb(r, g, b);
        w.weak_bg_fill = Color32::from_rgb(r, g, b);
        w.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0xD7, 0xDD, 0xE8));
    }
    v.widgets.hovered.bg_fill = ACCENT.linear_multiply(0.28);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);

    v.override_text_color = Some(Color32::from_rgb(0xE2, 0xE7, 0xF0));

    common(&mut style);
    style
}

/// Clean light theme with the same accent color.
pub fn light_style() -> Style {
    let mut style = Style::default();
    let v = &mut style.visuals;
    *v = Visuals::light();

    v.panel_fill = Color32::from_rgb(0xF3, 0xF4, 0xF6);
    v.window_fill = Color32::from_rgb(0xFC, 0xFD, 0xFE);
    v.extreme_bg_color = Color32::from_rgb(0xE5, 0xE7, 0xEB);
    v.faint_bg_color = Color32::from_rgb(0xEA, 0xEC, 0xF0);

    v.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0xD3, 0xD8, 0xE0));
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0xD3, 0xD8, 0xE0));

    let interactive = [
        (&mut v.widgets.inactive, (0xE8, 0xEA, 0xEE)),
        (&mut v.widgets.hovered, (0xDC, 0xE1, 0xE9)),
        (&mut v.widgets.active, (0xCD, 0xD3, 0xDC)),
    ];
    for (w, (r, g, b)) in interactive {
        w.bg_fill = Color32::from_rgb(r, g, b);
        w.weak_bg_fill = Color32::from_rgb(r, g, b);
        w.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x1F, 0x24, 0x2E));
    }
    v.widgets.hovered.bg_fill = ACCENT.linear_multiply(0.18);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x1D, 0x4E, 0xD8));

    v.override_text_color = Some(Color32::from_rgb(0x1F, 0x24, 0x2E));

    common(&mut style);
    style
}

/// Apply the chosen theme mode to the context.
pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    ctx.set_theme(mode.preference());
    ctx.set_style_of(Theme::Dark, dark_style());
    ctx.set_style_of(Theme::Light, light_style());
}
