use crate::app::TPlayApp;
use eframe::egui;

pub fn transport_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let prev_enabled = app.has_prev_track();
        if ui.add_enabled(prev_enabled, egui::Button::new("⏮")).clicked() {
            app.prev_track();
        }

        let is_paused = app.is_paused();
        let is_empty  = app.is_empty();
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

        let next_enabled = app.has_next_track();
        if ui.add_enabled(next_enabled, egui::Button::new("⏭")).clicked() {
            app.next_track();
        }

        ui.label("🔊");
        let mut volume = app.volume();
        if ui.add(egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false)).changed() {
            app.set_volume(volume);
        }
    });
}