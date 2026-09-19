use crate::app::TPlayApp;
use eframe::egui;

pub fn title_pane(app: &TPlayApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(app.track_title());
    });
}