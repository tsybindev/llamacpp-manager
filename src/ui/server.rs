//! Экран «Сервер»: hero-статус, пресеты, конфигурация, вкладки параметров,
//! CLI-предпросмотр и журналы.

use egui::{Color32, RichText};

use crate::app::App;
use crate::params;
use crate::process_mgr::ServerState;
use crate::theme::{self, MUTED};
use crate::ui::{
    self, cli_preview, card, card_titled, format_size, gauge_color, memory_pool, param_tabs,
    param_row, path_row, state_status, status_dot, PathPick, GaugeSegment, MemPool,
};

const KV_CACHE_COLOR: Color32 = Color32::from_rgb(0x81, 0x8C, 0xF8);
const WEIGHTS_COLOR: Color32 = Color32::from_rgb(0x60, 0xA5, 0xFA);

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    hero_card(app, ui);
    ui.add_space(10.0);
    presets_toolbar(app, ui);
    ui.add_space(10.0);
    config_card(app, ui);
    ui.add_space(10.0);
    params_card(app, ui);
    ui.add_space(10.0);
    cli_card(app, ui);
}

/// Верхний статус-баннер: крупный статус + адрес и быстрые действия.
fn hero_card(app: &mut App, ui: &mut egui::Ui) {
    let state = app.server.state();
    let (color, hint) = state_status(state);
    let error = app.server_form.last_error.clone();
    let ready = state == ServerState::Ready;
    let url = format!("http://{}:{}", app.server_form.host.trim(), app.server_form.port);

    card(ui, |ui| {
        ui.horizontal(|ui| {
            status_dot(ui, color, 5.0);
            ui.label(RichText::new(state.label()).size(17.0).strong());
            ui.label(RichText::new(hint).size(13.0).color(MUTED));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ready && ui.button("Веб-интерфейс").clicked() {
                    ui::open_url(&url);
                }
                if ready
                    && ui.button("Копировать URL").on_hover_text("Скопировать адрес API").clicked()
                {
                    ui.ctx().copy_text(url.clone());
                }
                if ready {
                    ui.label(RichText::new(&url).monospace().size(13.0).color(MUTED));
                }
            });
        });
        if let Some(error) = error {
            ui.label(RichText::new(error).size(13.0).color(theme::ERR_RED));
        }
    });
}

/// Компактный тулбар пресетов: выбор + все операции в одном-двух рядах.
fn presets_toolbar(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Пресеты", |ui| {
        app.sync_preset_selection();

        let names = app.presets.list();
        if let Some(sel) = app.selected_preset.as_ref()
            && !names.contains(sel)
        {
            app.selected_preset = None;
            app.preset_delete_armed = false;
        }
        let selection = app.selected_preset.clone();
        let has_selection = selection.is_some();
        let typed_name = app.preset_name_edit.trim().to_string();

        ui.horizontal(|ui| {
            let label = selection.clone().unwrap_or_else(|| "— выберите пресет —".into());
            egui::ComboBox::from_id_salt("preset-select")
                .selected_text(label)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for name in &names {
                        ui.selectable_value(&mut app.selected_preset, Some(name.clone()), name.clone());
                    }
                });
            if ui
                .add_enabled(has_selection, egui::Button::new("Сохранить"))
                .on_hover_text("Сохранить текущую конфигурацию в выбранный пресет")
                .clicked()
            {
                app.save_into_selected_preset();
            }
            let name_differs = has_selection && selection.as_deref() != Some(typed_name.as_str());
            if ui
                .add_enabled(name_differs, egui::Button::new("Переименовать"))
                .on_hover_text(format!(
                    "Переименовать «{}» в «{typed_name}»",
                    selection.as_deref().unwrap_or("")
                ))
                .clicked()
            {
                app.preset_delete_armed = false;
                app.rename_selected_preset();
            }
            if app.preset_delete_armed && has_selection {
                let name = selection.clone().unwrap_or_default();
                match ui::delete_confirm(ui, Some(&format!("Пресет «{name}» будет удалён с диска"))) {
                    Some(true) => {
                        app.delete_selected_preset();
                        app.preset_delete_armed = false;
                    }
                    Some(false) => app.preset_delete_armed = false,
                    None => {}
                }
            } else if ui
                .add_enabled(has_selection, egui::Button::new("Удалить"))
                .on_hover_text("Удалить выбранный пресет с диска")
                .clicked()
            {
                app.preset_delete_armed = true;
            }
            if ui.button("Экспорт…").on_hover_text("Экспорт пресета в JSON-файл").clicked() {
                app.export_selected_preset();
            }
            if ui.button("Импорт…").on_hover_text("Импорт пресета из JSON-файла").clicked() {
                app.import_preset_file();
            }
            if has_selection {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("правки автосохраняются в пресет").size(11.0).color(MUTED),
                    );
                });
            }
        });

        ui.horizontal(|ui| {
            ui.add_sized([60.0, 18.0], egui::Label::new(RichText::new("Имя пресета").size(13.0).color(MUTED)));
            ui.add(
                egui::TextEdit::singleline(&mut app.preset_name_edit)
                    .desired_width(220.0)
                    .hint_text("например: gemma-mtp-локально"),
            );
            if ui
                .add_enabled(!typed_name.is_empty(), egui::Button::new("+ Новый"))
                .on_hover_text("Сохранить текущую конфигурацию под введённым именем")
                .clicked()
            {
                app.save_current_as_preset();
            }
        });

        if let Some((is_error, message)) = &app.preset_msg {
            let color = if *is_error { theme::ERR_RED } else { theme::OK_GREEN };
            ui.label(RichText::new(message).size(12.0).color(color));
        }
    });
}

/// Конфигурация: 2 колонки (бинарник+модель | host+port) и оценка памяти.
fn config_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Конфигурация", |ui| {
        let mut form_changed = false;
        ui.columns(2, |columns| {
            columns[0].vertical(|ui| {
                form_changed |= path_row(
                    ui,
                    "Бинарник",
                    &mut app.server_form.binary,
                    "путь к llama-server",
                    PathPick::File,
                );
                form_changed |= path_row(
                    ui,
                    "Модель",
                    &mut app.server_form.model,
                    "путь к GGUF-файлу модели",
                    PathPick::File,
                );
            });
            columns[1].vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([ui::LABEL_WIDTH, 18.0], egui::Label::new(RichText::new("Host").size(13.0)));
                    form_changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut app.server_form.host)
                                .desired_width(160.0)
                                .hint_text("127.0.0.1"),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.add_sized([ui::LABEL_WIDTH, 18.0], egui::Label::new(RichText::new("Порт").size(13.0)));
                    form_changed |= ui
                        .add(
                            egui::DragValue::new(&mut app.server_form.port)
                                .range(1..=65535)
                                .custom_formatter(|v, _| format!("{}", v as u16))
                                .custom_parser(|s| s.parse::<u16>().ok().map(f64::from)),
                        )
                        .changed();
                });
                if app.server_form.host.trim() == "0.0.0.0" {
                    ui.label(
                        RichText::new("⚠ Host 0.0.0.0 доступен из сети").size(12.0).color(theme::WARN_YELLOW),
                    );
                }
            });
        });
        if form_changed {
            app.mark_dirty();
        }

        ui.add_space(6.0);
        memory_section(app, ui);
    });
}

/// Метаданные GGUF и цветной гейдж памяти.
fn memory_section(app: &mut App, ui: &mut egui::Ui) {
    let Some((path, info)) = app.cached_gguf_info() else {
        return;
    };
    let Some(info) = info else {
        if path.is_file() {
            ui.label(
                RichText::new("Не удалось прочитать GGUF-заголовок — оценка памяти недоступна")
                    .size(12.0)
                    .color(theme::WARN_YELLOW),
            );
        }
        return;
    };

    let quant = info
        .file_type
        .and_then(crate::gguf::file_type_label)
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .and_then(crate::huggingface::quant_from_filename)
        });
    let mut meta = format!(
        "{} · {} слоёв{}",
        info.arch.as_deref().unwrap_or("gguf"),
        info.n_layers.map_or("?".into(), |l| l.to_string()),
        quant.as_deref().map_or(String::new(), |q| format!(" · {q}"))
    );
    if let Some(train_ctx) = info.ctx_train {
        meta.push_str(&format!(" · контекст до {train_ctx}"));
    }
    ui.horizontal(|ui| {
        if let Some(q) = &quant {
            ui::badge(ui, q.as_str(), ui::ACCENT_BADGE);
        }
        ui.label(RichText::new(meta).size(12.0).color(MUTED));
    });

    let Some((_, estimate)) = app.memory_estimate() else {
        return;
    };

    // VRAM-пул знаменатель = VRAM (если определена), ОЗУ-пул — MemAvailable.
    let vram = app.vram_total();
    let (_ram_total, ram_available) = ui::mem_info().unwrap_or((0, 0));
    let ram_pool = MemPool {
        title: "ОЗУ (RAM)".into(),
        total: ram_available,
        segments: vec![
            GaugeSegment {
                label: "Веса".into(),
                bytes: estimate.weights_cpu,
                color: WEIGHTS_COLOR,
            },
            GaugeSegment {
                label: "KV-кэш".into(),
                bytes: estimate.kv_cpu,
                color: KV_CACHE_COLOR,
            },
        ],
    };

    match vram {
        Some(v) => {
            let vram_pool = MemPool {
                title: "VRAM".into(),
                total: v,
                segments: vec![
                    GaugeSegment {
                        label: "Веса".into(),
                        bytes: estimate.weights_gpu,
                        color: WEIGHTS_COLOR,
                    },
                    GaugeSegment {
                        label: "KV-кэш".into(),
                        bytes: estimate.kv_gpu,
                        color: KV_CACHE_COLOR,
                    },
                ],
            };
            memory_pool(ui, &vram_pool);
            ui.add_space(10.0);
            memory_pool(ui, &ram_pool);
        }
        None => {
            // VRAM не определили: честно показываем только ОЗУ.
            ui.label(
                RichText::new("VRAM: не определена — показана только ОЗУ")
                    .size(12.0)
                    .color(theme::WARN_YELLOW),
            );
            ui.add_space(6.0);
            memory_pool(ui, &ram_pool);
        }
    }

    // Итог «≈ Y VRAM / Z RAM» + пометка об offload; цвет — по худшему пулу.
    let vram_fill = vram.map_or(0.0, |v| {
        if v > 0 {
            (estimate.gpu_bytes as f32 / v as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    });
    let ram_fill = if ram_available > 0 {
        (estimate.cpu_bytes as f32 / ram_available as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let note = if estimate.gpu_bytes >= estimate.weights_bytes + estimate.kv_cache_bytes {
        "вся модель на GPU"
    } else if estimate.gpu_bytes > 0 {
        "частично на GPU"
    } else {
        "только CPU/RAM"
    };
    ui.label(
        RichText::new(format!(
            "≈ {} VRAM / {} RAM · {note} · приблизительная оценка",
            format_size(estimate.gpu_bytes),
            format_size(estimate.cpu_bytes),
        ))
        .size(11.0)
        .color(gauge_color(vram_fill.max(ram_fill))),
    );
}

/// Параметры запуска во вкладках (вместо гармошек).
fn params_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Параметры llama-server", |ui| {
        let tabs = param_tabs();
        let known_categories: std::collections::BTreeSet<&str> =
            tabs.iter().flat_map(|(_, cats)| cats.iter().copied()).collect();
        ui.horizontal(|ui| {
            ui.style_mut().spacing.item_spacing.x = 6.0;
            for (i, (label, _)) in tabs.iter().enumerate() {
                if ui
                    .selectable_label(app.params_tab == i, RichText::new(*label).size(13.0))
                    .clicked()
                {
                    app.params_tab = i;
                }
            }
        });
        ui.add_space(4.0);

        let tab = app.params_tab.min(tabs.len() - 1);
        let cats = tabs[tab].1;
        // Клонируем определения, чтобы не конфликтовать с &mut params.
        let defs: Vec<crate::params::ParamDef> = app
            .params_catalog
            .params
            .iter()
            .filter(|p| {
                cats.contains(&p.category.as_str())
                    || (tab == tabs.len() - 1 && !known_categories.contains(p.category.as_str()))
            })
            .cloned()
            .collect();

        let local_models = crate::app::library_models(&app.settings.models_dir);
        // Максимально допустимый контекст — из GGUF-заголовка загруженной модели.
        let model_ctx = app
            .cached_gguf_info()
            .and_then(|(_, info)| info)
            .and_then(|info| info.ctx_train);
        let mut params_changed = false;
        for def in &defs {
            params_changed |= param_row(ui, def, &mut app.settings.params, &local_models, model_ctx);
        }
        if params_changed {
            app.mark_dirty();
        }

        let problems = params::validate(&app.params_catalog, &app.settings.params);
        if !problems.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "⚠ Проверка параметров:\n{}",
                    problems.iter().map(|p| format!("• {p}")).collect::<Vec<_>>().join("\n")
                ))
                .size(12.0)
                .color(theme::WARN_YELLOW),
            );
        }
    });
}

/// Блок предпросмотра команды запуска.
fn cli_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, |ui| {
        let command = app
            .server_form
            .to_config(app.build_extra_args())
            .command_line();
        cli_preview(ui, &command);
    });
}
