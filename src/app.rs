use egui::{RichText, ScrollArea};

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
    theme_mode: ThemeMode,
    applied_theme: Option<ThemeMode>,
    sidebar_collapsed: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            page: Page::Server,
            theme_mode: ThemeMode::System,
            applied_theme: None,
            sidebar_collapsed: false,
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
                        .selectable_label(self.theme_mode == mode, mode.label())
                        .clicked()
                    {
                        self.theme_mode = mode;
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
        ui.label("Экран управления сервером будет здесь: запуск/остановка llama-server, логи, статус.");
    }

    fn models_page(&mut self, ui: &mut egui::Ui) {
        ui.label("Экран моделей будет здесь: поиск и скачивание GGUF с HuggingFace.");
    }

    fn builds_page(&mut self, ui: &mut egui::Ui) {
        ui.label("Экран сборок будет здесь: релизы llama.cpp и выбор бэкенда.");
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.label("Экран настроек будет здесь: пути, HF-токен, темы, debug-логи.");
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.applied_theme != Some(self.theme_mode) {
            theme::apply(&ui.ctx().clone(), self.theme_mode);
            self.applied_theme = Some(self.theme_mode);
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
}
