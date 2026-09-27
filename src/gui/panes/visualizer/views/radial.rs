//! Radial view — the same 32 log-spaced bands as Bars, painted around a circle.
//!
//! No new DSP: the same `compute_bands` call, a different placement. This is the
//! cheapest new look in the set, and the reason it exists rather than a
//! "Circular Bars" label on the existing view.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.radial")
}

/// Leave a ring of padding so a full-scale band never touches the pane edge.
const PAD_FRAC: f32 = 0.08;
/// Drawn as a ring, not a filled pie: the drawn length is the level, and a
/// ring keeps the band count readable the way bars.rs does.
const RING_W_FRAC: f32 = 0.35;
/// Arc resolution — 4 per band is indistinguishable from smooth at 32 bands.
pub const SEGMENTS: usize = 4;

/// The whole view's fill, as convex pieces: one quad per arc step, so
/// `VIZ_BANDS * SEGMENTS` of them.
///
/// A ring sector is **not** convex — it has the middle cut out — and egui
/// triangulates every closed path as a fan from its first point
/// (`epaint::tessellator::fill_closed_path`), which fills such a polygon as
/// garbage. Quads are convex by construction, and the chord of a quad's outer
/// edge deviates from the arc by ~0.05px at this radius, so the faceting is
/// invisible.
///
/// Pure and `Ui`-free so the convexity is testable without a window; `draw` is
/// the only caller.
pub fn fill_quads(rect: egui::Rect, levels: &[f32]) -> Vec<[egui::Pos2; 4]> {
    let radius = rect.width().min(rect.height()) * 0.5 * (1.0 - PAD_FRAC);
    let inner = radius * (1.0 - RING_W_FRAC);
    let center = rect.center();
    let step = std::f32::consts::TAU / levels.len().max(1) as f32;

    let mut quads = Vec::with_capacity(levels.len() * SEGMENTS);
    for (i, &level) in levels.iter().enumerate() {
        let outer = inner + level * (radius - inner);
        let a0 = i as f32 * step - std::f32::consts::FRAC_PI_2;
        // A gap between bands, matching the one bars.rs leaves.
        let a1 = a0 + step * 0.9;
        // The arc's unit direction at sub-step `s`, so scaling by a radius reads
        // as what it is.
        let at = |s: usize| {
            let a = a0 + (a1 - a0) * s as f32 / SEGMENTS as f32;
            egui::vec2(a.cos(), a.sin())
        };
        for s in 0..SEGMENTS {
            quads.push([
                center + at(s) * outer,
                center + at(s + 1) * outer,
                center + at(s + 1) * inner,
                center + at(s) * inner,
            ]);
        }
    }
    quads
}

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Same attack/release as bars.rs: the smoothing is what makes a spectrum
    // readable, so a different view of it should not be jitterier.
    const ATTACK: f32 = 0.3;
    const RELEASE: f32 = 0.92;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    // A floor keeps a silent band a visible tick, so a quiet passage reads as
    // "32 quiet bands" rather than as "no data".
    let levels: Vec<f32> = prev
        .iter()
        .map(|&db| ((db + 60.0) / 60.0).clamp(0.0, 1.0))
        .collect();

    for (i, quad) in fill_quads(rect, &levels).into_iter().enumerate() {
        let level = levels[i / SEGMENTS];
        let color = egui::Color32::from_rgba_unmultiplied(
            palette.accent.r(),
            palette.accent.g(),
            palette.accent.b(),
            (80.0 + 175.0 * level) as u8,
        );
        painter.add(egui::Shape::convex_polygon(
            quad.to_vec(),
            color,
            egui::Stroke::NONE,
        ));
    }
}
