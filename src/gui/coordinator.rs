//! Coordinator — wires the panes together in one frame.

use crate::app::TPlayApp;
use crate::gui::panes;
use eframe::egui;

/// Update the UI for one frame. Called from TPlayApp::update().
pub fn update_ui(app: &mut TPlayApp, ctx: &egui::Context) {
    egui::CentralPanel::default().show(ctx, |ui| {
        panes::title::title_pane(app, ui);
        ui.separator();
        panes::seekbar::seekbar_pane(app, ui);
        ui.separator();
        panes::transport::transport_pane(app, ui);
        ui.separator();
        panes::playlist::playlist_pane(app, ui);
    });
}