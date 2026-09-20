#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod gui;
mod library;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPlay")
            .with_inner_size([680.0, 460.0])
            .with_resizable(true)
            .with_min_inner_size([320.0, 160.0])
            // No native title bar: the controls live in the app's own top bar
            // (right-aligned next to the logo), and the bar itself is draggable.
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "TPlay",
        native_options,
        Box::new(|cc| Ok(Box::new(app::TPlayApp::new(cc)))),
    )
}
