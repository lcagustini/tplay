use crate::app::TPlayApp;
use eframe::egui;

/// egui Id for storing the seek slider position in memory
fn seek_id() -> egui::Id {
    egui::Id::new("tplay.seek")
}

pub fn now_playing_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        // Track title
        ui.horizontal(|ui| {
            ui.label(app.track_title());
        });

        ui.add_space(8.0);

        // Seek bar with time labels
        ui.horizontal(|ui| {
            let total_secs = app.total_duration().map(|d| d.as_secs_f32());
            let actual_ratio = app.playback_position();

            let pos_str = TPlayApp::fmt_duration(app.playback_position_secs());
            let total_str = app.total_duration()
                .map(TPlayApp::fmt_duration)
                .unwrap_or_else(|| "--:--".into());

            let mut seek_normalized = ui.ctx().memory_mut(|m| m.data.get_temp::<f32>(seek_id()).unwrap_or(0.0));

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

            ui.ctx().memory_mut(|m| m.data.insert_temp(seek_id(), seek_normalized));
        });

        ui.add_space(8.0);

        // Controls + Volume
        ui.horizontal(|ui| {
            // Prev track
            let prev_enabled = app.has_prev_track();
            if ui.add_enabled(prev_enabled, egui::Button::new("⏮")).clicked() {
                app.prev_track();
            }

            // Play/Pause/Stop
            let is_paused = app.is_paused();
            let is_empty = app.is_empty();
            if is_paused || is_empty {
                if ui.button("▶").clicked() {
                    app.play();
                }
            } else if ui.button("⏸").clicked() {
                app.pause();
            }

            if ui.button("⏹").clicked() {
                app.stop();
            }

            // Next track
            let next_enabled = app.has_next_track();
            if ui.add_enabled(next_enabled, egui::Button::new("⏭")).clicked() {
                app.next_track();
            }

            ui.separator();

            // Volume
            ui.label("🔊");
            let mut volume = app.volume();
            if ui.add(egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false)).changed() {
                app.set_volume(volume);
            }
        });
    });
}