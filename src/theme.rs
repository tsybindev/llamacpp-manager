use egui::{Color32, CornerRadius, Style, Theme, ThemePreference, Visuals};

// --- Design tokens (dark theme baseline) ---
/// App background / side panels.
pub const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x17);
/// Cards and containers.
pub const CARD: Color32 = Color32::from_rgb(0x18, 0x1B, 0x24);
/// Inputs and nested panels.
pub const INPUT: Color32 = Color32::from_rgb(0x21, 0x26, 0x34);
/// 1px borders.
pub const BORDER: Color32 = Color32::from_rgb(0x2C, 0x33, 0x45);
/// Muted secondary text.
pub const MUTED: Color32 = Color32::from_rgb(0x94, 0xA3, 0xB8);

pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
#[allow(dead_code)] // reserved for hover states
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x60, 0x9B, 0xF8);
pub const OK_GREEN: Color32 = Color32::from_rgb(0x10, 0xB9, 0x81);
pub const WARN_YELLOW: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);
pub const ERR_RED: Color32 = Color32::from_rgb(0xEF, 0x44, 0x44);

/// Corner radius for buttons/inputs.
pub const RADIUS_WIDGET: u8 = 6;
/// Corner radius for cards/panels.
pub const RADIUS_CARD: u8 = 8;

/// User-selectable UI theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
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
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 10.0);
    s.button_padding = egui::vec2(12.0, 6.0);
    s.menu_margin = egui::Margin::same(6);
    s.window_margin = egui::Margin::same(16);
    s.icon_width_inner = 16.0;
    s.icon_width = 20.0;
    s.combo_width = 180.0;

    for w in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        w.corner_radius = CornerRadius::same(RADIUS_WIDGET);
    }

    style.visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.text_cursor.stroke = egui::Stroke::new(1.5, ACCENT);
}

/// Dark theme: developer-dashboard palette (Raycast/Linear style).
pub fn dark_style() -> Style {
    let mut style = Style::default();
    let v = &mut style.visuals;
    *v = Visuals::dark();

    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = INPUT;
    v.faint_bg_color = CARD;

    v.window_stroke = egui::Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTED);
    v.widgets.noninteractive.bg_fill = CARD;

    let interactive = [
        (&mut v.widgets.inactive, INPUT),
        (&mut v.widgets.hovered, Color32::from_rgb(0x2A, 0x31, 0x44)),
        (&mut v.widgets.active, Color32::from_rgb(0x1A, 0x1F, 0x2C)),
    ];
    for (w, fill) in interactive {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = egui::Stroke::new(1.0, BORDER);
        w.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0xE2, 0xE7, 0xF0));
    }
    v.widgets.hovered.bg_fill = ACCENT.linear_multiply(0.28);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);

    v.override_text_color = Some(Color32::from_rgb(0xE2, 0xE7, 0xF0));

    common(&mut style);
    style
}

/// Light theme with the same accent color and layout metrics.
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
