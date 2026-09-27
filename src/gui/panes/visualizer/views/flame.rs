//! Flame view — the band array as a filled ridgeline, the `flam3`/audialsense
//! look.
//!
//! No new DSP: one `compute_bands` call. Where Bars draws 32 separate columns,
//! this closes them into one contour, which reads as a spectrum's *shape* and
//! its evolution rather than as 32 unrelated heights.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.flame")
}

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Brisker than bars.rs: a contour's edge is the signal, and the fast release
    // is what makes the decay trail read as a separate ridge.
    const ATTACK: f32 = 0.5;
    const RELEASE: f32 = 0.85;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    /// The flat floor the contour rises from.
    const BASE_FRAC: f32 = 0.12;
    let base_y = rect.bottom() - rect.height() * BASE_FRAC;
    let amp = rect.height() * (1.0 - BASE_FRAC) * 0.9;

    // Walk the array rather than indexing a fixed range: a 31-point contour
    // cannot close, and an off-by-one here would silently draw a gap.
    let level = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let pts: Vec<egui::Pos2> = prev
        .iter()
        .enumerate()
        .map(|(i, &db)| {
            let x = rect.left() + i as f32 / (VIZ_BANDS - 1) as f32 * rect.width();
            egui::pos2(x, base_y - level(db) * amp)
        })
        .collect();

    // Fill down to the baseline so it reads as a mass, not a wire.
    let mut fill = pts.clone();
    fill.push(egui::pos2(rect.right(), base_y));
    fill.push(egui::pos2(rect.left(), base_y));
    painter.add(egui::Shape::convex_polygon(
        fill,
        palette.accent.gamma_multiply(0.45),
        egui::Stroke::NONE,
    ));

    // The contour itself, in the brighter token — the same split bars.rs makes
    // between a dim body and an accent edge.
    painter.add(egui::Shape::line(
        pts,
        egui::Stroke::new(1.5_f32, palette.progress_fill),
    ));
}
