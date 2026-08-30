use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, ScrollArea};

use crate::config::{self, Settings};
use crate::logger::{self, LogHandle, LogEntry};
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
    fn to_config(&self) -> ServerConfig {
        ServerConfig {
            binary: self.binary.clone(),
            model: self.model.clone(),
            host: self.host.trim().to_string(),
            port: self.port,
            extra_args: shlex::split(&self.extra_args).unwrap_or_default(),
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
        if !config::config_path().exists() {
            if let Err(e) = config::save(&settings) {
                eprintln!("не удалось сохранить настройки по умолчанию: {e:#}");
            }
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

        Self {
            page: Page::Server,
            sidebar_collapsed: settings.sidebar_collapsed,
            server: ServerManager::new(settings.logs_dir.clone(), settings.auto_restore.clone()),
            server_form: ServerForm::default(),
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
            if !self.sidebar_collapsed {
                ui.separator();
                ui.add_space(4.0);
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
            }
        });
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
        self.server_config_section(ui);
        ui.add_space(12.0);
        self.server_log_panel(ui);
        ui.add_space(12.0);
        self.app_log_panel(ui);
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
        let (color, hint) = match state {
            ServerState::Stopped => (Color32::from_rgb(0x8A, 0x94, 0xA6), "Сервер не запущен"),
            ServerState::Starting => (theme::WARN_YELLOW, "Идёт загрузка модели, проверяется готовность…"),
            ServerState::Ready => (theme::OK_GREEN, "Сервер отвечает и готов принимать запросы"),
            ServerState::RestartScheduled => (theme::WARN_YELLOW, "Процесс упал, ожидается автоматический перезапуск"),
            ServerState::Crashed => (theme::ERR_RED, "Требуется вмешательство пользователя"),
        };
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
    }

    fn server_config_section(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("Конфигурация").size(16.0));
        let mut paths_changed = false;
        egui::Grid::new("server_grid")
            .num_columns(3)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let form = &mut self.server_form;
                paths_changed |= path_row(ui, "Бинарник llama-server", &mut form.binary, "путь к llama-server");
                paths_changed |= path_row(ui, "Файл модели", &mut form.model, "путь к GGUF-файлу модели");
                ui.label("Host").on_hover_text("127.0.0.1 — только локально; 0.0.0.0 — доступ из сети (опасно!)");
                let host_response = ui.add(
                    egui::TextEdit::singleline(&mut form.host).desired_width(160.0),
                );
                if host_response.changed() {
                    // normalise on leave only; keep raw while typing
                }
                ui.label("");
                ui.end_row();
                ui.label("Порт");
                ui.add(
                    egui::DragValue::new(&mut form.port)
                        .range(1..=65535)
                        .custom_formatter(|v, _| format!("{}", v as u16))
                        .custom_parser(|s| s.parse::<u16>().ok().map(f64::from)),
                );
                ui.label("");
                ui.end_row();
            });
        if paths_changed {
            // Paths live in the form, not settings — nothing to persist yet.
        }

        if self.server_form.host.trim() == "0.0.0.0" {
            ui.label(
                RichText::new("⚠ Host 0.0.0.0 делает сервер доступным из сети. Убедитесь, что это безопасно.")
                    .small()
                    .color(theme::WARN_YELLOW),
            );
        }

        ui.add_space(4.0);
        ui.label("Дополнительные аргументы").on_hover_text("Разделяйте пробелом; кавычки поддерживаются. Каталог параметров с описаниями появится позже.");
        let response = ui.add(
            egui::TextEdit::multiline(&mut self.server_form.extra_args)
                .desired_rows(2)
                .desired_width(560.0)
                .code_editor(),
        );
        let _ = response;

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Команда запуска:\n{}",
                self.server_form.to_config().command_line()
            ))
            .monospace()
            .small(),
        );

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let running = self.server.is_running();
            if ui.add_enabled(!running, egui::Button::new(RichText::new("▶ Запустить").color(theme::OK_GREEN)))
                .clicked()
            {
                self.server_form.last_error = None;
                let config = self.server_form.to_config();
                if let Err(e) = self.pre_flight_check(&config).and_then(|()| self.server.start(config)) {
                    self.server_form.last_error = Some(e);
                }
            }
            if ui
                .add_enabled(running, egui::Button::new("■ Остановить"))
                .clicked()
            {
                self.server.stop();
            }
            if ui
                .add_enabled(self.server.config().is_some(), egui::Button::new("↻ Перезапустить"))
                .clicked()
            {
                self.server_form.last_error = None;
                if let Err(e) = self.server.restart() {
                    self.server_form.last_error = Some(e);
                }
            }
            if let Some(error) = &self.server_form.last_error {
                ui.label(RichText::new(error).color(theme::ERR_RED).small());
            }
        });
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

        let paths_changed = {
            let mut changed = false;
            egui::Grid::new("paths_grid")
                .num_columns(3)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    let s = &mut self.settings;
                    changed |= path_row(ui, "Каталог моделей", &mut s.models_dir, "куда скачиваются GGUF-модели");
                    changed |= path_row(ui, "Каталог сборок", &mut s.builds_dir, "куда скачиваются бинарники llama.cpp");
                    changed |= path_row(ui, "Каталог логов", &mut s.logs_dir, "куда пишутся журналы");
                });
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
                .desired_width(420.0),
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

/// One "path picker" row: label, editable path, browse button. Returns true if changed.
fn path_row(ui: &mut egui::Ui, label: &str, path: &mut PathBuf, hint: &str) -> bool {
    let mut changed = false;
    ui.label(label).on_hover_text(hint);
    let mut text = path.display().to_string();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .desired_width(340.0),
    );
    if response.changed() {
        *path = PathBuf::from(text.trim());
        changed = true;
    }
    if ui.button("Обзор…").clicked()
        && let Some(dir) = rfd::FileDialog::new()
            .set_title(format!("Выберите: {label}"))
            .pick_folder()
    {
        *path = dir;
        changed = true;
    }
    ui.end_row();
    changed
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

        // Debounced auto-save: write at most twice per second while editing.
        if self.config_dirty
            && self
                .last_change
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(500))
        {
            self.save_settings();
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
