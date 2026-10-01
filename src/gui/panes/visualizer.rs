//! Visualizer pane — header (view selector) + dispatch to the selected
//! `VizView`. Each view is one file under `views/` exposing a `draw` fn; a new
//! view is a new file + a `VizView` variant + one match arm.

use crate::app::{Pane, TPlayApp, VizView};
use crate::audio::viz::VIZ_BANDS;
use crate::gui::theme::{Layout, ThemeState};
use eframe::egui;

pub mod gpu;
pub mod views;

/// The drawing area this pane refuses to be squeezed below, and the width it
/// refuses to be squeezed below.
///
/// Both are **two pixels per band cell** — the same threshold `bars` dims out at
/// per fragment, because below it a 32-band row is not a row. One number for both
/// axes on purpose: the views here are all aspect-corrected, so a pane gets the
/// room it needs whichever way it is split rather than one axis being starved.
fn viz_min(layout: Layout) -> (f32, f32) {
    let cell = VIZ_BANDS as f32 * 2.0;
    // The height floor is the drawing area *plus* the measured header, and the
    // header scales with the theme's type size — so the multiplier is the theme's
    // own `text_meta` over the 12px it defaults to. That keeps the two in step
    // rather than hardcoding a header height that a larger type would outgrow.
    (cell + layout.text_meta, cell)
}

pub fn visualizer_pane(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui) {
    let theme = themes.current().clone();
    let p = theme.palette;
    let layout = theme.layout;

    // Selected view — persisted in config.json via `Prefs::viz_view`.
    let mut view = app.prefs().viz_view();

    // **Measured, not assumed** — the same rule the Equalizer pane follows and for
    // the same reason: the header's height is a function of `layout.text_meta`,
    // and a theme with a larger meta grows both the header and the floor that
    // contains it. Writing the number down twice is how the EQ pane's slider
    // budget once clipped its own labels.
    let header_h = ui
        .horizontal(|ui| {
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
                                app.prefs_mut().set_viz_view(v);
                            }
                        }
                    });
            });
        })
        .response
        .rect
        .height();
    ui.add_space(4.0);

    let rect = ui.available_rect_before_wrap();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    // **A floor, recorded the way the Equalizer and Now Playing record theirs.**
    // This pane is the only one that draws nothing but a fullscreen shader, so
    // it has no content whose measured size could floor it — and without a floor
    // the splitter can drag it to a few pixels tall, where every view here is
    // either a smear or (for `bars`) a row of single pixels. The other panes are
    // floored by what they lay out; this one has to state its own.
    //
    // The height is the *measured* header plus the drawing area, so a theme with
    // a larger `text_meta` grows both and the floor still contains the whole
    // pane. `4.0` is the `add_space` above, written once here rather than
    // assumed — and the drawing area is the same cell rule the `bars` shader
    // applies per fragment, so a view that can dim itself out still gets enough
    // room to be legible in.
    let (min_draw_h, min_w) = viz_min(layout);
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("tplay.pane_content_h").with(Pane::Visualizer),
            header_h + 4.0 + min_draw_h,
        );
        d.insert_temp(
            egui::Id::new("tplay.pane_content_w").with(Pane::Visualizer),
            min_w,
        );
    });

    let painter = ui.painter();

    painter.rect_filled(rect, 0.0, p.bg);

    // Every view is a fragment shader, so there is one dispatch and no match.
    // The lookup is by name rather than by an exhaustive `match` over `VizView`,
    // which is what used to make the compiler the thing that noticed a new
    // variant had no arm — and that check is now `every_view_has_a_shader_and_the
    // _table_covers_them_all`, which is stronger: it also catches a `VizView` the
    // table names but does not implement, and the reverse.
    //
    // **Every view draws every frame, paused or not** — and that is not an
    // oversight, it is what "paused" looks like. Gating the dispatch on playback
    // was the obvious fix for a view that kept moving while paused, and it made
    // the pane go blank, which reads as the feature vanishing rather than as the
    // picture holding. The two feedback views are the only ones that *can* move,
    // because they are the only ones with a history, and they hold instead — see
    // `VizBuf::take_arrival`, which is the whole of "the visualizer pauses with
    // the song". The other seven are pure functions of a buffer that is not
    // changing, so they redraw the same frame without being told to stop.
    if let Some(entry) = views::SHADER_VIEWS.iter().find(|v| v.name == view.name()) {
        (entry.draw)(painter, rect, app.viz(), &p);
    }
}
