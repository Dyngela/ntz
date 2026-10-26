mod app;
mod cmder;
mod widgets;

pub use eframe::Error;

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ntz")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "ntz",
        options,
        Box::new(|cc| Ok(Box::new(app::NtzApp::new(cc)))),
    )
}
