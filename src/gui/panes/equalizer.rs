use crate::app::{Pane, TPlayApp};
use crate::audio::eq::{EQ_FREQUENCIES, EQ_PRESETS};
use eframe::egui;

pub fn equalizer_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    // Owned Arc copy — panes take `&mut app` while using theme data.
    let theme = app.theme().clone();
    let p = theme.palette;
    let gains = app.eq_gains();
    let layout = theme.layout.with_defaults();

    // Header — grouped controls; the tab already names the pane. Measured via
    // a scope so `min_content_h` below is exact, not guessed.
    let header_h = ui
        .scope(|ui| {
            ui.horizontal(|ui| {
                let on = app.eq_enabled();
                if ui.add(egui::Button::new("ON").selected(on)).clicked() {
                    app.toggle_eq();
                }
                if ui.button("Reset").clicked() {
                    for i in 0..10 {
                        app.set_eq_gain(i, 0.0);
                    }
                }
                let mut sel = app.eq_preset().map(|s| s.to_string());
                egui::ComboBox::from_id_salt("tplay.eq.preset")
                    .selected_text(app.eq_preset_name())
                    .show_ui(ui, |ui| {
                        for (name, _) in EQ_PRESETS {
                            ui.selectable_value(&mut sel, Some(name.to_string()), name);
                        }
                        ui.selectable_value(&mut sel, None, "Custom");
                    });
                if sel != app.eq_preset().map(|s| s.to_string()) {
                    app.set_eq_preset(sel);
                }
            });
        })
        .response
        .rect
        .height();

    // No scrollbars (see `scroll_bars` in coordinator), so the content must
    // always fit. Record the smallest height at which nothing clips — header +
    // gaps + sliders at their floor + a band label — and the coordinator keeps
    // the dock split at least this tall.
    let label_h = ui.fonts(|f| {
        f.layout_no_wrap(
            TPlayApp::format_freq(EQ_FREQUENCIES[0]),
            egui::FontId::new(layout.text_meta, theme.metadata_font.clone()),
            p.text_secondary,
        )
        .size()
        .y
    });
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("tplay.pane_content_h").with(Pane::Equalizer),
            header_h + layout.eq_header_gap + layout.eq_slider_min_h + layout.eq_band_gap + label_h,
        );
        // Horizontal floor: the 10 bands at min width. The same
        // `layout.eq_band_w_min` drives both the shrink logic and this floor.
        d.insert_temp(
            egui::Id::new("tplay.pane_content_w").with(Pane::Equalizer),
            10.0 * layout.eq_band_w_min,
        );
    });

    ui.add_space(layout.eq_header_gap);

    // 10 bands with fixed inter-band spacing, centered in the pane: the gap
    // between sliders is constant, and the margins to the pane edges absorb all
    // leftover width equally (dynamic centering).
    let slider_h = (ui.available_height() - 30.0).clamp(layout.eq_slider_min_h, layout.eq_slider_max_h);
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
        ((avail_w / 10.0).max(layout.eq_band_w_min), 0.0)
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
                    ui.add_space(layout.eq_band_gap);
                    ui.label(
                        egui::RichText::new(TPlayApp::format_freq(EQ_FREQUENCIES[i]))
                            .color(p.text_secondary)
                            .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                    );
                });
            });
        }
    });
}