use crate::app::{EQ_PRESETS, TPlayApp};
use crate::audio::eq::EQ_FREQUENCIES;
use eframe::egui;

fn format_freq(f: f32) -> String {
    if f >= 1000.0 {
        format!("{}K", (f / 1000.0) as i32)
    } else {
        format!("{}", f as i32)
    }
}

pub fn equalizer_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    // Owned Arc copy — panes call &mut app while using theme data.
    let theme = app.theme().clone();
    let p = theme.palette;
    let gains = *app.eq_gains();
    let has_track = app.total_duration().is_some();

    // Header — left: ON/AUTO toggles; center: title; right: preset selector.
    let title_text = egui::RichText::new("EQUALIZER")
        .strong()
        .size(14.0)
        .color(p.accent)
        .font(egui::FontId::new(14.0, theme.metadata_font.clone()));
    // Bound the header's height: ui.columns() spans the FULL remaining pane
    // height, which would leave zero space for the band row below it.
    ui.allocate_ui(egui::vec2(ui.available_width(), 24.0), |ui| {
        ui.columns(3, |cols| {
        cols[0].horizontal(|ui| {
            let on = app.eq_enabled();
            if ui.selectable_label(on, "ON").clicked() {
                app.toggle_eq();
            }
            let auto = app.eq_auto();
            if ui.selectable_label(auto, "AUTO").clicked() {
                app.toggle_eq_auto();
            }
        });
        cols[1].vertical_centered(|ui| {
            ui.label(title_text.clone());
        });
        cols[2].with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut sel = app.eq_preset();
            egui::ComboBox::from_id_salt("tplay.eq.preset")
                .selected_text(app.eq_preset_name())
                .show_ui(ui, |ui| {
                    for (i, (name, _)) in EQ_PRESETS.iter().enumerate() {
                        ui.selectable_value(&mut sel, i, *name);
                    }
                    ui.selectable_value(&mut sel, crate::app::EQ_PRESET_CUSTOM, "Custom");
                });
            if sel != app.eq_preset() {
                app.set_eq_preset(sel);
            }
            if ui.button("Reset").clicked() {
                for i in 0..10 {
                    app.set_eq_gain(i, 0.0);
                }
            }
        });
    });
    });

    ui.add_space(6.0);
    if !has_track {
        ui.label("(No track loaded)");
    }

    // 10 bands with fixed inter-band spacing, centered in the pane.
    // The gap between sliders is constant; the margins to the pane edges
    // absorb all leftover width equally (dynamic centering).
    let slider_h = (ui.available_height() - 30.0).clamp(60.0, 220.0);
    // Fixed band width and spacing normally; shrink to fit narrow panes.
    let min_spacing = ui.spacing().item_spacing.x;
    let avail_w = ui.available_width();
    let band_w_max = 80.0;
    let row_w_full = band_w_max * 10.0 + min_spacing * 9.0;
    let (band_w, spacing) = if row_w_full <= avail_w {
        (band_w_max, min_spacing)
    } else if band_w_max * 10.0 <= avail_w {
        // Enough room for bands, shrink spacing to fit.
        (band_w_max, ((avail_w - band_w_max * 10.0) / 9.0).max(0.0))
    } else {
        // Not enough room even at zero spacing — shrink bands too.
        ((avail_w / 10.0).max(40.0), 0.0)
    };
    let row_w = band_w * 10.0 + spacing * 9.0;
    let pad = ((avail_w - row_w) / 2.0).max(0.0);

    ui.horizontal(|ui| {
        if pad > 0.0 {
            ui.add_space(pad);
        }
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
                    ui.label(
                        egui::RichText::new(format_freq(EQ_FREQUENCIES[i]))
                            .small()
                            .color(p.text_secondary)
                            .font(egui::FontId::new(10.0, theme.metadata_font.clone())),
                    );
                });
            });
        }
    });
}