//! Bars view — FFT bands (32 log-spaced, dB-normalized) with attack/release
//! smoothing for the classic WMP bars feel.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;

/// egui memory id for the previous (smoothed) band values — this view's private
/// per-frame state.
fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.bars")
}

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Attack/release constants (per-frame, 60 FPS assumed)
    const ATTACK: f32 = 0.3;
    const RELEASE: f32 = 0.92;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let n = prev.len();
    let bar_w = (rect.width() / n as f32).max(1.0);
    let mid_y = rect.center().y;
    let max_h = rect.height() * 0.45; // leave margins top/bottom

    for (i, &db) in prev.iter().enumerate() {
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let h = level * max_h;

        let x = rect.left() + i as f32 * bar_w;
        let w = (bar_w * 0.8).max(1.0);
        let gap = bar_w - w;

        let top = mid_y - h;
        let bottom = mid_y + h;

        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(x + gap * 0.5, top),
            egui::pos2(x + gap * 0.5 + w, bottom),
        );

        // Gradient-like: accent at center, fading toward edges
        let alpha = (0.4 + 0.6 * level).clamp(0.0, 1.0);
        let color = egui::Color32::from_rgba_unmultiplied(
            palette.accent.r(),
            palette.accent.g(),
            palette.accent.b(),
            (alpha * 255.0) as u8,
        );
        painter.rect_filled(bar_rect, 1.0, color);

        // Subtle center line
        if i % 4 == 0 {
            painter.hline(
                x + gap * 0.5..=x + gap * 0.5 + w,
                mid_y,
                egui::Stroke::new(0.5_f32, palette.accent.gamma_multiply(0.5)),
            );
        }
    }
}
