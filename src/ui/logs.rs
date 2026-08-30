//! Экран «Логи»: журналы llama-server и приложения на всё окно.

use egui::{Color32, RichText, ScrollArea};

use crate::app::App;
use crate::logger::LogEntry;
use crate::theme::{self, MUTED};
use crate::ui::{self, level_color, line_color, terminal};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // Журнал llama-server занимает всё доступное место, журнал приложения —
    // фиксированный блок снизу.
    let app_log_height = 210.0;
    let server_height = (ui.available_height() - app_log_height - 16.0).max(120.0);

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Журнал llama-server").size(13.0).strong());
        ui.label(RichText::new("stdout/stderr процесса").size(11.0).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Очистить").clicked()
                && let Ok(mut log) = app.server.server_log().lock()
            {
                log.clear();
            }
            if ui.small_button("Сохранить в файл…").clicked() {
                app.save_server_log();
            }
        });
    });
    ui.add_space(4.0);
    let lines: Vec<(String, Color32)> = app
        .server
        .server_log()
        .lock()
        .map(|log| {
            log.lines()
                .iter()
                .map(|(time, line)| {
                    (
                        format!("{} {}", time.format("%H:%M:%S"), line),
                        line_color(line),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    ScrollArea::vertical()
        .id_salt("server_log_full")
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_min_height(server_height);
            ui.set_min_width(ui.available_width());
            terminal(ui, &lines, "нет вывода процесса — запустите сервер");
        });

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Журнал приложения").size(13.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Открыть папку логов").clicked() {
                ui::open_folder(&app.settings.logs_dir);
            }
            if ui.small_button("Очистить").clicked()
                && let Some(handle) = &app.log_handle
            {
                handle.clear();
            }
            for (i, name) in ["INFO", "WARN", "ERROR", "DEBUG"].iter().enumerate() {
                let color = match i {
                    1 => theme::WARN_YELLOW,
                    2 => theme::ERR_RED,
                    _ => MUTED,
                };
                let on = &mut app.log_filter[i];
                ui.toggle_value(
                    on,
                    RichText::new(*name).size(11.0).strong().color(if *on { color } else { MUTED }),
                );
            }
        });
    });
    ui.add_space(4.0);
    let Some(handle) = app.log_handle.clone() else {
        return;
    };
    let filter = app.log_filter;
    let entries: Vec<LogEntry> = handle
        .snapshot()
        .into_iter()
        .filter(|entry| match entry.level {
            log::Level::Info => filter[0],
            log::Level::Warn => filter[1],
            log::Level::Error => filter[2],
            log::Level::Debug | log::Level::Trace => filter[3],
        })
        .collect();
    let colored: Vec<(String, Color32)> = entries
        .iter()
        .map(|entry| {
            (
                format!(
                    "{} {:<5} {}",
                    entry.time.format("%H:%M:%S"),
                    entry.level,
                    entry.message
                ),
                level_color(entry.level),
            )
        })
        .collect();
    ScrollArea::vertical()
        .id_salt("app_log_full")
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_min_height(app_log_height - 8.0);
            ui.set_min_width(ui.available_width());
            terminal(ui, &colored, "журнал пуст");
        });
}
