//! Visualizer pane — header (view selector) + dispatch to the selected
//! `VizView`. Each view is one file under `views/` exposing a `draw` fn;
//! adding a view means a new file + a `VizView` variant + one match arm.

use crate::app::{TPlayApp, VizView};
use eframe::egui;

pub mod views;

pub fn visualizer_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let theme = app.theme().clone();
    let p = theme.palette;
    let layout = theme.layout.with_defaults();

    // Selected view — persisted in config.json via `TPlayApp::viz_view`.
    let mut view = app.viz_view();

    // Header: title + view selector
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Visualizer")
                .strong()
                .color(p.text_primary)
                .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::ComboBox::from_id_salt("viz_view")
                .selected_text(view.name())
                .show_ui(ui, |ui| {
                    for v in VizView::ALL {
                        if ui.selectable_value(&mut view, v, v.name()).changed() {
                            app.set_viz_view(view);
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

    // Background
    painter.rect_filled(rect, 0.0, p.bg);

    match view {
        VizView::Bars => views::bars::draw(painter, rect, app.viz(), &p),
        VizView::Wave => views::wave::draw(painter, rect, app.viz(), &p),
    }
}