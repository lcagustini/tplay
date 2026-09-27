use crate::app::{Pane, TPlayApp};
use crate::audio::eq::{EQ_FREQUENCIES, EQ_GAIN_MAX_DB, EQ_GAIN_MIN_DB, EQ_PRESETS};
use crate::gui::theme::ThemeState;
use eframe::egui;

/// The one band count every layout decision reads, so a change to
/// `EQ_FREQUENCIES` cannot leave the width maths and the floor behind.
const BANDS: usize = EQ_FREQUENCIES.len();
/// Widest a band's column gets before the row starts shrinking to fit.
const BAND_W_MAX: f32 = 80.0;

pub fn equalizer_pane(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui) {
    // Owned Arc copy — panes take `&mut app` while using theme data.
    let theme = themes.current().clone();
    let p = theme.palette;
    let gains = app.eq().gains();
    let layout = theme.layout;

    // Header — grouped controls; the tab already names the pane. Measured via
    // a scope so `min_content_h` below is exact, not guessed.
    let header_h = ui
        .scope(|ui| {
            ui.horizontal(|ui| {
                let on = app.eq().enabled();
                if ui.add(egui::Button::new("ON").selected(on)).clicked() {
                    app.eq_mut().toggle();
                }
                if ui.button("Reset").clicked() {
                    for i in 0..BANDS {
                        app.eq_mut().set_band(i, 0.0);
                    }
                }
                // One read of the derived preset: the ComboBox needs the `Option`
                // (it holds a `None` = Custom entry), and the label is that or
                // "Custom".
                let current = app.eq().preset();
                let mut sel = current.map(|s| s.to_string());
                egui::ComboBox::from_id_salt("tplay.eq.preset")
                    .selected_text(current.unwrap_or("Custom"))
                    .show_ui(ui, |ui| {
                        for (name, _) in EQ_PRESETS {
                            ui.selectable_value(&mut sel, Some(name.to_string()), name);
                        }
                        ui.selectable_value(&mut sel, None, "Custom");
                    });
                if sel != current.map(|s| s.to_string()) {
                    app.eq_mut().set_preset(sel.as_deref());
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
        // Horizontal floor: `BANDS` at min width. The same
        // `layout.eq_band_w_min` drives both the shrink logic and this floor.
        d.insert_temp(
            egui::Id::new("tplay.pane_content_w").with(Pane::Equalizer),
            BANDS as f32 * layout.eq_band_w_min,
        );
    });

    ui.add_space(layout.eq_header_gap);

    // Bands with fixed inter-band spacing, centered in the pane: the gap
    // between sliders is constant, and the margins to the pane edges absorb all
    // leftover width equally (dynamic centering).
    // The height budget subtracts the band gap and the label, both measured
    // above for the pane's minimum height. The floor above and this budget must
    // subtract the same two numbers, or a theme with a larger `text_meta` grows
    // one and not the other and the labels clip.
    let label_below = layout.eq_band_gap + label_h;
    let slider_h =
        (ui.available_height() - label_below).clamp(layout.eq_slider_min_h, layout.eq_slider_max_h);
    let min_spacing = ui.spacing().item_spacing.x;
    let avail_w = ui.available_width();
    let band_w_max = BAND_W_MAX;
    let n = BANDS as f32;
    let gaps = (BANDS - 1) as f32;
    let row_w_full = band_w_max * n + min_spacing * gaps;
    let (band_w, spacing) = if row_w_full <= avail_w {
        (band_w_max, min_spacing)
    } else if band_w_max * n <= avail_w {
        // Enough room for bands, shrink spacing to fit.
        (band_w_max, ((avail_w - band_w_max * n) / gaps).max(0.0))
    } else {
        // Not enough room even at zero spacing — shrink bands too.
        ((avail_w / n).max(layout.eq_band_w_min), 0.0)
    };
    let row_w = band_w * n + spacing * gaps;
    let pad = ((avail_w - row_w) / 2.0).max(0.0);

    ui.horizontal(|ui| {
        if pad > 0.0 {
            ui.add_space(pad);
        }
        ui.spacing_mut().item_spacing.x = spacing;

        for i in 0..BANDS {
            let mut gain = gains[i];

            ui.vertical(|ui| {
                ui.set_width(band_w);
                ui.spacing_mut().slider_width = slider_h;
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.push_id(i, |ui| {
                        let slider = egui::Slider::new(&mut gain, EQ_GAIN_MIN_DB..=EQ_GAIN_MAX_DB)
                            .show_value(true)
                            .vertical()
                            .trailing_fill(true);
                        let resp = ui.add(slider);
                        if resp.changed() {
                            app.eq_mut().set_band(i, gain);
                        }
                    });
                    ui.add_space(layout.eq_band_gap);
                    ui.label(
                        egui::RichText::new(TPlayApp::format_freq(EQ_FREQUENCIES[i]))
                            .color(p.text_secondary)
                            .font(egui::FontId::new(
                                layout.text_meta,
                                theme.metadata_font.clone(),
                            )),
                    );
                });
            });
        }
    });
}
