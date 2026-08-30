//! Экран «Сборки»: установленные сборки карточками + релизы GitHub
//! с чипами фильтров бэкендов и аккордеоном по тегам.

use egui::{RichText, ScrollArea};
use egui_extras::{Column, TableBuilder};

use crate::app::App;
use crate::builds;
use crate::github;
use crate::theme::{self, MUTED};
use crate::ui::{self, backend_badge, badge, card_titled, format_size, short_path};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    app.poll_builds_refresh();
    app.poll_build_download();
    // Список релизов лениво подгружается при первом открытии страницы.
    if app.build_releases.is_none()
        && app.build_releases_rx.is_none()
        && !app.build_releases_loading
        && app.build_releases_error.is_none()
    {
        app.start_builds_refresh(false);
    }

    installed_card(app, ui);
    ui.add_space(10.0);
    releases_card(app, ui);
}

/// Установленные сборки: компактные карточки-строки.
fn installed_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Установленные сборки", |ui| {
        let store = builds::BuildsStore::new(app.settings.builds_dir.clone());
        let installed = store.installed();
        if installed.is_empty() {
            ui.label(
                RichText::new(
                    "Сборок пока нет. Скачайте релиз ниже — бинарник llama-server попадёт в библиотеку сборок.",
                )
                .size(13.0)
                .color(MUTED),
            );
            return;
        }

        let mut activate: Option<(String, std::path::PathBuf)> = None;
        let mut delete: Option<builds::InstalledBuild> = None;
        for build in installed {
            let binary = build.server_binary();
            let is_active = binary.as_ref() == Some(&app.server_form.binary);
            ui::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&build.tag).monospace().size(14.0).strong());
                    backend_badge(ui, build.backend);
                    if is_active {
                        badge(ui, "Активна", theme::OK_GREEN);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let armed = app.build_delete_armed.as_deref() == Some(build.dir.as_path());
                        let label = if armed { "Точно удалить?" } else { "Удалить" };
                        let response = ui
                            .button(label)
                            .on_hover_text(format!("Удалить каталог сборки\n{}", build.dir.display()));
                        let mut warn = String::new();
                        if armed {
                            let affected = app.presets_using_dir(&build.dir);
                            if !affected.is_empty() {
                                warn = format!(
                                    "используется в пресетах: {} — их настройка бинарника станет нерабочей",
                                    affected.join(", ")
                                );
                            }
                        }
                        if response.clicked() {
                            if armed {
                                delete = Some(build.clone());
                            } else {
                                app.build_delete_armed = Some(build.dir.clone());
                            }
                        }
                        if ui
                            .add_enabled(binary.is_some() && !is_active, egui::Button::new("Сделать активной"))
                            .on_hover_text("Использовать llama-server из этой сборки при запуске")
                            .clicked()
                            && let Some(bin) = &binary
                        {
                            activate = Some((build.tag.clone(), bin.clone()));
                        }
                        if !warn.is_empty() {
                            ui.label(RichText::new(format!("⚠ {warn}")).size(11.0).color(theme::WARN_YELLOW));
                        }
                    });
                });
                match &binary {
                    Some(bin) => {
                        ui.label(
                            RichText::new(format!("📂 {}", short_path(bin, 4)))
                                .monospace()
                                .size(11.0)
                                .color(MUTED),
                        )
                        .on_hover_text(bin.display().to_string());
                    }
                    None => {
                        ui.label(
                            RichText::new("⚠ llama-server не найден в каталоге сборки")
                                .size(11.0)
                                .color(theme::WARN_YELLOW),
                        );
                    }
                }
            });
            ui.add_space(4.0);
        }
        if let Some((tag, bin)) = activate {
            app.activate_build_binary(tag, bin);
        }
        if let Some(build) = delete {
            app.delete_build(&build);
            app.build_delete_armed = None;
        }
    });
}

/// Доступные релизы: чипы бэкендов + аккордеон по тегам.
fn releases_card(app: &mut App, ui: &mut egui::Ui) {
    card_titled(ui, "Доступные релизы (GitHub)", |ui| {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!app.build_releases_loading, egui::Button::new("⟳ Обновить"))
                .clicked()
            {
                app.start_builds_refresh(true);
            }
            if app.build_releases_loading {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new("Загрузка списка релизов…").size(13.0).color(MUTED));
            }
            if let Some(error) = &app.build_releases_error {
                ui.label(RichText::new(error).size(12.0).color(theme::ERR_RED));
            }
        });

        // Чипы фильтров бэкендов.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.style_mut().spacing.item_spacing.x = 6.0;
            let chips: [(Option<github::Backend>, &str); 6] = [
                (None, "Все"),
                (Some(github::Backend::Vulkan), "Vulkan"),
                (Some(github::Backend::Cuda), "CUDA"),
                (Some(github::Backend::Cpu), "CPU"),
                (Some(github::Backend::Rocm), "ROCm"),
                (Some(github::Backend::Sycl), "SYCL"),
            ];
            for (backend, label) in chips {
                let selected = app.build_backend_filter == backend;
                if ui
                    .selectable_label(selected, RichText::new(label).size(12.5))
                    .clicked()
                {
                    app.build_backend_filter = backend;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let toggle_label = if app.build_show_all {
                    "Показать 5 последних"
                } else {
                    "Показать все"
                };
                if ui.small_button(toggle_label).clicked() {
                    app.build_show_all = !app.build_show_all;
                }
                if !app.build_show_all {
                    let hidden =
                        app.total_filtered_tags().saturating_sub(app.visible_release_count());
                    if hidden > 0 {
                        ui.label(RichText::new(format!("ещё {hidden} релизов")).size(11.0).color(MUTED));
                    }
                }
            });
        });

        ui.add_space(6.0);
        download_panel(app, ui);

        let store = builds::BuildsStore::new(app.settings.builds_dir.clone());
        let assets = app.visible_os_assets();
        if assets.is_empty() && !app.build_releases_loading && app.build_releases_error.is_none() {
            ui.label(
                RichText::new(
                    "Для вашей платформы сборки не определились автоматически — выберите файл вручную в списке ниже.",
                )
                .size(13.0)
                .color(theme::WARN_YELLOW),
            );
        }
        ScrollArea::vertical().show(ui, |ui| {
            let mut last_tag = String::new();
            for asset in &assets {
                if asset.tag != last_tag {
                    last_tag = asset.tag.clone();
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new(
                        RichText::new(&asset.tag).monospace().size(14.0).strong(),
                    )
                    .default_open(asset.tag == first_visible_tag(&assets))
                    .show(ui, |ui| {
                        assets_table(app, ui, &store, &assets.iter().filter(|a| a.tag == last_tag).cloned().collect::<Vec<_>>());
                    });
                }
            }

            let manual: Vec<_> = app
                .manual_assets(&assets)
                .into_iter()
                .filter(|(tag, asset)| {
                    let backend = github::classify_asset(&asset.name)
                        .map(|kind| kind.backend)
                        .unwrap_or(github::Backend::Other);
                    app.asset_matches_filter(tag, backend)
                })
                .collect();
            if !manual.is_empty() {
                ui.add_space(8.0);
                egui::CollapsingHeader::new(RichText::new("Другие файлы релизов (выбор вручную)").size(13.0))
                    .default_open(assets.is_empty())
                    .show(ui, |ui| {
                        for (tag, asset) in manual {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&asset.name).monospace().size(11.0));
                                ui.label(RichText::new(format_size(asset.size)).size(11.0).color(MUTED));
                                if ui
                                    .add_enabled(!app.is_downloading(&asset.name), egui::Button::new("⬇ Скачать").small())
                                    .on_hover_text(format!("Релиз {tag}\n{}", asset.browser_download_url))
                                    .clicked()
                                {
                                    let kind = github::classify_asset(&asset.name);
                                    app.start_build_download(github::BuildAsset {
                                        asset: asset.clone(),
                                        tag,
                                        os: kind.and_then(|k| k.os),
                                        arch: kind.and_then(|k| k.arch),
                                        backend: kind.map(|k| k.backend).unwrap_or(github::Backend::Other),
                                        runtime_asset: None,
                                    });
                                }
                            });
                        }
                    });
            }
        });
    });
}

/// Таблица бинарников внутри раскрытого релиза.
fn assets_table(
    app: &mut App,
    ui: &mut egui::Ui,
    store: &builds::BuildsStore,
    assets: &[github::BuildAsset],
) {
    let mut download: Option<github::BuildAsset> = None;
    TableBuilder::new(ui)
        .id_salt(format!("assets-{}", assets.first().map(|a| a.tag.as_str()).unwrap_or("none")))
        .striped(true)
        .vscroll(false)
        .column(Column::remainder().clip(true))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(90.0))
        .header(20.0, |mut header| {
            for name in ["Файл", "Бэкенд", "Размер", "Статус", ""] {
                header.col(|ui| {
                    ui.label(RichText::new(name).size(11.0).strong().color(MUTED));
                });
            }
        })
        .body(|mut body| {
            for asset in assets {
                let already_installed = store.dir_for(asset).is_dir();
                body.row(24.0, |mut row| {
                    row.col(|ui| {
                        ui.label(RichText::new(&asset.asset.name).monospace().size(11.0));
                    });
                    row.col(|ui| {
                        backend_badge(ui, asset.backend);
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(format_size(asset.asset.size)).size(11.0).color(MUTED));
                    });
                    row.col(|ui| {
                        if already_installed {
                            badge(ui, "установлена", theme::OK_GREEN);
                        }
                    });
                    row.col(|ui| {
                        let label = if already_installed { "⤓ Заменить" } else { "⬇ Скачать" };
                        if ui
                            .add_enabled(!app.is_downloading(&asset.asset.name), egui::Button::new(label).small())
                            .on_hover_text(format!(
                                "{}\n{}",
                                asset.asset.name,
                                if already_installed {
                                    "Уже установлена — скачивание заменит её заново"
                                } else {
                                    "Скачать и установить в библиотеку сборок"
                                }
                            ))
                            .clicked()
                        {
                            download = Some(asset.clone());
                        }
                    });
                });
            }
        });
    if let Some(asset) = download {
        app.start_build_download(asset);
    }
}

fn first_visible_tag(assets: &[github::BuildAsset]) -> String {
    assets.first().map(|a| a.tag.clone()).unwrap_or_default()
}

/// Панель прогресса текущего скачивания сборки.
fn download_panel(app: &mut App, ui: &mut egui::Ui) {
    let mut clear_download = false;
    if let Some(download) = app.build_download.as_mut() {
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&download.asset.asset.name).monospace().size(12.0).strong());
                if let Some(error) = &download.error {
                    ui.label(RichText::new(error).size(12.0).color(theme::ERR_RED));
                    if ui.small_button("Скрыть").clicked() {
                        clear_download = true;
                    }
                }
            });
            if download.error.is_none() {
                if download.extracting {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new("Распаковка архива…").size(13.0).color(MUTED));
                    });
                } else {
                    let fraction = if download.total > 0 {
                        download.downloaded as f32 / download.total as f32
                    } else {
                        0.0
                    };
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .show_percentage()
                            .desired_width(ui.available_width()),
                    );
                    let total = if download.total > 0 {
                        format_size(download.total)
                    } else {
                        "?".to_string()
                    };
                    ui.label(
                        RichText::new(format!("{} из {}", format_size(download.downloaded), total))
                            .size(11.0)
                            .color(MUTED),
                    );
                }
            }
        });
    }
    if clear_download {
        app.build_download = None;
    }
}
