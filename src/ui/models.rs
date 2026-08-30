//! Экран «Модели»: локальная библиотека (таблица) и поиск HuggingFace.

use egui::{RichText, ScrollArea};
use egui_extras::{Column, TableBuilder};

use crate::app::{library_models, App};
use crate::gguf;
use crate::theme::{self, MUTED};
use crate::ui::{self, badge, card_titled, format_size, short_path};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    app.poll_hf_search();
    app.poll_hf_files();
    app.poll_model_download();

    library_card(app, ui);
    ui.add_space(10.0);
    search_card(app, ui);
    if app.hf_selected_repo.is_some() {
        ui.add_space(10.0);
        files_card(app, ui);
    }
}

/// Локальная библиотека моделей в виде таблицы.
fn library_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Локальная библиотека", |ui| {
        ui.label(
            RichText::new(format!("Каталог: {}", app.settings.models_dir.display()))
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(4.0);

        let models = library_models(&app.settings.models_dir);
        if models.is_empty() {
            ui.label(
                RichText::new(
                    "Пока нет скачанных моделей — найдите и скачайте ниже, либо укажите путь к .gguf вручную на странице «Сервер».",
                )
                .size(13.0)
                .color(MUTED),
            );
            return;
        }

        let mut activate: Option<std::path::PathBuf> = None;
        let mut delete: Option<std::path::PathBuf> = None;
        TableBuilder::new(ui)
            .id_salt("models-library")
            .striped(true)
            .vscroll(false)
            .column(Column::initial(280.0).resizable(true).clip(true))
            .column(Column::auto().at_least(70.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(110.0))
            .column(Column::remainder())
            .header(22.0, |mut header| {
                for name in ["Имя файла", "Размер", "Квант", "Архитектура", "Действия"] {
                    header.col(|ui| {
                        ui.label(RichText::new(name).size(12.0).strong().color(MUTED));
                    });
                }
            })
            .body(|mut body| {
                for path in models {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string();
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let info = app.gguf_info_for(&path);
                    let quant = quant_label(&path, info.as_ref());
                    let arch = arch_label(info.as_ref());
                    let is_active = app.server_form.model == path;
                    body.row(26.0, |mut row| {
                        row.col(|ui| {
                            ui.label(
                                RichText::new(&name)
                                    .monospace()
                                    .size(12.0)
                                    .color(if is_active { theme::OK_GREEN } else { ui.visuals().text_color() }),
                            )
                            .on_hover_text(path.display().to_string());
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(format_size(size)).size(12.0).color(MUTED));
                        });
                        row.col(|ui| {
                            if let Some(q) = &quant {
                                badge(ui, q, ui::ACCENT_BADGE);
                            }
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(arch).size(12.0).color(MUTED));
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(!is_active, egui::Button::new("Выбрать для сервера"))
                                    .on_hover_text("Использовать эту модель при запуске llama-server")
                                    .clicked()
                                {
                                    activate = Some(path.clone());
                                }
                                let armed = app.model_delete_armed.as_deref() == Some(path.as_path());
                                let label = if armed { "Точно удалить?" } else { "Удалить" };
                                if ui
                                    .button(label)
                                    .on_hover_text(format!("Удалить файл с диска\n{}", path.display()))
                                    .clicked()
                                {
                                    if armed {
                                        delete = Some(path.clone());
                                    } else {
                                        app.model_delete_armed = Some(path.clone());
                                    }
                                }
                                if armed {
                                    let affected = app.presets_using_model(&path);
                                    if !affected.is_empty() {
                                        ui.label(
                                            RichText::new(format!("⚠ используется в пресетах: {}", affected.join(", ")))
                                                .size(11.0)
                                                .color(theme::WARN_YELLOW),
                                        );
                                    }
                                }
                            });
                        });
                    });
                }
            });
        if let Some(path) = activate {
            app.server_form.model = path.clone();
            app.mark_dirty();
            log::info!("Активная модель: {}", path.display());
        }
        if let Some(path) = delete {
            app.delete_model_file(&path);
            app.model_delete_armed = None;
        }
    });
}

/// Строка поиска HuggingFace + таблица результатов.
fn search_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Поиск на HuggingFace", |ui| {
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut app.hf_query)
                    .desired_width(340.0)
                    .hint_text("например: gemma 3n gguf"),
            );
            let enter_pressed =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(!app.hf_searching, egui::Button::new("Найти"))
                .clicked()
                || enter_pressed
            {
                app.start_hf_search();
            }
            if app.hf_searching {
                ui.add(egui::Spinner::new().size(16.0));
            }
            if let Some(error) = &app.hf_error {
                ui.label(RichText::new(error).size(12.0).color(theme::ERR_RED));
            }
        });

        if let Some(results) = app.hf_results.clone() {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Найдено: {}", results.len())).size(12.0).color(MUTED));
            let mut pick: Option<String> = None;
            ScrollArea::vertical()
                .id_salt("hf-results")
                .max_height(200.0)
                .show(ui, |ui| {
                    TableBuilder::new(ui)
                        .id_salt("hf-results-table")
                        .striped(true)
                        .vscroll(false)
                        .column(Column::remainder().clip(true))
                        .column(Column::auto().at_least(80.0))
                        .column(Column::auto().at_least(60.0))
                        .header(22.0, |mut header| {
                            for name in ["Репозиторий", "Скачиваний", "Звёзд"] {
                                header.col(|ui| {
                                    ui.label(RichText::new(name).size(12.0).strong().color(MUTED));
                                });
                            }
                        })
                        .body(|mut body| {
                            for model in &results {
                                let selected =
                                    app.hf_selected_repo.as_deref() == Some(model.id.as_str());
                                body.row(26.0, |mut row| {
                                    row.col(|ui| {
                                        if ui
                                            .selectable_label(selected, RichText::new(&model.id).monospace().size(12.0))
                                            .on_hover_text("Показать файлы модели")
                                            .clicked()
                                        {
                                            pick = Some(model.id.clone());
                                        }
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(model.downloads.to_string()).size(12.0).color(MUTED));
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(model.likes.to_string()).size(12.0).color(MUTED));
                                    });
                                });
                            }
                        });
                });
            if let Some(repo) = pick {
                app.select_hf_repo(repo);
            }
        }
    });
}

/// Таблица файлов выбранного репозитория со статусами и прогрессом.
fn files_card(app: &mut App, ui: &mut egui::Ui) {
    let repo = app.hf_selected_repo.clone().unwrap_or_default();
    card_titled(ui, &format!("Файлы {repo}"), |ui| {
        if app.hf_files_rx.is_some() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new("Получение списка файлов…").size(13.0).color(MUTED));
            });
            return;
        }
        let files = app.hf_files.clone();
        if files.is_empty() {
            ui.label(RichText::new("GGUF-файлов в репозитории не найдено").size(13.0).color(MUTED));
            return;
        }

        let mut start_download: Option<crate::huggingface::HfFile> = None;
        let mut clear_download = false;
        let models_dir = app.settings.models_dir.clone();
        let busy = app.model_download.is_some();
        TableBuilder::new(ui)
            .id_salt("hf-files")
            .striped(true)
            .vscroll(false)
            .column(Column::initial(300.0).resizable(true).clip(true))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(70.0))
            .column(Column::remainder())
            .column(Column::auto().at_least(90.0))
            .header(22.0, |mut header| {
                for name in ["Файл", "Квант", "Размер", "Статус / Прогресс", "Действие"] {
                    header.col(|ui| {
                        ui.label(RichText::new(name).size(12.0).strong().color(MUTED));
                    });
                }
            })
            .body(|mut body| {
                for file in &files {
                    let dest = models_dir.join(&file.path);
                    let downloaded = dest.is_file();
                    let quant = crate::huggingface::quant_from_filename(&file.path);
                    let downloading = app.is_downloading_model(&file.path);
                    body.row(26.0, |mut row| {
                        row.col(|ui| {
                            ui.label(RichText::new(&file.path).monospace().size(12.0))
                                .on_hover_text(short_path(&dest, 4));
                        });
                        row.col(|ui| {
                            if let Some(q) = &quant {
                                badge(ui, q.as_str(), ui::ACCENT_BADGE);
                            }
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(format_size(file.size)).size(12.0).color(MUTED));
                        });
                        row.col(|ui| {
                            if downloading
                                && let Some(download) = app.model_download.as_ref()
                            {
                                let fraction = if download.total > 0 {
                                    download.downloaded as f32 / download.total as f32
                                } else {
                                    0.0
                                };
                                ui.add(
                                    egui::ProgressBar::new(fraction)
                                        .show_percentage()
                                        .desired_width(160.0),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} из {}",
                                        format_size(download.downloaded),
                                        if download.total > 0 {
                                            format_size(download.total)
                                        } else {
                                            "?".into()
                                        }
                                    ))
                                    .size(11.0)
                                    .color(MUTED),
                                );
                            } else if downloaded {
                                badge(ui, "скачана", theme::OK_GREEN);
                            } else if let Some(download) = app.model_download.as_ref()
                                && download.error.is_some()
                                && download.path == file.path
                            {
                                ui.label(
                                    RichText::new(download.error.clone().unwrap_or_default())
                                        .size(11.0)
                                        .color(theme::ERR_RED),
                                );
                            } else {
                                ui.label(RichText::new("Готов к загрузке").size(12.0).color(MUTED));
                            }
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if downloading {
                                    if ui
                                        .button("Отменить")
                                        .on_hover_text("Остановить скачивание (частичный файл сохранится и докачается позже)")
                                        .clicked()
                                    {
                                        app.cancel_model_download();
                                    }
                                } else if let Some(download) = app.model_download.as_ref()
                                    && download.error.is_some()
                                    && download.path == file.path
                                    && ui.small_button("Скрыть").clicked()
                                {
                                    clear_download = true;
                                } else if ui
                                    .add_enabled(!busy, egui::Button::new(if downloaded { "Перекачать" } else { "Скачать" }))
                                    .on_hover_text(format!(
                                        "{} в {}\n{}",
                                        if downloaded { "Скачать заново" } else { "Скачать" },
                                        models_dir.display(),
                                        file.path
                                    ))
                                    .clicked()
                                {
                                    start_download = Some(file.clone());
                                }
                            });
                        });
                    });
                }
            });
        if let Some(file) = start_download {
            app.start_model_download(repo.clone(), file);
        }
        if clear_download {
            app.model_download = None;
        }
    });
}

fn quant_label(path: &std::path::Path, info: Option<&gguf::GgufInfo>) -> Option<String> {
    info.and_then(|i| i.file_type)
        .and_then(gguf::file_type_label)
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .and_then(crate::huggingface::quant_from_filename)
        })
}

fn arch_label(info: Option<&gguf::GgufInfo>) -> String {
    match info {
        Some(info) => match (info.arch.as_deref(), info.n_layers) {
            (Some(arch), Some(layers)) => format!("{arch} · {layers}L"),
            (Some(arch), None) => arch.to_string(),
            (None, Some(layers)) => format!("gguf · {layers}L"),
            (None, None) => "gguf".into(),
        },
        None => "—".into(),
    }
}
