//! Wave view — mirrored time-domain peak envelope silhouette.

use crate::audio::viz::{compute_wave, VizBuf};
use crate::gui::theme::Palette;
use eframe::egui;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    // ~2px columns read as a solid filled silhouette, not bars.
    const STRIP_W: f32 = 2.0;
    let n = (rect.width() / STRIP_W).ceil().max(2.0) as usize;
    let wave = compute_wave(viz, n);
    let bar_w = rect.width() / n as f32;

    let mid_y = rect.center().y;
    let amp = rect.height() * 0.4;

    // Fill: one adjacent column per envelope bucket, top and bottom mirrored.
    let fill_color = egui::Color32::from_rgba_unmultiplied(
        palette.accent.r(),
        palette.accent.g(),
        palette.accent.b(),
        80,
    );
    // Iterate the vector, not `0..n`: `compute_wave` returns exactly `n` buckets
    // today; if that changed, indexing would panic where iterating just draws
    // fewer columns.
    for (i, &w) in wave.iter().enumerate() {
        let x = rect.left() + i as f32 * bar_w;
        let y = w * amp;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, mid_y - y),
                egui::pos2(x + bar_w, mid_y + y),
            ),
            0.0,
            fill_color,
        );
    }

    // Outline the upper and lower envelope curves. `windows(2)` not `0..n-1` for
    // the same reason as the fill loop: walk what `wave` holds rather than trust
    // its length.
    let stroke = egui::Stroke::new(1.5_f32, palette.progress_fill);
    for (i, pair) in wave.windows(2).enumerate() {
        let x0 = rect.left() + i as f32 * bar_w;
        let x1 = x0 + bar_w;
        let y0 = pair[0] * amp;
        let y1 = pair[1] * amp;
        painter.line_segment([egui::pos2(x0, mid_y - y0), egui::pos2(x1, mid_y - y1)], stroke);
        painter.line_segment([egui::pos2(x0, mid_y + y0), egui::pos2(x1, mid_y + y1)], stroke);
    }
}