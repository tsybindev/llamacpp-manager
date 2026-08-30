use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, ScrollArea};

use crate::config::{self, Settings};
use crate::logger::{self, LogHandle, LogEntry};
use crate::params::{self, ParamDef, ParamKind, ParamsCatalog, ParamState};
use crate::presets::{self, Preset};
use crate::process_mgr::{ServerConfig, ServerState, ServerManager};
use crate::theme::{self, ThemeMode};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Server,
    Models,
    Builds,
    Settings,
}

impl Page {
    const ALL: [Page; 4] = [Page::Server, Page::Models, Page::Builds, Page::Settings];

    fn icon(self) -> &'static str {
        match self {
            Page::Server => "🖥",
            Page::Models => "📦",
            Page::Builds => "⚙️",
            Page::Settings => "🔧",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Page::Server => "🖥  Сервер",
            Page::Models => "📦  Модели",
            Page::Builds => "⚙️  Сборки",
            Page::Settings => "🔧  Настройки",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Page::Server => "Управление сервером",
            Page::Models => "Модели (HuggingFace)",
            Page::Builds => "Сборки llama.cpp",
            Page::Settings => "Настройки",
        }
    }
}

pub struct App {
    page: Page,
    settings: Settings,
    applied_theme: Option<ThemeMode>,
    sidebar_collapsed: bool,
    log_handle: Option<LogHandle>,
    config_dirty: bool,
    last_change: Option<Instant>,
    server: ServerManager,
    server_form: ServerForm,
    params_catalog: ParamsCatalog,
    presets: presets::PresetStore,
    /// Preset picked in the combo box, if any.
    selected_preset: Option<String>,
    /// Mirror of `selected_preset` to detect selection changes and sync
    /// the name input field with the picked preset.
    mirrored_preset: Option<String>,
    /// Name input used for save/rename/import operations.
    preset_name_edit: String,
    /// Second-click confirmation state for the delete button.
    preset_delete_armed: bool,
    /// Result of the last preset operation: (is_error, message).
    preset_msg: Option<(bool, String)>,
    /// Fresh catalog fetched in the background, picked up on the next frame.
    catalog_refresh: std::sync::Arc<std::sync::Mutex<Option<ParamsCatalog>>>,
}

/// Editable server launch parameters (will become presets in a later stage).
#[derive(Clone, Debug)]
struct ServerForm {
    binary: PathBuf,
    model: PathBuf,
    host: String,
    port: u16,
    extra_args: String,
    last_error: Option<String>,
}

impl Default for ServerForm {
    fn default() -> Self {
        Self {
            binary: PathBuf::new(),
            model: PathBuf::new(),
            host: "127.0.0.1".to_string(),
            port: 8080,
            extra_args: String::new(),
            last_error: None,
        }
    }
}

impl ServerForm {
    fn to_config(&self, extra_args: Vec<String>) -> ServerConfig {
        ServerConfig {
            binary: self.binary.clone(),
            model: self.model.clone(),
            host: self.host.trim().to_string(),
            port: self.port,
            extra_args,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let (settings, load_warning) = config::load();
        if let Err(e) = settings.ensure_dirs() {
            eprintln!("не удалось создать каталоги данных: {e:#}");
        }
        if !config::config_path().exists()
            && let Err(e) = config::save(&settings)
        {
            eprintln!("не удалось сохранить настройки по умолчанию: {e:#}");
        }
        let (log_handle, logger_error) = logger::init(settings.debug_logging, &settings.logs_dir);

        log::info!("LlamaCpp Manager запущен");
        if let Some(warning) = load_warning {
            log::warn!("Настройки: {warning}");
        }
        if let Some(error) = logger_error {
            log::warn!("Логирование в файл может не работать: {error}");
        }
        log::debug!("Debug-логирование: {}", if settings.debug_logging { "включено" } else { "выключено" });

        let params_catalog = params::bundled_catalog();
        let mut settings = settings;
        settings.params.merge_defaults(&params_catalog);

        // Presets live next to the data directories (<data>/presets).
        let presets_dir = settings
            .logs_dir
            .parent()
            .map(|p| p.join("presets"))
            .unwrap_or_else(|| settings.logs_dir.join("presets"));

        // Background catalog refresh: bundled catalog first, remote applied
        // when its version is newer.
        let catalog_refresh = std::sync::Arc::new(std::sync::Mutex::new(None));
        {
            let pending = catalog_refresh.clone();
            let url = settings.params_catalog_url.clone();
            let current_version = params_catalog.version;
            std::thread::Builder::new()
                .name("catalog-refresh".into())
                .spawn(move || match params::fetch_catalog(&url) {
                    Ok(remote) if remote.version > current_version => {
                        log::info!(
                            "Найдено обновление каталога параметров: версия {current_version} → {}",
                            remote.version
                        );
                        if let Ok(mut guard) = pending.lock() {
                            *guard = Some(remote);
                        }
                    }
                    Ok(remote) => {
                        log::debug!("Каталог параметров актуален (версия {})", remote.version);
                        let _ = remote;
                    }
                    Err(e) => log::debug!("Каталог параметров оставлен без изменений: {e}"),
                })
                .ok();
        }

        Self {
            page: Page::Server,
            sidebar_collapsed: settings.sidebar_collapsed,
            server: ServerManager::new(settings.logs_dir.clone(), settings.auto_restore.clone()),
            server_form: ServerForm::default(),
            params_catalog,
            presets: presets::PresetStore::new(presets_dir),
            selected_preset: None,
            mirrored_preset: None,
            preset_name_edit: String::new(),
            preset_delete_armed: false,
            preset_msg: None,
            catalog_refresh,
            settings,
            applied_theme: None,
            log_handle,
            config_dirty: false,
            last_change: None,
        }
    }

    fn mark_dirty(&mut self) {
        self.config_dirty = true;
        self.last_change = Some(Instant::now());
    }

    fn save_settings(&mut self) {
        match config::save(&self.settings) {
            Ok(()) => {
                self.config_dirty = false;
                self.last_change = None;
                log::debug!("Настройки сохранены в {}", config::config_path().display());
            }
            Err(e) => log::error!("Не удалось сохранить настройки: {e:#}"),
        }
    }

    fn nav_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            if !self.sidebar_collapsed {
                ui.heading(
                    RichText::new("LlamaCpp Manager")
                        .color(theme::ACCENT)
                        .size(18.0),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let toggle = if self.sidebar_collapsed { "»" } else { "«" };
                if ui
                    .button(toggle)
                    .on_hover_text(if self.sidebar_collapsed {
                        "Развернуть панель"
                    } else {
                        "Свернуть панель"
                    })
                    .clicked()
                {
                    self.sidebar_collapsed = !self.sidebar_collapsed;
                    self.settings.sidebar_collapsed = self.sidebar_collapsed;
                    self.mark_dirty();
                }
            });
        });

        if !self.sidebar_collapsed {
            ui.small("локальный менеджер llama-server");
        }
        ui.add_space(16.0);

        for page in Page::ALL {
            let selected = self.page == page;
            let response = if self.sidebar_collapsed {
                let icon = RichText::new(page.icon()).size(17.0);
                ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::Button::new(icon).selected(selected),
                )
            } else {
                ui.selectable_label(selected, RichText::new(page.label()).size(15.0))
            };
            if response.clicked() {
                self.page = page;
            }
            if self.sidebar_collapsed {
                response.on_hover_text(page.title());
            }
            ui.add_space(2.0);
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            self.sidebar_server_block(ui);
        });
    }

    /// Persistent server status and control buttons at the bottom of the
    /// sidebar, so they are reachable from any page without scrolling.
    fn sidebar_server_block(&mut self, ui: &mut egui::Ui) {
        let collapsed = self.sidebar_collapsed;
        let state = self.server.state();
        let (color, hint) = state_status(state);
        let running = self.server.is_running();
        let has_config = self.server.config().is_some();

        if collapsed {
            ui.add_sized(
                [ui.available_width(), 20.0],
                egui::Label::new(RichText::new("●").color(color).size(16.0)),
            )
            .on_hover_text(format!("Сервер: {}", state.label()));
            for (icon, tooltip, enabled, action) in [
                ("▶", "Запустить сервер", !running, 0),
                ("■", "Остановить сервер", running, 1),
                ("↻", "Перезапустить сервер", has_config, 2),
            ] {
                let button = if enabled {
                    egui::Button::new(icon)
                } else {
                    egui::Button::new(RichText::new(icon).weak())
                };
                let clicked = ui
                    .add_sized([ui.available_width(), 24.0], button)
                    .on_hover_text(tooltip)
                    .clicked();
                if clicked && enabled {
                    match action {
                        0 => self.try_start_server(),
                        1 => self.server.stop(),
                        _ => self.try_restart_server(),
                    }
                }
            }
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(color).size(16.0))
                    .on_hover_text(hint);
                ui.label(RichText::new(state.label()).strong());
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!running, egui::Button::new("▶"))
                    .on_hover_text("Запустить сервер")
                    .clicked()
                {
                    self.try_start_server();
                }
                if ui
                    .add_enabled(running, egui::Button::new("■"))
                    .on_hover_text("Остановить сервер")
                    .clicked()
                {
                    self.server.stop();
                }
                if ui
                    .add_enabled(has_config, egui::Button::new("↻"))
                    .on_hover_text("Перезапустить сервер")
                    .clicked()
                {
                    self.try_restart_server();
                }
                if let Some(error) = self.server_form.last_error.as_ref() {
                    ui.label(RichText::new(error).small().color(theme::ERR_RED))
                        .on_hover_text(error);
                }
            });
        }
    }

    fn page_content(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.page.title());
        ui.add_space(8.0);
        match self.page {
            Page::Server => self.server_page(ui),
            Page::Models => self.models_page(ui),
            Page::Builds => self.builds_page(ui),
            Page::Settings => self.settings_page(ui),
        }
    }

    fn server_page(&mut self, ui: &mut egui::Ui) {
        self.server_status_bar(ui);
        ui.add_space(8.0);
        self.presets_section(ui);
        ui.add_space(12.0);
        self.server_config_section(ui);
        ui.add_space(12.0);
        self.server_log_panel(ui);
        ui.add_space(12.0);
        self.app_log_panel(ui);
    }

    /// Preset management: load, save-as, rename, delete, import/export.
    /// While a preset is selected, any configuration change is auto-saved
    /// into it (see the debounce block in `ui`).
    fn presets_section(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("Пресеты").size(16.0));
        let names = self.presets.list();
        // Forget a selection that no longer exists on disk.
        if let Some(sel) = self.selected_preset.as_ref()
            && !names.contains(sel)
        {
            self.selected_preset = None;
            self.preset_delete_armed = false;
        }
        // Sync the name field whenever the combo box selection changes.
        if self.selected_preset != self.mirrored_preset {
            self.mirrored_preset = self.selected_preset.clone();
            self.preset_name_edit = self.selected_preset.clone().unwrap_or_default();
            self.preset_delete_armed = false;
        }
        let selection = self.selected_preset.clone();
        let has_selection = selection.is_some();
        let typed_name = self.preset_name_edit.trim().to_string();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = selection.clone().unwrap_or_else(|| "— выберите пресет —".into());
            egui::ComboBox::from_id_salt("preset-select")
                .selected_text(label)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for name in &names {
                        ui.selectable_value(&mut self.selected_preset, Some(name.clone()), name.clone());
                    }
                });
            if ui
                .add_enabled(has_selection, egui::Button::new("Загрузить"))
                .on_hover_text("Применить пресет к текущей конфигурации")
                .clicked()
            {
                self.preset_delete_armed = false;
                self.load_selected_preset();
            }
            let delete_label = if self.preset_delete_armed { "Точно удалить?" } else { "Удалить" };
            if ui
                .add_enabled(has_selection, egui::Button::new(delete_label))
                .on_hover_text("Второй щелчок подтверждает удаление")
                .clicked()
            {
                if self.preset_delete_armed {
                    self.delete_selected_preset();
                    self.preset_delete_armed = false;
                } else {
                    self.preset_delete_armed = true;
                }
            }
        });

        ui.horizontal(|ui| {
            ui.add_sized([LABEL_WIDTH, 18.0], egui::Label::new("Имя пресета"));
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name_edit)
                    .desired_width(240.0)
                    .hint_text("например: gemma-mtp-локально"),
            );
            if ui
                .add_enabled(
                    !typed_name.is_empty(),
                    egui::Button::new("Сохранить как новый"),
                )
                .on_hover_text("Сохранить текущую конфигурацию и параметры под введённым именем")
                .clicked()
            {
                self.save_current_as_preset();
            }
            let name_differs = has_selection && selection.as_deref() != Some(typed_name.as_str());
            if ui
                .add_enabled(
                    name_differs,
                    egui::Button::new("Переименовать"),
                )
                .on_hover_text(format!(
                    "Переименовать «{}» в «{typed_name}»",
                    selection.as_deref().unwrap_or("")
                ))
                .clicked()
            {
                self.preset_delete_armed = false;
                self.rename_selected_preset();
            }
            if ui.button("Экспорт…").clicked() {
                self.export_selected_preset();
            }
            if ui.button("Импорт…").clicked() {
                self.import_preset_file();
            }
        });

        if has_selection {
            ui.label(
                RichText::new(format!(
                    "Изменения конфигурации и параметров автоматически сохраняются в пресет «{}».",
                    selection.unwrap()
                ))
                .small()
                .weak(),
            );
        }
        if let Some((is_error, message)) = &self.preset_msg {
            let color = if *is_error { theme::ERR_RED } else { theme::OK_GREEN };
            ui.label(RichText::new(message).color(color).small());
        }
    }

    fn set_preset_msg(&mut self, is_error: bool, message: String) {
        if is_error {
            log::error!("Пресеты: {message}");
        } else {
            log::info!("Пресеты: {message}");
        }
        self.preset_msg = Some((is_error, message));
    }

    /// Snapshot of the current form + parameters as a preset.
    fn current_preset(&self, name: String) -> Preset {
        Preset {
            name,
            binary: self.server_form.binary.clone(),
            model: self.server_form.model.clone(),
            host: self.server_form.host.clone(),
            port: self.server_form.port,
            extra_args: self.server_form.extra_args.clone(),
            params: self.settings.params.clone(),
        }
    }

    fn apply_preset(&mut self, mut preset: Preset) {
        self.server_form.binary = preset.binary.clone();
        self.server_form.model = preset.model.clone();
        self.server_form.host = preset.host.clone();
        self.server_form.port = preset.port;
        self.server_form.extra_args = preset.extra_args.clone();
        self.server_form.last_error = None;
        preset.params.merge_defaults(&self.params_catalog);
        self.settings.params = preset.params;
        self.mark_dirty();
    }

    fn load_selected_preset(&mut self) {
        let Some(name) = self.selected_preset.clone() else {
            return;
        };
        match self.presets.load(&name) {
            Ok(preset) => {
                self.apply_preset(preset);
                self.set_preset_msg(false, format!("Пресет «{name}» загружен"));
            }
            Err(e) => self.set_preset_msg(true, format!("Не удалось загрузить пресет: {e:#}")),
        }
    }

    fn save_current_as_preset(&mut self) {
        let name = self.preset_name_edit.trim().to_string();
        let preset = self.current_preset(name.clone());
        match self.presets.save(&preset) {
            Ok(()) => {
                self.selected_preset = Some(name.clone());
                self.preset_delete_armed = false;
                self.set_preset_msg(false, format!("Пресет «{name}» сохранён"));
            }
            Err(e) => self.set_preset_msg(true, format!("Не удалось сохранить пресет: {e:#}")),
        }
    }

    fn rename_selected_preset(&mut self) {
        let new_name = self.preset_name_edit.trim().to_string();
        let Some(old_name) = self.selected_preset.clone() else {
            return;
        };
        if new_name == old_name {
            self.set_preset_msg(true, "Новое имя совпадает с текущим".into());
            return;
        }
        match self.presets.load(&old_name).map(|mut p| {
            p.name = new_name.clone();
            p
        }) {
            Ok(renamed) => match self.presets.save(&renamed).and_then(|()| self.presets.delete(&old_name)) {
                Ok(()) => {
                    self.selected_preset = Some(new_name.clone());
                    self.set_preset_msg(false, format!("«{old_name}» переименован в «{new_name}»"));
                }
                Err(e) => self.set_preset_msg(true, format!("Не удалось переименовать пресет: {e:#}")),
            },
            Err(e) => self.set_preset_msg(true, format!("Не удалось переименовать пресет: {e:#}")),
        }
    }

    fn delete_selected_preset(&mut self) {
        let Some(name) = self.selected_preset.clone() else {
            return;
        };
        match self.presets.delete(&name) {
            Ok(()) => {
                self.selected_preset = None;
                self.set_preset_msg(false, format!("Пресет «{name}» удалён"));
            }
            Err(e) => self.set_preset_msg(true, format!("Не удалось удалить пресет: {e:#}")),
        }
    }

    fn export_selected_preset(&mut self) {
        let Some(name) = self.selected_preset.clone() else {
            self.set_preset_msg(true, "Сначала выберите пресет для экспорта".into());
            return;
        };
        match self.presets.load(&name) {
            Ok(preset) => {
                let default_file = format!("{}.json", presets::sanitize_name(&name));
                let dialog = rfd::FileDialog::new()
                    .set_title("Куда сохранить пресет")
                    .set_file_name(&default_file)
                    .add_filter("JSON-пресет", &["json"]);
                if let Some(path) = dialog.save_file() {
                    match presets::export_to(&preset, &path) {
                        Ok(()) => self.set_preset_msg(false, format!("Пресет экспортирован: {}", path.display())),
                        Err(e) => self.set_preset_msg(true, format!("Не удалось экспортировать пресет: {e:#}")),
                    }
                }
            }
            Err(e) => self.set_preset_msg(true, format!("Не удалось прочитать пресет: {e:#}")),
        }
    }

    fn import_preset_file(&mut self) {
        // The dialog must be created and awaited only on click —
        // pick_file is a blocking call.
        let dialog = rfd::FileDialog::new()
            .set_title("Выберите файл пресета")
            .add_filter("JSON-пресет", &["json"]);
        if let Some(path) = dialog.pick_file() {
            match presets::import_from(&path) {
                Ok(mut preset) => {
                    // A typed name wins over the name inside the file / file stem.
                    let typed = self.preset_name_edit.trim().to_string();
                    if !typed.is_empty() {
                        preset.name = typed;
                    }
                    let name = preset.name.clone();
                    match self.presets.save(&preset) {
                        Ok(()) => {
                            self.selected_preset = Some(name.clone());
                            self.preset_delete_armed = false;
                            self.set_preset_msg(false, format!("Пресет «{name}» импортирован"));
                        }
                        Err(e) => self.set_preset_msg(true, format!("Не удалось импортировать пресет: {e:#}")),
                    }
                }
                Err(e) => self.set_preset_msg(true, format!("Не удалось импортировать пресет: {e:#}")),
            }
        }
    }

    /// Pre-start checks shown to the user before spawning the process.
    /// Returns Err with a human-readable summary when it is not OK to proceed.
    fn pre_flight_check(&self, config: &ServerConfig) -> Result<(), String> {
        let mut problems: Vec<String> = Vec::new();
        if !config.binary.is_file() {
            problems.push(format!("бинарник не найден: {}", config.binary.display()));
        }
        if !config.model.is_file() {
            problems.push(format!("файл модели не найден: {}", config.model.display()));
        }
        if crate::process_mgr::port_in_use(&config.host, config.port) {
            problems.push(format!("порт {} уже занят другим процессом", config.port));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            let summary = problems.join("; ");
            log::warn!("Предпусковая проверка не пройдена: {summary}");
            Err(summary)
        }
    }

    fn server_status_bar(&mut self, ui: &mut egui::Ui) {
        let state = self.server.state();
        let (color, hint) = state_status(state);
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(color).size(18.0));
            ui.label(RichText::new(state.label()).size(16.0).strong());
            ui.label(RichText::new(hint).weak().small());
        });
        if state == ServerState::Ready {
            ui.horizontal(|ui| {
                let url = format!(
                    "http://{}:{}",
                    self.server_form.host.trim(),
                    self.server_form.port
                );
                ui.label(RichText::new(&url).monospace());
                if ui.small_button("Копировать адрес API").clicked() {
                    ui.ctx().copy_text(url.clone());
                }
                if ui.small_button("Открыть в браузере").clicked() {
                    open_url(&url);
                }
            });
        }
        if let Some(error) = &self.server_form.last_error {
            ui.label(RichText::new(error).color(theme::ERR_RED).small());
        }
    }

    fn try_start_server(&mut self) {
        self.server_form.last_error = None;
        let config = self.server_form.to_config(self.build_extra_args());
        if let Err(e) = self
            .pre_flight_check(&config)
            .and_then(|()| self.server.start(config))
        {
            log::error!("Запуск не удался: {e}");
            self.server_form.last_error = Some(e);
        }
    }

    fn try_restart_server(&mut self) {
        self.server_form.last_error = None;
        if let Err(e) = self.server.restart() {
            log::error!("Перезапуск не удался: {e}");
            self.server_form.last_error = Some(e);
        }
    }

    fn build_extra_args(&self) -> Vec<String> {
        let mut args = params::to_args(&self.params_catalog, &self.settings.params);
        if let Some(raw) = shlex::split(&self.server_form.extra_args) {
            args.extend(raw);
        }
        args
    }

    /// Parameter catalog UI grouped by category.
    fn params_section(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("Параметры llama-server").size(16.0));
        // Sections with draft/MTP and GPU settings are open by default —
        // they are the most-used knobs for this app's audience.
        for (cat_id, cat_label) in self.params_catalog.categories() {
            let defs: Vec<&ParamDef> = self
                .params_catalog
                .params
                .iter()
                .filter(|p| p.category == cat_id)
                .collect();
            if defs.is_empty() {
                continue;
            }
            let default_open = matches!(cat_id, "context" | "gpu" | "spec");
            egui::CollapsingHeader::new(RichText::new(cat_label).size(14.0))
                .default_open(default_open)
                .show(ui, |ui| {
                    for def in defs {
                        param_row(ui, def, &mut self.settings.params);
                    }
                });
        }
        ui.add_space(4.0);
    }

    fn server_config_section(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("Конфигурация").size(16.0));
        {
            let form = &mut self.server_form;
            path_row(ui, "Бинарник llama-server", &mut form.binary, "путь к llama-server", PathPick::File);
            path_row(ui, "Файл модели", &mut form.model, "путь к GGUF-файлу модели", PathPick::File);

            ui.horizontal(|ui| {
                ui.add_sized([LABEL_WIDTH, 18.0], egui::Label::new("Host"));
                ui.add(
                    egui::TextEdit::singleline(&mut form.host)
                        .desired_width(220.0)
                        .hint_text("127.0.0.1"),
                );
                ui.label("Порт");
                ui.add(
                    egui::DragValue::new(&mut form.port)
                        .range(1..=65535)
                        .custom_formatter(|v, _| format!("{}", v as u16))
                        .custom_parser(|s| s.parse::<u16>().ok().map(f64::from)),
                );
            });
        }

        if self.server_form.host.trim() == "0.0.0.0" {
            ui.label(
                RichText::new("⚠ Host 0.0.0.0 делает сервер доступным из сети. Убедитесь, что это безопасно.")
                    .small()
                    .color(theme::WARN_YELLOW),
            );
        }

        ui.add_space(4.0);
        self.params_section(ui);

        ui.add_space(4.0);
        ui.label("Сырые аргументы (сверх каталога)").on_hover_text("Разделяйте пробелом; кавычки поддерживаются. Для флагов, которых нет в каталоге.");
        ui.add(
            egui::TextEdit::multiline(&mut self.server_form.extra_args)
                .desired_rows(2)
                .desired_width(ui.available_width())
                .code_editor(),
        );

        // Parameter validation warnings.
        let problems = params::validate(&self.params_catalog, &self.settings.params);
        if !problems.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "⚠ Проверка параметров:\n{}",
                    problems.iter().map(|p| format!("• {p}")).collect::<Vec<_>>().join("\n")
                ))
                .small()
                .color(theme::WARN_YELLOW),
            );
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Команда запуска:\n{}",
                self.server_form.to_config(self.build_extra_args()).command_line()
            ))
            .monospace()
            .small(),
        );
    }

    fn server_log_panel(&mut self, ui: &mut egui::Ui) {
        ui.collapsing(RichText::new("📜 Журнал llama-server").size(15.0), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Сохранить лог в файл…").clicked() {
                    self.save_server_log();
                }
                if ui.button("Очистить").clicked()
                    && let Ok(mut log) = self.server.server_log().lock()
                {
                    log.clear();
                }
            });
            ui.add_space(4.0);
            let lines: Vec<(chrono::DateTime<chrono::Local>, String)> = self
                .server
                .server_log()
                .lock()
                .map(|log| log.lines().to_vec())
                .unwrap_or_default();
            ScrollArea::vertical()
                .id_salt("server_log")
                .stick_to_bottom(true)
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::Frame::default()
                        .fill(ui.visuals().extreme_bg_color)
                        .inner_margin(6.0)
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            if lines.is_empty() {
                                ui.weak("нет вывода процесса");
                            }
                            for (time, line) in lines {
                                ui.label(
                                    RichText::new(format!("{} {}", time.format("%H:%M:%S"), line))
                                        .monospace()
                                        .size(12.0),
                                );
                            }
                        });
                });
        });
    }

    fn save_server_log(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("llama-server.log")
            .save_file()
        else {
            return;
        };
        let lines = self
            .server
            .server_log()
            .lock()
            .map(|log| log.lines().to_vec())
            .unwrap_or_default();
        let content = lines
            .iter()
            .map(|(time, line)| format!("{} {}\n", time.format("%Y-%m-%d %H:%M:%S%.3f"), line))
            .collect::<String>();
        match std::fs::write(&path, content) {
            Ok(()) => log::info!("Лог сервера сохранён: {}", path.display()),
            Err(e) => log::error!("Не удалось сохранить лог: {e}"),
        }
    }

    fn app_log_panel(&mut self, ui: &mut egui::Ui) {
        let Some(handle) = self.log_handle.clone() else {
            return;
        };
        ui.collapsing(RichText::new("📜 Журнал приложения").size(15.0), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Очистить").clicked() {
                    handle.clear();
                }
                if ui.button("Открыть папку логов").clicked() {
                    open_folder(&self.settings.logs_dir);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut debug = self.settings.debug_logging;
                    if ui
                        .checkbox(&mut debug, "Debug")
                        .on_hover_text("Подробные debug-сообщения (настраивается и на экране настроек)")
                        .changed()
                    {
                        self.settings.debug_logging = debug;
                        if let Some(handle) = &self.log_handle {
                            handle.set_debug(debug);
                        }
                        self.mark_dirty();
                    }
                });
            });
            ui.add_space(4.0);

            let entries: Vec<LogEntry> = handle.snapshot();
            ScrollArea::vertical()
                .id_salt("app_log")
                .stick_to_bottom(true)
                .max_height(220.0)
                .show(ui, |ui| {
                    egui::Frame::default()
                        .fill(ui.visuals().extreme_bg_color)
                        .inner_margin(6.0)
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            if entries.is_empty() {
                                ui.weak("журнал пуст");
                            }
                            for entry in entries {
                                let color = level_color(entry.level);
                                ui.label(
                                    RichText::new(format!(
                                        "{} {:<5} {}",
                                        entry.time.format("%H:%M:%S"),
                                        entry.level,
                                        entry.message
                                    ))
                                    .monospace()
                                    .size(12.0)
                                    .color(color),
                                );
                            }
                        });
                });
        });
    }

    fn models_page(&mut self, ui: &mut egui::Ui) {
        ui.label("Экран моделей будет здесь: поиск и скачивание GGUF с HuggingFace.");
    }

    fn builds_page(&mut self, ui: &mut egui::Ui) {
        ui.label("Экран сборок будет здесь: релизы llama.cpp и выбор бэкенда.");
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Все настройки сохраняются автоматически.");
            if self.config_dirty {
                ui.label(RichText::new("● не сохранено").color(theme::WARN_YELLOW));
            }
            if ui.button("Сохранить сейчас").clicked() {
                self.save_settings();
            }
        });
        ui.add_space(8.0);

        ui.add_space(12.0);
        ui.separator();
        ui.heading(RichText::new("Каталоги").size(16.0));
        let paths_changed = {
            let s = &mut self.settings;
            let mut changed = false;
            changed |= path_row(ui, "Каталог моделей", &mut s.models_dir, "куда скачиваются GGUF-модели", PathPick::Folder);
            changed |= path_row(ui, "Каталог сборок", &mut s.builds_dir, "куда скачиваются бинарники llama.cpp", PathPick::Folder);
            changed |= path_row(ui, "Каталог логов", &mut s.logs_dir, "куда пишутся журналы", PathPick::Folder);
            changed
        };
        if paths_changed {
            self.mark_dirty();
        }

        ui.add_space(12.0);
        ui.separator();
        ui.heading(RichText::new("HuggingFace").size(16.0));
        let mut token = self.settings.hf_token.clone();
        let token_response = ui.add(
            egui::TextEdit::singleline(&mut token)
                .password(true)
                .hint_text("hf_...")
                .desired_width(ui.available_width()),
        );
        if token_response.changed() {
            self.settings.hf_token = token.trim().to_string();
            self.mark_dirty();
        }
        ui.label(
            RichText::new("⚠ Токен хранится в конфиг-файле в открытом виде. Нужен только для приватных/gated-моделей.")
                .small()
                .color(theme::WARN_YELLOW),
        );

        ui.add_space(12.0);
        ui.separator();
        ui.heading(RichText::new("Интерфейс").size(16.0));
        ui.horizontal(|ui| {
            ui.label("Тема:");
            for mode in ThemeMode::ALL {
                if ui
                    .selectable_label(self.settings.theme == mode, mode.label())
                    .clicked()
                {
                    self.settings.theme = mode;
                    self.mark_dirty();
                }
            }
        });
        let mut debug = self.settings.debug_logging;
        if ui
            .checkbox(&mut debug, "Debug-логирование (подробные сообщения в журнал)")
            .changed()
        {
            self.settings.debug_logging = debug;
            if let Some(handle) = &self.log_handle {
                handle.set_debug(debug);
            }
            self.mark_dirty();
        }
        ui.label(
            RichText::new(format!(
                "Файл журнала: {}",
                self.settings.logs_dir.join("manager.log").display()
            ))
            .small(),
        );

        ui.add_space(12.0);
        ui.separator();
        ui.heading(RichText::new("Автовосстановление сервера").size(16.0));
        let mut ar = self.settings.auto_restore.clone();
        ui.checkbox(&mut ar.enabled, "Автоматически перезапускать llama-server при падении");
        ui.add_enabled_ui(ar.enabled, |ui| {
            egui::Grid::new("auto_restore_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Макс. попыток рестарта:");
                    ui.add(
                        egui::DragValue::new(&mut ar.max_restarts)
                            .range(1..=20)
                            .suffix(" раз"),
                    );
                    ui.end_row();
                    ui.label("Окно времени:");
                    ui.add(
                        egui::DragValue::new(&mut ar.window_secs)
                            .range(30..=3600)
                            .speed(10.0)
                            .suffix(" сек"),
                    );
                    ui.end_row();
                    ui.label("Задержка между попытками (backoff):");
                    ui.add(
                        egui::DragValue::new(&mut ar.backoff_start_secs)
                            .range(1..=60)
                            .suffix(" сек"),
                    );
                    ui.end_row();
                });
            ui.label(
                RichText::new("Задержка удваивается после каждой попытки. При исчерпании лимита сервер остаётся в состоянии «упал» и требуется вмешательство.")
                    .small(),
            );
        });
        if ar != self.settings.auto_restore {
            self.server.set_auto_restore(ar.clone());
            self.settings.auto_restore = ar;
            self.mark_dirty();
        }
    }

}

/// Minimum width reserved for parameter/path labels before the value widget.
const LABEL_WIDTH: f32 = 210.0;
/// Width reserved for the "Обзор…" browse button plus margins.
const BROWSE_BUTTON_WIDTH: f32 = 88.0;

/// What kind of item the file dialog should let the user pick.
#[derive(Clone, Copy)]
enum PathPick {
    File,
    Folder,
}

/// One parameter row: enable checkbox + value widget depending on kind.
/// Non-bool parameters render as `☑ Name [widget……………]` on a single line.
fn param_row(ui: &mut egui::Ui, def: &ParamDef, state: &mut ParamState) {
    let mut tooltip = format!("{}\n\nФлаг: {}", def.description, def.flag);
    if let Some(short) = &def.short {
        tooltip.push_str(&format!(" / {short}"));
    }

    match def.kind {
        // Bool parameters have no value: the checkbox is the whole control.
        ParamKind::Bool => {
            let mut on = state.is_enabled(&def.id);
            if ui
                .checkbox(&mut on, &def.name)
                .on_hover_text(&tooltip)
                .changed()
            {
                state.set(&def.id, on);
            }
        }
        _ => {
            let new_value = ui
                .horizontal(|ui| {
                    let mut enabled = state.is_enabled(&def.id);
                    ui.checkbox(&mut enabled, &def.name)
                        .on_hover_text(&tooltip)
                        .changed()
                        .then(|| state.set(&def.id, enabled));

                    let value = state
                        .entries
                        .get(&def.id)
                        .and_then(|e| e.value.clone())
                        .or_else(|| def.default.clone());
                    let field_width = (ui.available_width() - 8.0).max(120.0);

                    match def.kind {
                        ParamKind::Int => {
                            let min = def.min.unwrap_or(i64::MIN as f64) as i64;
                            let max = def.max.unwrap_or(i64::MAX as f64).min(i64::MAX as f64) as i64;
                            let mut v = value.and_then(|x| x.as_i64()).unwrap_or(min.max(0));
                            if ui
                                .add_sized(
                                    [110.0, 20.0],
                                    egui::DragValue::new(&mut v)
                                        .range(min..=max)
                                        .custom_formatter(|n, _| format!("{n}"))
                                        .custom_parser(|s| {
                                            s.trim().parse::<i64>().ok().map(|n| n as f64)
                                        }),
                                )
                                .on_hover_text(&tooltip)
                                .changed()
                            {
                                return Some(serde_json::json!(v));
                            }
                        }
                        ParamKind::Float => {
                            let min = def.min.unwrap_or(f64::MIN);
                            let max = def.max.unwrap_or(f64::MAX);
                            let mut v = value.and_then(|x| x.as_f64()).unwrap_or(min.max(0.0));
                            if ui
                                .add_sized(
                                    [110.0, 20.0],
                                    egui::DragValue::new(&mut v)
                                        .range(min..=max)
                                        .speed(0.05),
                                )
                                .on_hover_text(&tooltip)
                                .changed()
                            {
                                return Some(serde_json::json!(v));
                            }
                        }
                        ParamKind::Enum => {
                            let previous = value
                                .as_ref()
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let mut current = previous.clone();
                            egui::ComboBox::from_id_salt(def.id.clone())
                                .selected_text(current.clone())
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for option in &def.options {
                                        ui.selectable_value(&mut current, option.clone(), option);
                                    }
                                })
                                .response
                                .on_hover_text(&tooltip);
                            if current != previous {
                                return Some(serde_json::json!(current));
                            }
                        }
                        ParamKind::String => {
                            let mut text = value
                                .as_ref()
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string();
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut text)
                                        .desired_width(field_width),
                                )
                                .on_hover_text(&tooltip)
                                .changed()
                            {
                                return Some(serde_json::json!(text.trim()));
                            }
                        }
                        ParamKind::Path => {
                            let mut text = value
                                .as_ref()
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let edit_width = (field_width - BROWSE_BUTTON_WIDTH).max(120.0);
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut text)
                                        .desired_width(edit_width),
                                )
                                .on_hover_text(&tooltip)
                                .changed()
                            {
                                return Some(serde_json::json!(text.trim()));
                            }
                            if ui.small_button("Обзор…").clicked()
                                && let Some(file) = rfd::FileDialog::new()
                                    .set_title(format!("Выберите: {}", def.name))
                                    .pick_file()
                            {
                                return Some(serde_json::json!(file.display().to_string()));
                            }
                        }
                        ParamKind::Bool => unreachable!(),
                    }
                    None
                })
                .inner;

            if let Some(value) = new_value {
                state.set_value(&def.id, value);
            }
        }
    }
}

/// One path picker row: label, editable path, browse button.
/// `pick` selects whether the dialog opens files or folders.
fn path_row(ui: &mut egui::Ui, label: &str, path: &mut PathBuf, hint: &str, pick: PathPick) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_WIDTH, 18.0], egui::Label::new(label))
            .on_hover_text(hint);
        let mut text = path.display().to_string();
        let edit_width = (ui.available_width() - BROWSE_BUTTON_WIDTH).max(160.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut text).desired_width(edit_width),
        );
        if response.changed() {
            *path = PathBuf::from(text.trim());
            changed = true;
        }
        if ui.button("Обзор…").clicked() {
            // The dialog must be created and awaited only on click —
            // pick_file/pick_folder are blocking calls.
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

/// Color and human hint for a server state, shared by the sidebar and the
/// status bar on the Server page.
fn state_status(state: ServerState) -> (Color32, &'static str) {
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

fn level_color(level: log::Level) -> Color32 {
    match level {
        log::Level::Error => theme::ERR_RED,
        log::Level::Warn => theme::WARN_YELLOW,
        log::Level::Debug | log::Level::Trace => Color32::from_rgb(0x8A, 0x94, 0xA6),
        log::Level::Info => Color32::from_rgb(0xB9, 0xC2, 0xD0),
    }
}

fn open_folder(path: &std::path::Path) {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
    if let Err(e) = result {
        log::warn!("Не удалось открыть папку {}: {e}", path.display());
    }
}

fn open_url(url: &str) {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd").args(["/C", "start", url]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        log::warn!("Не удалось открыть {url}: {e}");
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.applied_theme != Some(self.settings.theme) {
            theme::apply(&ui.ctx().clone(), self.settings.theme);
            self.applied_theme = Some(self.settings.theme);
        }

        // Pick up a catalog fetched in the background, if any.
        let incoming = self
            .catalog_refresh
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        if let Some(remote) = incoming {
            self.settings.params.merge_defaults(&remote);
            self.params_catalog = remote;
            self.mark_dirty();
        }

        // Debounced auto-save: write at most twice per second while editing.
        if self.config_dirty
            && self
                .last_change
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(500))
        {
            self.save_settings();
            // Persist the same changes into the selected preset, if any.
            if let Some(name) = self.selected_preset.clone() {
                let preset = self.current_preset(name.clone());
                if let Err(e) = self.presets.save(&preset) {
                    self.set_preset_msg(true, format!("Не удалось автосохранить пресет: {e:#}"));
                }
            }
        }

        let nav_width = if self.sidebar_collapsed { 56.0 } else { 200.0 };
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(nav_width)
            .show(ui, |ui| self.nav_panel(ui));

        egui::CentralPanel::default().show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| self.page_content(ui));
        });
    }

    fn on_exit(&mut self) {
        // PRD: llama-server must not outlive the manager.
        if self.server.is_running() {
            log::info!("Выход из приложения: останавливаем llama-server");
            self.server.stop();
        }
        if self.config_dirty {
            self.save_settings();
        }
    }
}
