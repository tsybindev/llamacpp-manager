use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::download::CancelFlag;

use egui::ScrollArea;

use crate::builds;
use crate::config::{self, Settings};
use crate::github;
use crate::gguf;
use crate::huggingface;
use crate::logger::{self, LogHandle};
use crate::params;
use crate::params::ParamsCatalog;
use crate::presets::{self, Preset};
use crate::process_mgr::{ServerConfig, ServerManager};
use crate::theme::{self, ThemeMode};
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Server,
    Models,
    Builds,
    Logs,
    Settings,
}

impl Page {
    const ALL: [Page; 5] = [Page::Server, Page::Models, Page::Builds, Page::Logs, Page::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Page::Server => "Сервер",
            Page::Models => "Модели",
            Page::Builds => "Сборки",
            Page::Logs => "Логи",
            Page::Settings => "Настройки",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Server => "Управление сервером",
            Page::Models => "Модели",
            Page::Builds => "Сборки llama.cpp",
            Page::Logs => "Журналы",
            Page::Settings => "Настройки",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Page::Server => "llama-server",
            Page::Models => "HuggingFace и локальная библиотека",
            Page::Builds => "релизы llama.cpp",
            Page::Logs => "вывод llama-server и приложения",
            Page::Settings => "",
        }
    }
}

pub struct App {
    pub page: Page,
    pub settings: Settings,
    pub applied_theme: Option<ThemeMode>,
    pub sidebar_collapsed: bool,
    pub log_handle: Option<LogHandle>,
    pub config_dirty: bool,
    pub last_change: Option<Instant>,
    pub server: ServerManager,
    pub server_form: ServerForm,
    pub params_catalog: ParamsCatalog,
    pub presets: presets::PresetStore,
    /// Preset picked in the combo box, if any.
    pub selected_preset: Option<String>,
    /// Mirror of `selected_preset` to detect selection changes and sync
    /// the name input field with the picked preset.
    pub mirrored_preset: Option<String>,
    /// Name input used for save/rename/import operations.
    pub preset_name_edit: String,
    /// Second-click confirmation state for the delete button.
    pub preset_delete_armed: bool,
    /// Result of the last preset operation: (is_error, message).
    pub preset_msg: Option<(bool, String)>,
    /// Активная вкладка параметров на странице «Сервер».
    pub params_tab: usize,
    /// Фильтр уровней журнала приложения: [info, warn, error, debug].
    pub log_filter: [bool; 4],
    // --- Состояние страницы «Сборки» ---
    /// Список релизов llama.cpp (из кэша или сети).
    pub build_releases: Option<Vec<github::Release>>,
    /// Канал фонового обновления списка релизов.
    pub build_releases_rx: Option<mpsc::Receiver<Result<Vec<github::Release>, String>>>,
    pub build_releases_loading: bool,
    pub build_releases_error: Option<String>,
    /// Фильтр списка релизов по бэкенду (None = все).
    pub build_backend_filter: Option<github::Backend>,
    /// Показывать все релизы (по умолчанию — только 5 последних).
    pub build_show_all: bool,
    /// Текущее скачивание/установка сборки, если идёт.
    pub build_download: Option<BuildDownload>,
    /// Сборка, ожидающая подтверждения удаления (второй щелчок).
    pub build_delete_armed: Option<PathBuf>,
    // --- Состояние страницы «Модели» ---
    pub hf_query: String,
    pub hf_results: Option<Vec<huggingface::HfModel>>,
    pub hf_search_rx: Option<mpsc::Receiver<Result<Vec<huggingface::HfModel>, String>>>,
    pub hf_searching: bool,
    /// Репозиторий, выбранный из результатов поиска.
    pub hf_selected_repo: Option<String>,
    pub hf_files: Vec<huggingface::HfFile>,
    pub hf_files_rx: Option<mpsc::Receiver<Result<Vec<huggingface::HfFile>, String>>>,
    pub hf_error: Option<String>,
    /// Показывать HF-токен в открытом виде.
    pub hf_show_token: bool,
    /// Текущее скачивание файла модели, если идёт.
    pub model_download: Option<ModelDownload>,
    /// Файл модели, ожидающий подтверждения удаления (второй щелчок).
    pub model_delete_armed: Option<PathBuf>,
    /// Кэш прочитанного GGUF-заголовка для текущего пути модели.
    pub model_info_cache: Option<(PathBuf, Option<gguf::GgufInfo>)>,
    /// Кэш GGUF-заголовков для таблицы библиотеки моделей.
    pub gguf_cache: HashMap<PathBuf, Option<gguf::GgufInfo>>,
    /// Fresh catalog fetched in the background, picked up on the next frame.
    pub catalog_refresh: std::sync::Arc<std::sync::Mutex<Option<ParamsCatalog>>>,
}

/// Message from the background model file download thread.
pub(crate) enum ModelDownloadMsg {
    Progress(u64, u64),
    Done(Result<(), String>),
}

/// Состояние скачивания одного файла GGUF-модели.
pub struct ModelDownload {
    pub repo: String,
    pub path: String,
    pub downloaded: u64,
    pub total: u64,
    pub rx: mpsc::Receiver<ModelDownloadMsg>,
    pub error: Option<String>,
    /// Общий флаг отмены: выставляется из UI, читается в потоке скачивания.
    pub cancel: CancelFlag,
}

/// Message from the background build download thread.
pub(crate) enum BuildDownloadMsg {
    Progress(builds::Progress),
    Done(Result<(), String>),
}

/// Состояние скачивания и установки одной сборки llama.cpp.
pub struct BuildDownload {
    pub asset: github::BuildAsset,
    pub downloaded: u64,
    pub total: u64,
    pub extracting: bool,
    pub rx: mpsc::Receiver<BuildDownloadMsg>,
    /// Результат при ошибке (при успехе скачивание убирается сразу —
    /// сборка появляется в списке «Установленные сборки»).
    pub error: Option<String>,
    /// Общий флаг отмены: выставляется из UI, читается в потоке скачивания.
    pub cancel: CancelFlag,
}

/// Editable server launch parameters.
#[derive(Clone, Debug)]
pub struct ServerForm {
    pub binary: PathBuf,
    pub model: PathBuf,
    pub host: String,
    pub port: u16,
    pub extra_args: String,
    pub last_error: Option<String>,
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
    pub fn to_config(&self, extra_args: Vec<String>) -> ServerConfig {
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

        let mut app = Self {
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
            params_tab: 0,
            log_filter: [true; 4],
            build_releases: None,
            build_releases_rx: None,
            build_releases_loading: false,
            build_releases_error: None,
            build_backend_filter: None,
            build_show_all: false,
            build_download: None,
            build_delete_armed: None,
            hf_query: String::new(),
            hf_results: None,
            hf_search_rx: None,
            hf_searching: false,
            hf_selected_repo: None,
            hf_files: Vec::new(),
            hf_files_rx: None,
            hf_error: None,
            hf_show_token: false,
            model_download: None,
            model_delete_armed: None,
            model_info_cache: None,
            gguf_cache: HashMap::new(),
            catalog_refresh,
            settings,
            applied_theme: None,
            log_handle,
            config_dirty: false,
            last_change: None,
        };
        app.restore_last_preset();
        app
    }

    /// Re-apply the preset selected in the previous session, if it still exists.
    fn restore_last_preset(&mut self) {
        let Some(name) = self.settings.last_preset.clone() else {
            return;
        };
        match self.presets.load(&name) {
            Ok(preset) => {
                self.apply_preset(preset);
                self.selected_preset = Some(name.clone());
                self.mirrored_preset = Some(name.clone());
                self.preset_name_edit = name;
                log::info!("Восстановлен последний выбранный пресет");
            }
            Err(e) => {
                self.settings.last_preset = None;
                log::warn!("Не удалось восстановить последний пресет «{name}»: {e:#}");
            }
        }
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.config_dirty = true;
        self.last_change = Some(Instant::now());
    }

    pub(crate) fn save_settings(&mut self) {
        match config::save(&self.settings) {
            Ok(()) => {
                self.config_dirty = false;
                self.last_change = None;
                log::debug!("Настройки сохранены в {}", config::config_path().display());
            }
            Err(e) => log::error!("Не удалось сохранить настройки: {e:#}"),
        }
    }

    // -----------------------------------------------------------------------
    // Шелл: боковая панель и каркас страницы
    // -----------------------------------------------------------------------

    fn nav_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            if !self.sidebar_collapsed {
                ui.label(
                    egui::RichText::new("LlamaCpp Manager")
                        .size(15.0)
                        .strong()
                        .color(theme::ACCENT),
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
            ui.label(
                egui::RichText::new("локальный менеджер llama-server")
                    .size(11.0)
                    .color(crate::theme::MUTED),
            );
        }
        ui.add_space(16.0);

        for page in Page::ALL {
            let selected = self.page == page;
            let response = if self.sidebar_collapsed {
                // Узкий режим: первая буква названия (иконки в шрифте egui
                // покрыты не полностью — текст надёжнее).
                let letter = page.label().chars().next().unwrap_or('?').to_string();
                let button = egui::Button::new(egui::RichText::new(letter).size(14.0))
                    .selected(selected)
                    .min_size(egui::vec2(ui.available_width(), 32.0));
                ui.add(button).on_hover_text(page.title())
            } else {
                ui::nav_item(ui, selected, page.label())
            };
            if response.clicked() {
                self.page = page;
            }
            if !self.sidebar_collapsed {
                ui.add_space(2.0);
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(8.0);
            self.sidebar_server_block(ui);
        });
    }

    /// Карточка статуса сервера внизу сайдбара: индикатор + порт +
    /// кнопки Start/Stop/Restart, доступные с любого экрана.
    fn sidebar_server_block(&mut self, ui: &mut egui::Ui) {
        let collapsed = self.sidebar_collapsed;
        let state = self.server.state();
        let (color, hint) = ui::state_status(state);
        let running = self.server.is_running();
        let has_config = self.server.config().is_some();
        let port = self.server_form.port;

        if collapsed {
            ui.add_sized(
                [ui.available_width(), 20.0],
                egui::Label::new(egui::RichText::new("●").color(color).size(16.0)),
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
                    egui::Button::new(egui::RichText::new(icon).weak())
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
            // Компактная карточка: статус + порт в одну строку, кнопки — во
            // вторую, чтобы всё помещалось даже в узкий сайдбар.
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui::status_dot(ui, color, 4.0);
                    ui.label(egui::RichText::new(state.label()).size(12.5).strong())
                        .on_hover_text(hint);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(":{port}"))
                                .monospace()
                                .size(11.0)
                                .color(crate::theme::MUTED),
                        );
                    });
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.style_mut().spacing.item_spacing.x = 4.0;
                    if ui
                        .add_enabled(!running, egui::Button::new("▶ Запустить").small())
                        .on_hover_text("Запустить сервер")
                        .clicked()
                    {
                        self.try_start_server();
                    }
                    if ui
                        .add_enabled(running, egui::Button::new("■ Стоп").small())
                        .on_hover_text("Остановить сервер")
                        .clicked()
                    {
                        self.server.stop();
                    }
                    if ui
                        .add_enabled(has_config, egui::Button::new("↻").small())
                        .on_hover_text("Перезапустить сервер")
                        .clicked()
                    {
                        self.try_restart_server();
                    }
                });
                if let Some(error) = self.server_form.last_error.as_ref() {
                    ui.label(
                        egui::RichText::new(error)
                            .size(11.0)
                            .color(crate::theme::ERR_RED),
                    )
                    .on_hover_text(error);
                }
            });
        }
    }

    fn page_content(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui::screen_header(ui, self.page.title(), self.page.subtitle());
        ui.add_space(8.0);
        match self.page {
            Page::Server => ui::server::show(self, ui),
            Page::Models => ui::models::show(self, ui),
            Page::Builds => ui::builds::show(self, ui),
            Page::Logs => ui::logs::show(self, ui),
            Page::Settings => ui::settings::show(self, ui),
        }
        ui.add_space(16.0);
    }

    // -----------------------------------------------------------------------
    // Пресеты
    // -----------------------------------------------------------------------

    /// Синхронизация выбора пресета: при смене выделения мгновенно загружает
    /// пресет в форму (до дебаунс-автосохранения, чтобы не затереть его
    /// текущим состоянием формы). Вызывается из тулбара пресетов.
    pub fn sync_preset_selection(&mut self) {
        if self.selected_preset == self.mirrored_preset {
            return;
        }
        // Flush pending auto-save of the preset being switched away from,
        // so edits made less than a debounce ago are not lost.
        if self.config_dirty
            && let Some(previous) = self.mirrored_preset.clone()
            && let Err(e) = self.presets.save(&self.current_preset(previous.clone()))
        {
            self.set_preset_msg(true, format!("Не удалось автосохранить пресет: {e:#}"));
        }
        self.mirrored_preset = self.selected_preset.clone();
        self.preset_name_edit = self.selected_preset.clone().unwrap_or_default();
        self.preset_delete_armed = false;
        self.settings.last_preset = self.selected_preset.clone();
        self.mark_dirty();
        if let Some(name) = self.selected_preset.clone() {
            match self.presets.load(&name) {
                Ok(preset) => {
                    self.apply_preset(preset);
                    self.set_preset_msg(false, format!("Пресет «{name}» применён"));
                }
                Err(e) => {
                    self.selected_preset = None;
                    self.mirrored_preset = None;
                    self.preset_name_edit.clear();
                    self.set_preset_msg(true, format!("Не удалось загрузить пресет: {e:#}"));
                }
            }
        }
    }

    pub fn set_preset_msg(&mut self, is_error: bool, message: String) {
        if is_error {
            log::error!("Пресеты: {message}");
        } else {
            log::info!("Пресеты: {message}");
        }
        self.preset_msg = Some((is_error, message));
    }

    /// Snapshot of the current form + parameters as a preset.
    pub fn current_preset(&self, name: String) -> Preset {
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


    /// Сохранить текущую конфигурацию в выбранный пресет.
    pub fn save_into_selected_preset(&mut self) {
        let Some(name) = self.selected_preset.clone() else {
            return;
        };
        let preset = self.current_preset(name.clone());
        match self.presets.save(&preset) {
            Ok(()) => self.set_preset_msg(false, format!("Пресет «{name}» сохранён")),
            Err(e) => self.set_preset_msg(true, format!("Не удалось сохранить пресет: {e:#}")),
        }
    }

    pub fn save_current_as_preset(&mut self) {
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

    pub fn rename_selected_preset(&mut self) {
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
            Ok(renamed) => {
                match self
                    .presets
                    .save(&renamed)
                    .and_then(|()| self.presets.delete(&old_name))
                {
                    Ok(()) => {
                        self.selected_preset = Some(new_name.clone());
                        self.set_preset_msg(false, format!("«{old_name}» переименован в «{new_name}»"));
                    }
                    Err(e) => {
                        self.set_preset_msg(true, format!("Не удалось переименовать пресет: {e:#}"))
                    }
                }
            }
            Err(e) => self.set_preset_msg(true, format!("Не удалось переименовать пресет: {e:#}")),
        }
    }

    pub fn delete_selected_preset(&mut self) {
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

    pub fn export_selected_preset(&mut self) {
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
                        Ok(()) => {
                            self.set_preset_msg(false, format!("Пресет экспортирован: {}", path.display()))
                        }
                        Err(e) => {
                            self.set_preset_msg(true, format!("Не удалось экспортировать пресет: {e:#}"))
                        }
                    }
                }
            }
            Err(e) => self.set_preset_msg(true, format!("Не удалось прочитать пресет: {e:#}")),
        }
    }

    pub fn import_preset_file(&mut self) {
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
                        Err(e) => {
                            self.set_preset_msg(true, format!("Не удалось импортировать пресет: {e:#}"))
                        }
                    }
                }
                Err(e) => self.set_preset_msg(true, format!("Не удалось импортировать пресет: {e:#}")),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Запуск сервера
    // -----------------------------------------------------------------------

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

    pub fn try_start_server(&mut self) {
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

    pub fn try_restart_server(&mut self) {
        self.server_form.last_error = None;
        if let Err(e) = self.server.restart() {
            log::error!("Перезапуск не удался: {e}");
            self.server_form.last_error = Some(e);
        }
    }

    pub fn build_extra_args(&self) -> Vec<String> {
        let mut args = params::to_args(&self.params_catalog, &self.settings.params);
        if let Some(raw) = shlex::split(&self.server_form.extra_args) {
            args.extend(raw);
        }
        args
    }

    /// Сохранить журнал llama-server в файл через диалог.
    pub fn save_server_log(&mut self) {
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

    // -----------------------------------------------------------------------
    // Модели: поиск, скачивание, библиотека
    // -----------------------------------------------------------------------

    /// GGUF-заголовок файла библиотеки (с кэшем по пути).
    pub fn gguf_info_for(&mut self, path: &std::path::Path) -> Option<gguf::GgufInfo> {
        if !self.gguf_cache.contains_key(path) {
            let info = if path.is_file() {
                match gguf::read_header(path) {
                    Ok(info) => Some(info),
                    Err(e) => {
                        log::debug!("GGUF-заголовок {}: {e:#}", path.display());
                        None
                    }
                }
            } else {
                None
            };
            self.gguf_cache.insert(path.to_path_buf(), info);
        }
        self.gguf_cache.get(path).cloned().flatten()
    }

    pub fn is_downloading_model(&self, path: &str) -> bool {
        self.model_download
            .as_ref()
            .is_some_and(|download| download.path == path)
    }

    fn hf_token(&self) -> Option<&str> {
        let token = self.settings.hf_token.trim();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    }

    pub fn start_hf_search(&mut self) {
        if self.hf_searching {
            return;
        }
        let query = self.hf_query.trim().to_string();
        if query.is_empty() {
            self.hf_error = Some("Введите поисковый запрос".to_string());
            return;
        }
        self.hf_searching = true;
        self.hf_error = None;
        let (tx, rx) = mpsc::channel();
        let token = self.hf_token().map(str::to_string);
        let _ = std::thread::Builder::new()
            .name("hf-search".into())
            .spawn(move || {
                let result = huggingface::search_models(&query, token.as_deref())
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
            });
        self.hf_search_rx = Some(rx);
    }

    pub fn poll_hf_search(&mut self) {
        let received = self.hf_search_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(Ok(models))) => {
                self.hf_results = Some(models);
                self.hf_search_rx = None;
                self.hf_searching = false;
            }
            Some(Ok(Err(message))) => {
                self.hf_error = Some(message.clone());
                self.hf_search_rx = None;
                self.hf_searching = false;
                log::warn!("Поиск HuggingFace не удался: {message}");
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {}
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.hf_search_rx = None;
                self.hf_searching = false;
            }
            None => {}
        }
    }

    pub fn select_hf_repo(&mut self, repo: String) {
        if self.hf_selected_repo.as_deref() == Some(repo.as_str()) {
            return;
        }
        self.hf_selected_repo = Some(repo.clone());
        self.hf_files.clear();
        self.hf_files_rx = None;
        let (tx, rx) = mpsc::channel();
        let token = self.hf_token().map(str::to_string);
        let _ = std::thread::Builder::new()
            .name("hf-files".into())
            .spawn(move || {
                let result = huggingface::list_gguf_files(&repo, token.as_deref())
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
            });
        self.hf_files_rx = Some(rx);
    }

    pub fn poll_hf_files(&mut self) {
        let received = self.hf_files_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(Ok(files))) => {
                self.hf_files = files;
                self.hf_files_rx = None;
            }
            Some(Ok(Err(message))) => {
                self.hf_error = Some(message.clone());
                self.hf_files_rx = None;
                log::warn!("Не удалось получить файлы модели: {message}");
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {}
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.hf_files_rx = None;
            }
            None => {}
        }
    }

    pub fn start_model_download(&mut self, repo: String, file: huggingface::HfFile) {
        if self.model_download.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let dest = self.settings.models_dir.join(&file.path);
        let token = self.hf_token().map(str::to_string);
        let cancel = CancelFlag::new();
        let thread_cancel = cancel.clone();
        let thread_repo = repo.clone();
        let thread_path = file.path.clone();
        let _ = std::thread::Builder::new()
            .name("model-download".into())
            .spawn(move || {
                let outcome = huggingface::download_model_file(
                    &thread_repo,
                    &thread_path,
                    &dest,
                    token.as_deref(),
                    thread_cancel,
                    |downloaded, total| {
                        let _ = tx.send(ModelDownloadMsg::Progress(downloaded, total));
                    },
                )
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(ModelDownloadMsg::Done(outcome));
            });
        log::info!("Начато скачивание модели {repo}/{}", file.path);
        self.model_download = Some(ModelDownload {
            repo,
            path: file.path,
            downloaded: 0,
            total: file.size,
            rx,
            error: None,
            cancel,
        });
    }

    /// Отменить текущее скачивание модели (частичный файл сохраняется
    /// и докачается при следующем «Скачать» через Range).
    pub fn cancel_model_download(&mut self) {
        if let Some(download) = self.model_download.as_ref() {
            download.cancel.cancel();
            log::info!("Отмена скачивания модели {}/{}", download.repo, download.path);
        }
    }

    /// Отменить текущее скачивание сборки.
    pub fn cancel_build_download(&mut self) {
        if let Some(download) = self.build_download.as_ref() {
            download.cancel.cancel();
            log::info!("Отмена скачивания сборки {}", download.asset.asset.name);
        }
    }

    /// Подобрать сообщения из фонового потока скачивания модели.
    /// Успех убирает панель (файл появляется в библиотеке ниже).
    pub fn poll_model_download(&mut self) {
        let mut succeeded = false;
        {
            let Some(download) = self.model_download.as_mut() else {
                return;
            };
            loop {
                match download.rx.try_recv() {
                    Ok(ModelDownloadMsg::Progress(downloaded, total)) => {
                        download.downloaded = downloaded;
                        if total > 0 {
                            download.total = total;
                        }
                    }
                    Ok(ModelDownloadMsg::Done(Ok(()))) => {
                        log::info!("Модель {}/{} скачана", download.repo, download.path);
                        succeeded = true;
                        break;
                    }
                    Ok(ModelDownloadMsg::Done(Err(message))) => {
                        if download.cancel.is_cancelled() {
                            // Отмена из UI — не ошибка, панель закрывается тихо.
                            log::info!("Скачивание модели отменено");
                            succeeded = true;
                        } else {
                            download.error = Some(message.clone());
                            log::error!("Не удалось скачать модель: {message}");
                        }
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if download.error.is_none() {
                            download.error =
                                Some("поток скачивания завершился без результата".into());
                        }
                        break;
                    }
                }
            }
        }
        if succeeded {
            self.model_download = None;
        }
    }

    /// Удалить файл модели с диска. Если он был активной моделью — сбросить.
    pub fn delete_model_file(&mut self, path: &std::path::Path) {
        if self.server_form.model == path {
            self.server_form.model = PathBuf::new();
            self.mark_dirty();
            log::warn!("Удалённая модель была активной — файл модели сброшен, укажите другой");
        }
        match std::fs::remove_file(path) {
            Ok(()) => log::info!("Модель удалена: {}", path.display()),
            Err(e) => log::error!("Не удалось удалить {}: {e:#}", path.display()),
        }
    }

    /// Имена пресетов, использующих файл модели (как основную модель
    /// или как Path-параметр: draft/mmproj и т.п.).
    pub fn presets_using_model(&self, path: &std::path::Path) -> Vec<String> {
        fn value_is_path(value: &serde_json::Value, path: &std::path::Path) -> bool {
            value
                .as_str()
                .is_some_and(|s| std::path::Path::new(s) == path)
        }
        self.presets
            .list()
            .into_iter()
            .filter(|name| {
                self.presets.load(name).is_ok_and(|preset| {
                    preset.model == path
                        || preset
                            .params
                            .entries
                            .values()
                            .any(|entry| {
                                entry
                                    .value
                                    .as_ref()
                                    .is_some_and(|v| value_is_path(v, path))
                            })
                })
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Сборки llama.cpp
    // -----------------------------------------------------------------------

    /// Ассеты релизов для текущей ОС и архитектуры (в порядке релизов).
    fn current_os_assets(&self) -> Vec<github::BuildAsset> {
        let (current_os, current_arch) = (github::TargetOs::current(), github::Arch::current());
        self.build_releases
            .as_deref()
            .map(github::buildable_assets)
            .unwrap_or_default()
            .into_iter()
            .filter(|asset| asset.os == current_os && asset.arch == current_arch)
            .collect()
    }

    /// Соответствует ли сборка фильтру (тег или бэкенд).
    pub fn asset_matches_filter(&self, tag: &str, backend: github::Backend) -> bool {
        match self.build_backend_filter {
            Some(filter) => backend == filter,
            None => {
                // Фильтр «Все»: текстовый поиск по тегу оставлен для ручного
                // списка файлов, где бэкенд может быть неопознан.
                true || !tag.is_empty()
            }
        }
    }

    /// Ассеты текущей платформы с учётом фильтра и лимита релизов
    /// (по умолчанию — только 5 последних версий).
    pub(crate) fn visible_os_assets(&self) -> Vec<github::BuildAsset> {
        let filtered: Vec<_> = self
            .current_os_assets()
            .into_iter()
            .filter(|asset| self.asset_matches_filter(&asset.tag, asset.backend))
            .collect();
        if self.build_show_all {
            return filtered;
        }
        limit_releases(filtered, self.visible_release_count())
    }

    /// Сколько разных тегов осталось после фильтра (для подписи «ещё N»).
    pub fn total_filtered_tags(&self) -> usize {
        self.current_os_assets()
            .iter()
            .filter(|asset| self.asset_matches_filter(&asset.tag, asset.backend))
            .map(|asset| asset.tag.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    pub fn visible_release_count(&self) -> usize {
        5
    }

    /// Прочие файлы релизов (другие ОС/архитектуры) для ручного выбора,
    /// когда автоматическая классификация ничего не подобрала или нужен
    /// нестандартный вариант.
    pub fn manual_assets(&self, primary: &[github::BuildAsset]) -> Vec<(String, github::Asset)> {
        let mut out = Vec::new();
        for release in self.build_releases.as_deref().unwrap_or(&[]) {
            for asset in &release.assets {
                if primary.iter().any(|build| build.asset.name == asset.name) {
                    continue;
                }
                let lower = asset.name.to_ascii_lowercase();
                let is_server_archive = lower.starts_with("llama-")
                    && lower.contains("-bin-")
                    && (lower.ends_with(".zip") || lower.ends_with(".tar.gz"));
                if is_server_archive {
                    out.push((release.tag.clone(), asset.clone()));
                }
            }
        }
        out
    }

    /// Загружается ли сейчас указанный архив (кнопку этой сборки блокируем,
    /// остальные остаются доступны).
    pub fn is_downloading(&self, asset_name: &str) -> bool {
        self.build_download
            .as_ref()
            .is_some_and(|download| download.asset.asset.name == asset_name)
    }

    /// Запустить фоновую загрузку списка релизов (с кэшем на сутки).
    pub fn start_builds_refresh(&mut self, force: bool) {
        if self.build_releases_loading {
            return;
        }
        self.build_releases_loading = true;
        self.build_releases_error = None;
        let (tx, rx) = mpsc::channel();
        let cache_dir = self.settings.builds_dir.clone();
        let _ = std::thread::Builder::new()
            .name("builds-refresh".into())
            .spawn(move || {
                let result = builds::fetch_releases_cached(&cache_dir, force)
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
            });
        self.build_releases_rx = Some(rx);
    }

    /// Подобрать результат фоновой загрузки списка релизов.
    pub fn poll_builds_refresh(&mut self) {
        let received = self.build_releases_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(Ok(releases))) => {
                self.build_releases = Some(releases);
                self.build_releases_rx = None;
                self.build_releases_loading = false;
                log::info!("Список релизов llama.cpp получен");
            }
            Some(Ok(Err(message))) => {
                self.build_releases_error = Some(message.clone());
                self.build_releases_rx = None;
                self.build_releases_loading = false;
                log::warn!("Не удалось получить список релизов: {message}");
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {}
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.build_releases_rx = None;
                self.build_releases_loading = false;
            }
            None => {}
        }
    }

    /// Запустить скачивание и установку сборки в фоновом потоке.
    pub fn start_build_download(&mut self, asset: github::BuildAsset) {
        if self.build_download.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let store = builds::BuildsStore::new(self.settings.builds_dir.clone());
        let size = asset.asset.size;
        let name = asset.asset.name.clone();
        let tag = asset.tag.clone();
        let thread_asset = asset.clone();
        let cancel = CancelFlag::new();
        let thread_cancel = cancel.clone();
        let _ = std::thread::Builder::new()
            .name("build-download".into())
            .spawn(move || {
                let outcome = store.install(&thread_asset, thread_cancel, |progress| {
                    let _ = tx.send(BuildDownloadMsg::Progress(progress));
                });
                let _ = tx.send(BuildDownloadMsg::Done(
                    outcome.map(|_| ()).map_err(|e| format!("{e:#}")),
                ));
            });
        log::info!("Начато скачивание сборки {tag}: {name}");
        self.build_download = Some(BuildDownload {
            asset,
            downloaded: 0,
            total: size,
            extracting: false,
            rx,
            error: None,
            cancel,
        });
    }

    /// Подобрать сообщения из фонового потока скачивания сборки.
    /// Успех убирает скачивание с экрана (сборка видна в «Установленных»).
    pub fn poll_build_download(&mut self) {
        let mut succeeded = false;
        {
            let Some(download) = self.build_download.as_mut() else {
                return;
            };
            loop {
                match download.rx.try_recv() {
                    Ok(BuildDownloadMsg::Progress(builds::Progress::Downloading {
                        downloaded,
                        total,
                    })) => {
                        download.downloaded = downloaded;
                        if total > 0 {
                            download.total = total;
                        }
                    }
                    Ok(BuildDownloadMsg::Progress(builds::Progress::Extracting)) => {
                        download.extracting = true;
                    }
                    Ok(BuildDownloadMsg::Done(Ok(()))) => {
                        log::info!("Сборка {} установлена", download.asset.asset.name);
                        succeeded = true;
                        break;
                    }
                    Ok(BuildDownloadMsg::Done(Err(message))) => {
                        if download.cancel.is_cancelled() {
                            // Отмена из UI — не ошибка, панель закрывается тихо.
                            log::info!("Скачивание сборки отменено");
                            succeeded = true;
                        } else {
                            download.error = Some(message.clone());
                            log::error!("Не удалось установить сборку: {message}");
                        }
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if download.error.is_none() {
                            download.error =
                                Some("поток скачивания завершился без результата".into());
                        }
                        break;
                    }
                }
            }
        }
        if succeeded {
            self.build_download = None;
        }
    }

    /// Сделать llama-server из установленной сборки активным бинарником.
    pub fn activate_build_binary(&mut self, tag: String, binary: PathBuf) {
        if self.server_form.binary != binary {
            self.server_form.binary = binary.clone();
            self.mark_dirty();
        }
        log::info!("Активная сборка: {tag} ({})", binary.display());
    }

    /// Имена пресетов, чей бинарник лежит внутри каталога сборки.
    pub fn presets_using_dir(&self, dir: &std::path::Path) -> Vec<String> {
        self.presets
            .list()
            .into_iter()
            .filter(|name| {
                self.presets
                    .load(name)
                    .is_ok_and(|preset| preset.binary.starts_with(dir))
            })
            .collect()
    }

    /// Удалить установленную сборку с диска. Если её бинарник был активным —
    /// сбросить конфигурацию; о пресетах пользователь предупреждён заранее.
    pub fn delete_build(&mut self, build: &builds::InstalledBuild) {
        if self.server_form.binary.starts_with(&build.dir) {
            self.server_form.binary = PathBuf::new();
            self.mark_dirty();
            log::warn!(
                "Удалённая сборка была активной — бинарник сервера сброшен, укажите другой"
            );
        }
        match std::fs::remove_dir_all(&build.dir) {
            Ok(()) => log::info!("Сборка {} удалена ({})", build.label(), build.dir.display()),
            Err(e) => log::error!(
                "Не удалось удалить {}: {e:#}",
                build.dir.display()
            ),
        }
    }

    // -----------------------------------------------------------------------
    // GGUF-метаданные и оценка памяти
    // -----------------------------------------------------------------------

    /// Прочитать GGUF-заголовок текущей модели (с кэшем по пути).
    pub fn cached_gguf_info(&mut self) -> Option<(PathBuf, Option<gguf::GgufInfo>)> {
        let path = self.server_form.model.clone();
        if path.as_os_str().is_empty() {
            return None;
        }
        if self
            .model_info_cache
            .as_ref()
            .is_some_and(|(cached, _)| *cached == path)
        {
            return self
                .model_info_cache
                .as_ref()
                .map(|(p, info)| (p.clone(), info.clone()));
        }
        let info = if path.is_file() {
            match gguf::read_header(&path) {
                Ok(info) => Some(info),
                Err(e) => {
                    log::debug!("GGUF-заголовок {}: {e:#}", path.display());
                    None
                }
            }
        } else {
            None
        };
        self.model_info_cache = Some((path.clone(), info.clone()));
        Some((path, info))
    }

    fn param_u64(&self, id: &str) -> Option<u64> {
        if !self.settings.params.is_enabled(id) {
            return None;
        }
        self.settings
            .params
            .entries
            .get(id)
            .and_then(|e| e.value.as_ref())
            .and_then(|v| v.as_i64())
            .map(|v| v.max(0) as u64)
    }

    fn param_string(&self, id: &str) -> Option<String> {
        if !self.settings.params.is_enabled(id) {
            return None;
        }
        self.settings
            .params
            .entries
            .get(id)
            .and_then(|e| e.value.as_ref())
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    /// Контекст из параметров сервера (если включён).
    fn ctx_from_params(&self) -> Option<u64> {
        self.param_u64("ctx-size")
    }

    /// Байт на элемент KV-кэша с учётом cache-type-k/v (по половине кэша на K и V).
    fn kv_elem_bytes(&self) -> f64 {
        let cache_type_bytes = |id: &str| match self.param_string(id).as_deref() {
            Some(t) if t.eq_ignore_ascii_case("q8_0") => 1.0,
            Some(t) if t.eq_ignore_ascii_case("q4_0") => 0.5,
            _ => 2.0, // f16 по умолчанию
        };
        (cache_type_bytes("cache-type-k") + cache_type_bytes("cache-type-v")) / 2.0
    }

    /// Оценка памяти для текущей модели: (модель, оценка).
    pub fn memory_estimate(&mut self) -> Option<(gguf::GgufInfo, gguf::MemoryEstimate)> {
        let (path, info) = self.cached_gguf_info()?;
        let info = info?;
        let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let gpu_offload_all = self
            .param_u64("ngl")
            .is_some_and(|ngl| info.n_layers.is_some_and(|layers| ngl >= layers) || ngl >= 999);
        let estimate =
            gguf::estimate_memory(&info, file_size, self.ctx_from_params(), self.kv_elem_bytes(), gpu_offload_all)?;
        Some((info, estimate))
    }
}

/// GGUF-файлы в библиотеке моделей (в каталоге и на один уровень вглубь),
/// отсортированные по имени.
pub fn library_models(models_dir: &std::path::Path) -> Vec<PathBuf> {
    fn gguf(p: &std::path::Path) -> bool {
        p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("gguf")) == Some(true)
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(models_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && gguf(&path) {
            out.push(path);
        } else if path.is_dir()
            && let Ok(nested) = std::fs::read_dir(&path)
        {
            for sub in nested.flatten() {
                let sub_path = sub.path();
                if sub_path.is_file() && gguf(&sub_path) {
                    out.push(sub_path);
                }
            }
        }
    }
    out.sort();
    out
}

/// Обрезать список ассетов до первых `max_tags` разных тегов релизов
/// (релизы идут от новых к старым).
fn limit_releases(assets: Vec<github::BuildAsset>, max_tags: usize) -> Vec<github::BuildAsset> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    assets
        .into_iter()
        .filter(|asset| {
            if seen.len() < max_tags || seen.contains(&asset.tag) {
                seen.insert(asset.tag.clone());
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset_with_tag(tag: &str) -> github::BuildAsset {
        github::BuildAsset {
            asset: github::Asset {
                name: format!("llama-{tag}-bin-ubuntu-x64.tar.gz"),
                browser_download_url: "https://unused".into(),
                size: 1,
            },
            tag: tag.into(),
            os: Some(github::TargetOs::Linux),
            arch: Some(github::Arch::X64),
            backend: github::Backend::Cpu,
            runtime_asset: None,
        }
    }

    #[test]
    fn limit_releases_keeps_all_assets_of_first_tags() {
        let mut assets = Vec::new();
        for tag in ["b90", "b89", "b88", "b87", "b86", "b85", "b84"] {
            assets.push(asset_with_tag(tag));
            assets.push(asset_with_tag(tag)); // по два бэкенда на релиз
        }
        let limited = limit_releases(assets.clone(), 5);
        // Первые 5 тегов целиком (10 ассетов), остальные 4 — отброшены.
        assert_eq!(limited.len(), 10);
        assert!(limited.iter().all(|a| !["b85", "b84"].contains(&a.tag.as_str())));

        assert_eq!(limit_releases(assets, 10).len(), 14);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.applied_theme != Some(self.settings.theme) {
            theme::apply(&ui.ctx().clone(), self.settings.theme);
            self.applied_theme = Some(self.settings.theme);
        }

        // Масштаб интерфейса (0.8x–1.5x).
        if (ui.ctx().zoom_factor() - self.settings.ui_zoom).abs() > 0.001 {
            ui.ctx().set_zoom_factor(self.settings.ui_zoom);
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

        let collapsed = self.sidebar_collapsed;
        let panel = egui::Panel::left("nav");
        let panel = if collapsed {
            panel.exact_size(56.0)
        } else {
            panel
                .resizable(true)
                .default_size(230.0)
                .size_range(180.0..=360.0)
        };
        panel.show(ui, |ui| self.nav_panel(ui));

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
            // Flush pending auto-save into the selected preset too.
            if let Some(name) = self.selected_preset.clone()
                && let Err(e) = self.presets.save(&self.current_preset(name.clone()))
            {
                log::error!("Не удалось автосохранить пресет «{name}» при выходе: {e:#}");
            }
            self.save_settings();
        }
    }
}
