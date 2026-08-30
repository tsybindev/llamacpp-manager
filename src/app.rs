use egui::{RichText, ScrollArea};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Server,
    Models,
    Builds,
    Settings,
}

impl Page {
    const ALL: [Page; 4] = [Page::Server, Page::Models, Page::Builds, Page::Settings];

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
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self { page: Page::Server }
    }

    fn nav_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.heading(RichText::new("LlamaCpp Manager").color(crate::theme::ACCENT).size(18.0));
        ui.small("локальный менеджер llama-server");
        ui.add_space(16.0);

        for page in Page::ALL {
            let selected = self.page == page;
            if ui
                .selectable_label(selected, RichText::new(page.label()).size(15.0))
                .clicked()
            {
                self.page = page;
            }
            ui.add_space(2.0);
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
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(200.0)
            .show(ui, |ui| self.nav_panel(ui));

        egui::CentralPanel::default().show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| self.page_content(ui));
        });
    }
}
