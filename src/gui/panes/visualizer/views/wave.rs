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
    for i in 0..n {
        let x = rect.left() + i as f32 * bar_w;
        let y = wave[i] * amp;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, mid_y - y),
                egui::pos2(x + bar_w, mid_y + y),
            ),
            0.0,
            fill_color,
        );
    }

    // Outline along the upper and lower envelope curves
    let stroke = egui::Stroke::new(1.5_f32, palette.progress_fill);
    for i in 0..n - 1 {
        let x0 = rect.left() + i as f32 * bar_w;
        let x1 = x0 + bar_w;
        let y0 = wave[i] * amp;
        let y1 = wave[i + 1] * amp;
        painter.line_segment([egui::pos2(x0, mid_y - y0), egui::pos2(x1, mid_y - y1)], stroke);
        painter.line_segment([egui::pos2(x0, mid_y + y0), egui::pos2(x1, mid_y + y1)], stroke);
    }
}