//! Spectrogram view — the band array over *time* instead of frequency, as a
//! scrolling waterfall.
//!
//! No new DSP: one `compute_bands` call per frame produces a column, and the
//! columns are kept in egui memory. That history is what makes this a different
//! read of the same spectrum rather than a repaint of Bars — a transient shows
//! as a vertical streak, which a bar chart cannot show at all.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;
use std::collections::VecDeque;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.spectrogram")
}

fn history_id() -> egui::Id {
    egui::Id::new("tplay.viz.history.spectrogram")
}

/// Oldest column drops off the left once this is exceeded. Bounded because
/// egui memory is not, and because a spectrogram of unbounded width is
/// indistinguishable from a smear.
const MAX_COLUMNS: usize = 256;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // No release smoothing: a waterfall is the place the raw transient is the
    // point, and the history already shows decay over the columns behind.
    const ATTACK: f32 = 0.6;
    const RELEASE: f32 = 0.6;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    let ctx = painter.ctx().clone();
    let mut history: VecDeque<[f32; VIZ_BANDS]> = ctx.memory_mut(|m| {
        m.data
            .get_temp::<VecDeque<[f32; VIZ_BANDS]>>(history_id())
            .unwrap_or_default()
    });
    history.push_back(prev);
    while history.len() > MAX_COLUMNS {
        history.pop_front();
    }
    ctx.memory_mut(|m| {
        m.data.insert_temp(prev_id(), prev);
        m.data.insert_temp(history_id(), history.clone());
    });

    let col_w = rect.width() / MAX_COLUMNS as f32;
    let row_h = rect.height() / VIZ_BANDS as f32;
    let offset = rect.width() - history.len() as f32 * col_w;

    for (age, bands) in history.iter().enumerate() {
        // Low bands at the bottom, the way the spectrum is read everywhere else.
        let x = rect.left() + offset + age as f32 * col_w;
        for (b, &db) in bands.iter().enumerate() {
            let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            if level <= 0.0 {
                continue;
            }
            let y = rect.bottom() - (b + 1) as f32 * row_h;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x, y),
                    egui::pos2(x + col_w + 0.5, y + row_h + 0.5),
                ),
                0.0,
                // bg → accent is the whole ramp, so no new theme tokens: the two
                // ends are already in the palette and the heat axis is theirs.
                palette.bg.lerp_to_gamma(palette.accent, level),
            );
        }
    }
}
