use crate::app::TPlayApp;
use eframe::egui;

const EQ_FREQUENCIES: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

fn format_freq(f: f32) -> String {
    if f >= 1000.0 {
        format!("{:.0}k", f / 1000.0)
    } else {
        format!("{:.0}", f)
    }
}

pub fn equalizer_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let enabled = app.eq_enabled();
    let gains = *app.eq_gains();
    let has_track = app.total_duration().is_some();

    ui.horizontal(|ui| {
        let mut eq_enabled = enabled;
        if ui.checkbox(&mut eq_enabled, "Enable EQ").changed() {
            app.toggle_eq();
        }
        if ui.button("Reset").clicked() {
            for i in 0..10 {
                app.set_eq_gain(i, 0.0);
            }
        }
        if !has_track {
            ui.label("(No track loaded)");
        }
    });

    ui.add_space(8.0);

    // Spread the 10 bands across the full pane width: content always fits, so
    // nothing overflows and no scrollbar can appear. slider_width is the long
    // axis for vertical sliders (their track length).
    let spacing = 6.0;
    let band_w = ((ui.available_width() - spacing * 9.0) / 10.0).max(24.0);
    let slider_h = (ui.available_height() - 30.0).clamp(60.0, 220.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = spacing;

        for i in 0..10 {
            let mut gain = gains[i];

            ui.vertical(|ui| {
                ui.set_width(band_w);
                ui.spacing_mut().slider_width = slider_h;
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.push_id(i, |ui| {
                        let slider = egui::Slider::new(&mut gain, -12.0..=12.0)
                            .show_value(true)
                            .vertical()
                            .trailing_fill(true);
                        let resp = ui.add(slider);
                        if resp.changed() {
                            app.set_eq_gain(i, gain);
                        }
                    });
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(format_freq(EQ_FREQUENCIES[i])).small());
                });
            });
        }
    });
}