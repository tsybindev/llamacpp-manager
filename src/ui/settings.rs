//! Экран «Настройки»: карточки с фиксированной шириной меток.

use egui::{RichText, Slider};

use crate::app::App;
use crate::theme::{self, MUTED, ThemeMode};
use crate::ui::{self, card_titled, path_row, PathPick};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    status_row(app, ui);
    ui.add_space(10.0);
    paths_card(app, ui);
    ui.add_space(10.0);
    hf_card(app, ui);
    ui.add_space(10.0);
    auto_restore_card(app, ui);
    ui.add_space(10.0);
    interface_card(app, ui);
}

/// Верхняя строка: индикатор автосохранения и ручное сохранение.
fn status_row(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Все настройки сохраняются автоматически.").size(13.0).color(MUTED));
        if app.config_dirty {
            ui.label(RichText::new("● не сохранено").size(13.0).color(theme::WARN_YELLOW));
        }
        if ui.button("Сохранить сейчас").clicked() {
            app.save_settings();
        }
    });
}

/// Пути и директории.
fn paths_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Пути и директории", |ui| {
        let changed = {
            let s = &mut app.settings;
            path_row(ui, "Каталог моделей", &mut s.models_dir, "куда скачиваются GGUF-модели", PathPick::Folder)
                | path_row(ui, "Каталог сборок", &mut s.builds_dir, "куда скачиваются бинарники llama.cpp", PathPick::Folder)
                | path_row(ui, "Каталог логов", &mut s.logs_dir, "куда пишутся журналы", PathPick::Folder)
        };
        if changed {
            app.mark_dirty();
        }
    });
}

/// Авторизация HuggingFace: маскированный токен с показом/скрытием.
fn hf_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Авторизация HuggingFace", |ui| {
        ui.horizontal(|ui| {
            ui.add_sized([ui::LABEL_WIDTH, 18.0], egui::Label::new(RichText::new("Токен доступа").size(13.0)));
            let mut token = app.settings.hf_token.clone();
            let response = ui.add(
                egui::TextEdit::singleline(&mut token)
                    .password(!app.hf_show_token)
                    .hint_text("hf_...")
                    .desired_width(380.0),
            );
            if response.changed() {
                app.settings.hf_token = token.trim().to_string();
                app.mark_dirty();
            }
            let toggle_label = if app.hf_show_token { "Скрыть" } else { "Показать" };
            if ui.button(toggle_label).clicked() {
                app.hf_show_token = !app.hf_show_token;
            }
        });
        ui.label(
            RichText::new("⚠ Токен хранится в конфиг-файле в открытом виде. Нужен только для приватных/gated-моделей.")
                .size(12.0)
                .color(theme::WARN_YELLOW),
        );
    });
}

/// Автовосстановление сервера: toggle + параметры в одну строку.
fn auto_restore_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Автовосстановление сервера", |ui| {
        let mut ar = app.settings.auto_restore.clone();
        ui.add(
            egui::Checkbox::new(&mut ar.enabled, RichText::new("Автоматически перезапускать llama-server при падении").size(13.0)),
        );
        ui.add_enabled_ui(ar.enabled, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_sized([ui::LABEL_WIDTH, 18.0], egui::Label::new(RichText::new("Макс. попыток").size(13.0)));
                ui.add(egui::DragValue::new(&mut ar.max_restarts).range(1..=20).suffix(" раз"));
                ui.add_space(16.0);
                ui.add_sized([110.0, 18.0], egui::Label::new(RichText::new("Окно времени").size(13.0)));
                ui.add(egui::DragValue::new(&mut ar.window_secs).range(30..=3600).speed(10.0).suffix(" сек"));
                ui.add_space(16.0);
                ui.add_sized([70.0, 18.0], egui::Label::new(RichText::new("Backoff").size(13.0)));
                ui.add(egui::DragValue::new(&mut ar.backoff_start_secs).range(1..=60).suffix(" сек"));
            });
            ui.label(
                RichText::new("Задержка удваивается после каждой попытки. При исчерпании лимита сервер остаётся в состоянии «упал» и требуется вмешательство.")
                    .size(12.0)
                    .color(MUTED),
            );
        });
        if ar != app.settings.auto_restore {
            app.server.set_auto_restore(ar.clone());
            app.settings.auto_restore = ar;
            app.mark_dirty();
        }
    });
}

/// Интерфейс: тема, масштаб, debug-логирование.
fn interface_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Интерфейс", |ui| {
        egui::Grid::new("interface_grid")
            .num_columns(2)
            .spacing([16.0, 12.0])
            .min_col_width(ui::LABEL_WIDTH)
            .show(ui, |ui| {
                ui.label(RichText::new("Тема").size(13.0));
                ui.horizontal(|ui| {
                    for mode in ThemeMode::ALL {
                        if ui
                            .selectable_label(app.settings.theme == mode, mode.label())
                            .clicked()
                        {
                            app.settings.theme = mode;
                            app.mark_dirty();
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Масштаб интерфейса").size(13.0));
                let mut zoom = app.settings.ui_zoom;
                if ui.add(Slider::new(&mut zoom, 0.8..=1.5).text("x").step_by(0.05)).changed() {
                    app.settings.ui_zoom = zoom;
                    app.mark_dirty();
                }
                ui.end_row();

                ui.label(RichText::new("Debug-логирование").size(13.0));
                let mut debug = app.settings.debug_logging;
                if ui
                    .checkbox(&mut debug, "подробные сообщения в журнал")
                    .changed()
                {
                    app.settings.debug_logging = debug;
                    if let Some(handle) = &app.log_handle {
                        handle.set_debug(debug);
                    }
                    app.mark_dirty();
                }
                ui.end_row();

                ui.label(RichText::new("Файл журнала").size(13.0));
                ui.label(
                    RichText::new(app.settings.logs_dir.join("manager.log").display().to_string())
                        .monospace()
                        .size(12.0)
                        .color(MUTED),
                );
                ui.end_row();
            });
    });
}
