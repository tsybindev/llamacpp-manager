//! Переиспользуемые UI-виджеты и дизайн-токены (карточки, бейджи, гейджи,
//! терминальные панели). Экраны живут в подпрограммах [`server`], [`models`],
//! [`builds`] и [`settings`].

pub mod builds;
pub mod logs;
pub mod models;
pub mod server;
pub mod settings;

use std::path::PathBuf;

use egui::{Color32, Frame, RichText, Sense, Stroke, TextEdit, Ui, Vec2};

use crate::github;
use crate::params::{ParamDef, ParamKind, ParamState};
use crate::process_mgr::ServerState;
use crate::theme::{self, BORDER, CARD, INPUT, MUTED, RADIUS_CARD, RADIUS_WIDGET};

/// Ширина колонки подписей в формах (Grid/строки параметров).
pub const LABEL_WIDTH: f32 = 150.0;
/// Максимальная ширина текстовых инпутов с путями.
pub const PATH_INPUT_WIDTH: f32 = 450.0;
/// Ширина кнопки «Обзор…».
const BROWSE_BUTTON_WIDTH: f32 = 88.0;

// ---------------------------------------------------------------------------
// Карточки и заголовки
// ---------------------------------------------------------------------------

/// Карточка-контейнер: тёмный фон, скругление 8, бордер 1px, внутренний отступ 12.
pub fn card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(RADIUS_CARD)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

/// Карточка с подзаголовком секции (13px, uppercase, приглушённый).
pub fn card_titled(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    card(ui, |ui| {
        ui.add_space(-2.0);
        section_label(ui, title);
        ui.add_space(4.0);
        add_contents(ui);
    });
}

/// Подзаголовок секции: мелкий, uppercase, приглушённый цвет.
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(12.0).strong().color(MUTED));
}

/// Заголовок экрана: 19px bold + опциональная подпись справа.
pub fn screen_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(19.0).strong());
        if !subtitle.is_empty() {
            ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
        }
    });
    ui.add_space(2.0);
}

// ---------------------------------------------------------------------------
// Бейджи и статусы
// ---------------------------------------------------------------------------

/// Pill-бейдж: скругление 4, мелкий шрифт 11, полупрозрачный фон статуса.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let bg = if ui.visuals().dark_mode {
        color.linear_multiply(0.22)
    } else {
        color.gamma_multiply(0.16)
    };
    Frame::new()
        .fill(bg)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).strong().color(color));
        });
}

/// Бейдж бэкенда сборки (Vulkan/CUDA/CPU/…).
pub fn backend_badge(ui: &mut Ui, backend: github::Backend) {
    let color = match backend {
        github::Backend::Cuda => Color32::from_rgb(0x76, 0xB9, 0x00),
        github::Backend::Vulkan => ACCENT_BADGE,
        github::Backend::Rocm => Color32::from_rgb(0xE0, 0x51, 0x8E),
        github::Backend::Cpu => MUTED,
        _ => theme::WARN_YELLOW,
    };
    badge(ui, backend.label(), color);
}

const ACCENT_BADGE: Color32 = Color32::from_rgb(0x60, 0xA5, 0xFA);

/// Цветная точка-индикатор статуса.
pub fn status_dot(ui: &mut Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}

/// Цвет и подсказка для состояния сервера.
pub fn state_status(state: ServerState) -> (Color32, &'static str) {
    match state {
        ServerState::Stopped => (Color32::from_rgb(0x8A, 0x94, 0xA6), "Сервер не запущен"),
        ServerState::Starting => (
            theme::WARN_YELLOW,
            "Идёт загрузка модели, проверяется готовность…",
        ),
        ServerState::Ready => (
            theme::OK_GREEN,
            "Сервер отвечает и готов принимать запросы",
        ),
        ServerState::RestartScheduled => (
            theme::WARN_YELLOW,
            "Процесс упал, ожидается автоматический перезапуск",
        ),
        ServerState::Crashed => (theme::ERR_RED, "Требуется вмешательство пользователя"),
    }
}

// ---------------------------------------------------------------------------
// Навигация (sidebar)
// ---------------------------------------------------------------------------

/// Полноширинная кнопка навигации с левым выравниванием (текстовая —
/// часть глифов в дефолтном шрифте egui не покрывается).
pub fn nav_item(ui: &mut Ui, selected: bool, label: &str) -> egui::Response {
    let height = 30.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let visuals = ui.style().interact(&response);
    let bg = if selected {
        theme::ACCENT.linear_multiply(0.25)
    } else {
        visuals.bg_fill
    };
    if response.hovered() || selected {
        ui.painter()
            .rect_filled(rect.expand2(Vec2::new(4.0, 1.0)), RADIUS_WIDGET, bg);
    }
    let text_color = if selected {
        Color32::from_rgb(0xE2, 0xE7, 0xF0)
    } else {
        MUTED
    };
    if selected {
        // Акцентная полоска слева у активного пункта.
        let bar = egui::Rect::from_min_size(
            egui::Pos2::new(rect.left() + 2.0, rect.top() + 3.0),
            Vec2::new(3.0, rect.height() - 6.0),
        );
        ui.painter().rect_filled(bar, 2.0, theme::ACCENT);
    }
    ui.painter().text(
        egui::Pos2::new(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        text_color,
    );
    response.on_hover_text(label)
}

// ---------------------------------------------------------------------------
// Подтверждение удаления
// ---------------------------------------------------------------------------

/// Инлайн-подтверждение удаления с выбором «Да, удалить» / «Отмена».
/// Возвращает `Some(true)` — подтверждено, `Some(false)` — отмена,
/// `None` — пользователь ещё не ответил. `warning` — предупреждение
/// о последствиях (например, «используется в пресетах: …»).
pub fn delete_confirm(ui: &mut Ui, warning: Option<&str>) -> Option<bool> {
    let mut result: Option<bool> = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Удалить безвозвратно?")
                .size(12.0)
                .strong()
                .color(theme::WARN_YELLOW),
        );
        let yes = egui::Button::new(
            RichText::new("Да, удалить").size(12.5).strong().color(theme::ERR_RED),
        )
        .fill(theme::ERR_RED.linear_multiply(0.16))
        .stroke(Stroke::new(1.0, theme::ERR_RED.linear_multiply(0.6)));
        if ui
            .add(yes)
            .on_hover_text("Действие нельзя отменить")
            .clicked()
        {
            result = Some(true);
        }
        if ui.small_button("Отмена").clicked() {
            result = Some(false);
        }
        if let Some(warning) = warning {
            ui.label(RichText::new(warning).size(11.0).color(theme::WARN_YELLOW));
        }
    });
    result
}

// ---------------------------------------------------------------------------
// Терминал логов и CLI-предпросмотр
// ---------------------------------------------------------------------------

/// Терминальное окно: тёмный контейнер, monospace, автопрокрутка снаружи.
/// Цвет каждой строки задаёт вызывающий код (`line_color` / `level_color`).
pub fn terminal(ui: &mut Ui, lines: &[(String, Color32)], empty_hint: &str) {
    Frame::new()
        .fill(INPUT)
        .corner_radius(RADIUS_WIDGET)
        .inner_margin(8.0)
        .stroke(Stroke::new(1.0, BORDER))
        .show(ui, |ui| {
            ui.style_mut().spacing.item_spacing.y = 2.0;
            if lines.is_empty() {
                ui.label(RichText::new(empty_hint).monospace().size(12.0).color(MUTED));
            }
            for (line, color) in lines {
                ui.label(RichText::new(line).monospace().size(12.0).color(*color));
            }
        });
}

/// Цвет строки терминала по уровню в тексте (для сырого вывода процесса).
pub fn line_color(line: &str) -> Color32 {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("fatal") {
        theme::ERR_RED
    } else if lower.contains("warn") {
        theme::WARN_YELLOW
    } else {
        Color32::from_rgb(0xB9, 0xC2, 0xD0)
    }
}

/// Цвет уровня журнала приложения.
pub fn level_color(level: log::Level) -> Color32 {
    match level {
        log::Level::Error => theme::ERR_RED,
        log::Level::Warn => theme::WARN_YELLOW,
        log::Level::Debug | log::Level::Trace => MUTED,
        log::Level::Info => Color32::from_rgb(0xB9, 0xC2, 0xD0),
    }
}

/// Блок предпросмотра команды: monospace в тёмном контейнере + кнопка копирования.
pub fn cli_preview(ui: &mut Ui, command: &str) {
    ui.horizontal(|ui| {
        section_label(ui, "Команда запуска");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Копировать").on_hover_text("Скопировать команду в буфер обмена").clicked() {
                ui.ctx().copy_text(command.to_string());
            }
        });
    });
    Frame::new()
        .fill(INPUT)
        .corner_radius(RADIUS_WIDGET)
        .inner_margin(8.0)
        .stroke(Stroke::new(1.0, BORDER))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(command).monospace().size(12.0).color(ACCENT_BADGE));
        });
}

// ---------------------------------------------------------------------------
// Гейдж памяти
// ---------------------------------------------------------------------------

/// Сегмент гейджа: (подпись, байты, цвет).
pub struct GaugeSegment {
    pub label: String,
    pub bytes: u64,
    pub color: Color32,
}

/// Горизонтальный гейдж памяти: закрашенная часть — сумма сегментов,
/// свободное место — приглушённое. Под баром — легенда `[ Веса | KV | Свободно ]`.
pub fn memory_gauge(ui: &mut Ui, total: u64, segments: &[GaugeSegment]) {
    let used: u64 = segments.iter().map(|s| s.bytes).sum();
    let fraction = if total > 0 { (used as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    let color = if fraction > 0.95 {
        theme::ERR_RED
    } else if fraction > 0.80 {
        theme::WARN_YELLOW
    } else {
        theme::OK_GREEN
    };

    let height = 14.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(rect, RADIUS_WIDGET, theme::BG);
    ui.painter().rect_stroke(rect, RADIUS_WIDGET, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    let inner = rect.shrink2(Vec2::new(2.0, 3.0));
    let mut x = inner.left();
    for segment in segments {
        if total == 0 || segment.bytes == 0 {
            continue;
        }
        let w = inner.width() * (segment.bytes as f32 / total as f32);
        let seg_rect = egui::Rect::from_min_size(egui::Pos2::new(x, inner.top()), Vec2::new(w, inner.height()));
        ui.painter().rect_filled(seg_rect, 2.0, segment.color);
        x += w;
    }

    let mut legend = String::new();
    for segment in segments {
        legend.push_str(&format!("{}: {}   ", segment.label, format_size(segment.bytes)));
    }
    legend.push_str(&format!("Свободно: {}", format_size(total.saturating_sub(used))));
    ui.label(RichText::new(legend).size(12.0).color(color));
}

/// Итоговый цвет-предупреждение по заполнению памяти.
pub fn gauge_color(fraction: f32) -> Color32 {
    if fraction > 0.95 {
        theme::ERR_RED
    } else if fraction > 0.80 {
        theme::WARN_YELLOW
    } else {
        theme::OK_GREEN
    }
}

/// Объём и доступная память системы (Linux: /proc/meminfo, иначе — None).
pub fn mem_info() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let field = |name: &str| -> Option<u64> {
            text.lines().find_map(|line| {
                let mut parts = line.split_whitespace();
                (parts.next()? == name)
                    .then(|| parts.next()?.parse::<u64>().ok())
                    .flatten()
                    .map(|kb| kb * 1024)
            })
        };
        Some((field("MemTotal:")?, field("MemAvailable:")?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Формы: пути и параметры
// ---------------------------------------------------------------------------

/// Что диалог файлов должен выбрать.
#[derive(Clone, Copy)]
pub enum PathPick {
    File,
    Folder,
}

/// Строка выбора пути: подпись фиксированной ширины, инпут (до 450px), «Обзор…».
/// Возвращает true, если путь изменился.
pub fn path_row(ui: &mut Ui, label: &str, path: &mut PathBuf, hint: &str, pick: PathPick) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_WIDTH, 18.0], egui::Label::new(RichText::new(label).size(13.0)))
            .on_hover_text(hint);
        let mut text = path.display().to_string();
        let edit_width = (PATH_INPUT_WIDTH - 0.0).min(ui.available_width() - BROWSE_BUTTON_WIDTH).max(160.0);
        let response = ui.add(TextEdit::singleline(&mut text).desired_width(edit_width).font(egui::TextStyle::Monospace));
        if response.changed() {
            *path = PathBuf::from(text.trim());
            changed = true;
        }
        if ui.button("Обзор…").clicked() {
            // Диалог создаётся и awaited только по щелчку — pick_file блокирует.
            let dialog = rfd::FileDialog::new().set_title(format!("Выберите: {label}"));
            let picked = match pick {
                PathPick::File => dialog.pick_file(),
                PathPick::Folder => dialog.pick_folder(),
            };
            if let Some(new_path) = picked {
                *path = new_path;
                changed = true;
            }
        }
    });
    changed
}

/// Строка параметра: `☑ Название 🛈 | виджет ввода`. Для Path-параметров
/// дополнительно доступны выпадающий список локальных моделей (`local_models`)
/// и кнопка «Обзор…». Возвращает true при изменении.
pub fn param_row(
    ui: &mut Ui,
    def: &ParamDef,
    state: &mut ParamState,
    local_models: &[PathBuf],
) -> bool {
    let before = state.clone();
    let mut tooltip = format!("{}\n\nФлаг: {}", def.description, def.flag);
    if let Some(short) = &def.short {
        tooltip.push_str(&format!(" / {short}"));
    }

    match def.kind {
        // У bool-параметров нет значения: чекбокс — весь контрол.
        ParamKind::Bool => {
            let mut on = state.is_enabled(&def.id);
            if ui.checkbox(&mut on, RichText::new(&def.name).size(13.0)).on_hover_text(&tooltip).changed() {
                state.set(&def.id, on);
            }
        }
        _ => {
            ui.horizontal(|ui| {
                let mut enabled = state.is_enabled(&def.id);
                ui.checkbox(&mut enabled, "").on_hover_text(&tooltip);
                ui.add_sized([LABEL_WIDTH, 18.0], egui::Label::new(RichText::new(&def.name).size(13.0)))
                    .on_hover_text(&tooltip);

                let value = state.entries.get(&def.id).and_then(|e| e.value.clone()).or_else(|| def.default.clone());
                let field_width = (ui.available_width() - 8.0).max(120.0);

                let new_value = match def.kind {
                    ParamKind::Int => {
                        let min = def.min.unwrap_or(i64::MIN as f64) as i64;
                        let max = def.max.unwrap_or(i64::MAX as f64).min(i64::MAX as f64) as i64;
                        let mut v = value.and_then(|x| x.as_i64()).unwrap_or(min.max(0));
                        if ui
                            .add_sized(
                                [130.0, 20.0],
                                egui::DragValue::new(&mut v)
                                    .range(min..=max)
                                    .custom_formatter(|n, _| format!("{n}"))
                                    .custom_parser(|s| s.trim().parse::<i64>().ok().map(|n| n as f64)),
                            )
                            .on_hover_text(&tooltip)
                            .changed()
                        {
                            Some(serde_json::json!(v))
                        } else {
                            None
                        }
                    }
                    ParamKind::Float => {
                        let min = def.min.unwrap_or(f64::MIN);
                        let max = def.max.unwrap_or(f64::MAX);
                        let mut v = value.and_then(|x| x.as_f64()).unwrap_or(min.max(0.0));
                        if ui.add_sized([130.0, 20.0], egui::DragValue::new(&mut v).range(min..=max).speed(0.05)).on_hover_text(&tooltip).changed() {
                            Some(serde_json::json!(v))
                        } else {
                            None
                        }
                    }
                    ParamKind::Enum => {
                        let previous = value.as_ref().and_then(|x| x.as_str()).unwrap_or_default().to_string();
                        let mut current = previous.clone();
                        egui::ComboBox::from_id_salt(def.id.clone())
                            .selected_text(current.clone())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for option in &def.options {
                                    ui.selectable_value(&mut current, option.clone(), option);
                                }
                            })
                            .response
                            .on_hover_text(&tooltip);
                        if current != previous {
                            Some(serde_json::json!(current))
                        } else {
                            None
                        }
                    }
                    ParamKind::String => {
                        let mut text = value.as_ref().and_then(|x| x.as_str()).unwrap_or_default().to_string();
                        if ui.add(TextEdit::singleline(&mut text).desired_width(field_width.min(300.0))).on_hover_text(&tooltip).changed() {
                            Some(serde_json::json!(text.trim()))
                        } else {
                            None
                        }
                    }
                    ParamKind::Path => {
                        let mut text = value
                            .as_ref()
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let before = text.clone();
                        let mut picked: Option<serde_json::Value> = None;
                        let extras_width = if local_models.is_empty() { 0.0 } else { 130.0 };
                        let edit_width =
                            (field_width - BROWSE_BUTTON_WIDTH - extras_width).clamp(120.0, 400.0);
                        if ui
                            .add(
                                TextEdit::singleline(&mut text)
                                    .desired_width(edit_width)
                                    .font(egui::TextStyle::Monospace),
                            )
                            .on_hover_text(&tooltip)
                            .changed()
                        {
                            picked = Some(serde_json::json!(text.trim()));
                        }
                        if !local_models.is_empty() {
                            // Выбор из библиотеки моделей: в значение
                            // записывается полный путь, показывается имя файла.
                            let selected_name = std::path::Path::new(text.trim())
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            egui::ComboBox::from_id_salt(format!("path-select-{}", def.id))
                                .selected_text(if selected_name.is_empty() {
                                    "из библиотеки…".to_string()
                                } else {
                                    selected_name
                                })
                                .width(120.0)
                                .show_ui(ui, |ui| {
                                    for model in local_models {
                                        let name = model
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        ui.selectable_value(
                                            &mut text,
                                            model.display().to_string(),
                                            name,
                                        )
                                        .on_hover_text(model.display().to_string());
                                    }
                                })
                                .response
                                .on_hover_text(&tooltip);
                            if text != before {
                                picked = Some(serde_json::json!(text.trim()));
                            }
                        }
                        if ui.button("Обзор…").clicked()
                            && let Some(file) = rfd::FileDialog::new()
                                .set_title(format!("Выберите: {}", def.name))
                                .pick_file()
                        {
                            picked = Some(serde_json::json!(file.display().to_string()));
                        }
                        picked
                    }
                    ParamKind::Bool => None,
                };
                if let Some(value) = new_value {
                    state.set_value(&def.id, value);
                }
            });
        }
    }
    before != *state
}

// ---------------------------------------------------------------------------
// Утилиты
// ---------------------------------------------------------------------------

/// Человекочитаемый размер: «245 МБ», «1.4 ГБ».
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * KB;
    const GB: f64 = MB * KB;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} ГБ", bytes / GB)
    } else if bytes >= MB {
        format!("{:.0} МБ", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} КБ", bytes / KB)
    } else {
        format!("{bytes} Б")
    }
}

/// Сокращённый путь для карточек: «…/builds/b10690-vulkan/bin/llama-server».
pub fn short_path(path: &std::path::Path, max_components: usize) -> String {
    let components: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    if components.len() <= max_components {
        return path.display().to_string();
    }
    let tail: Vec<_> = components[components.len() - max_components..].to_vec();
    format!("…/{}", tail.join("/"))
}

/// Категории каталога параметров, сведённые к вкладкам экрана Сервера:
/// (вкладка, [категории]).
pub fn param_tabs() -> [(&'static str, &'static [&'static str]); 4] {
    [
        ("Основные", &["context"]),
        ("GPU и память", &["gpu", "kv"]),
        ("Draft (спекуляция)", &["spec"]),
        ("Дополнительно", &["sampling", "server"]),
    ]
}

/// Открыть папку в системном файловом менеджере.
pub fn open_folder(path: &std::path::Path) {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
    if let Err(e) = result {
        log::warn!("Не удалось открыть папку {}: {e}", path.display());
    }
}

/// Открыть URL в браузере.
pub fn open_url(url: &str) {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd").args(["/C", "start", url]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        log::warn!("Не удалось открыть {url}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_keeps_tail_components() {
        let path =
            std::path::Path::new("/home/user/.local/share/llamacpp-manager/builds/b10690/bin/llama-server");
        assert_eq!(short_path(path, 3), "…/b10690/bin/llama-server");
        assert_eq!(short_path(std::path::Path::new("a/b"), 3), "a/b");
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(594 * 1024 * 1024), "594 МБ");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "2.5 ГБ");
        assert_eq!(format_size(10), "10 Б");
    }
}
