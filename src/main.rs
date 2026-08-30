mod app;
mod config;
mod github;
mod logger;
mod params;
mod presets;
mod process_mgr;
mod theme;

use app::App;

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("LlamaCpp Manager")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([960.0, 640.0]);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "LlamaCpp Manager",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
