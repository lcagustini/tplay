use crate::app::TPlayApp;
use eframe::egui;

/// egui Id for storing the seek slider position in memory
const SEEK_ID: egui::Id = egui::Id::NULL;

pub fn seekbar_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let total_secs = app.total_duration().map(|d| d.as_secs_f32());
    let actual_ratio = app.playback_position();

    let pos_str = TPlayApp::fmt_duration(app.playback_position_secs());
    let total_str = app.total_duration()
        .map(TPlayApp::fmt_duration)
        .unwrap_or_else(|| "--:--".into());

    let mut seek_normalized = ui.ctx().memory_mut(|m| m.data.get_temp::<f32>(SEEK_ID).unwrap_or(0.0));

    ui.horizontal(|ui| {
        ui.label(pos_str);

        let bar = ui.add_enabled(
            total_secs.is_some(),
            egui::Slider::new(&mut seek_normalized, 0.0..=1.0).show_value(false),
        );

        if bar.dragged() {
            // hold
        } else if bar.drag_stopped() || bar.clicked() {
            app.seek(seek_normalized);
        } else {
            let target_reached = app.seek_target_reached().unwrap_or(true);
            if target_reached {
                app.clear_seek_target();
                if total_secs.is_some() {
                    seek_normalized = actual_ratio;
                }
            }
        }

        ui.label(total_str);
    });

    ui.ctx().memory_mut(|m| m.data.insert_temp(SEEK_ID, seek_normalized));
}