//! Visualizer pane — header (view selector) + dispatch to the selected
//! `VizView`. Each view is one file under `views/` exposing a `draw` fn; a new
//! view is a new file + a `VizView` variant + one match arm.

use crate::app::{TPlayApp, VizView};
use crate::gui::theme::ThemeState;
use eframe::egui;

pub mod gpu;
pub mod views;

pub fn visualizer_pane(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui) {
    let theme = themes.current().clone();
    let p = theme.palette;
    let layout = theme.layout;

    // Selected view — persisted in config.json via `Prefs::viz_view`.
    let mut view = app.prefs().viz_view();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Visualizer")
                .strong()
                .color(p.text_primary)
                .font(egui::FontId::new(
                    layout.text_meta,
                    theme.metadata_font.clone(),
                )),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::ComboBox::from_id_salt("viz_view")
                .selected_text(view.name())
                .show_ui(ui, |ui| {
                    for v in VizView::ALL {
                        if ui.selectable_value(&mut view, v, v.name()).changed() {
                            app.prefs_mut().set_viz_view(view);
                        }
                    }
                });
        });
    });
    ui.add_space(4.0);

    let rect = ui.available_rect_before_wrap();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let painter = ui.painter();

    painter.rect_filled(rect, 0.0, p.bg);

    match view {
        VizView::Bars => views::bars::draw(painter, rect, app.viz(), &p),
        VizView::Wave => views::wave::draw(painter, rect, app.viz(), &p),
        VizView::Radial => views::radial::draw(painter, rect, app.viz(), &p),
        VizView::Spectrogram => views::spectrogram::draw(painter, rect, app.viz(), &p),
        VizView::Flame => views::flame::draw(painter, rect, app.viz(), &p),
        VizView::Vu => views::vu::draw(painter, rect, app.viz(), &p),
        VizView::Chladni => views::chladni::draw(painter, rect, app.viz(), &p),
        // A shader view is dispatched through the table rather than by a direct
        // call, so the table is the registry the app itself reads and cannot fall
        // behind what ships. The match stays exhaustive over `VizView`, so adding
        // a variant without an arm does not build; and a name in the table that
        // does not match any variant routes nothing, which is what
        // `the_shader_table_and_the_view_match_agree` is for.
        other => {
            if let Some(view) = views::SHADER_VIEWS.iter().find(|v| v.name == other.name()) {
                (view.draw)(painter, rect, app.viz(), &p);
            }
        }
    }
}
