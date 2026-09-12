#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod gui;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPlay")
            .with_inner_size([320.0, 130.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "TPlay",
        native_options,
        Box::new(|cc| Ok(Box::new(gui::TPlayApp::new(cc)))),
    )
}
