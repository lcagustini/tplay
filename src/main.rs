#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod gui;
mod library;
mod network;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPlay")
            .with_inner_size([680.0, 460.0])
            .with_min_inner_size([320.0, 160.0])
            // No native title bar: the controls live in the app's own top bar
            // (right-aligned next to the logo), and the bar itself is draggable.
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "TPlay",
        native_options,
        Box::new(|cc| {
            // App-wide text fallback: a system font appended after egui's
            // bundled fonts so any Unicode in tags renders (no tofu boxes).
            gui::theme::install_fallback_fonts(&cc.egui_ctx, gui::theme::SYSTEM_FONT_CANDIDATES);
            Ok(Box::new(app::TPlayApp::new(cc)))
        }),
    )
}
