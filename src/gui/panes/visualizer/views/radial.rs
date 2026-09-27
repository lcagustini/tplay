//! Radial view — the same 32 log-spaced bands as Bars, painted around a circle.
//!
//! No new DSP: the same `compute_bands` call, a different placement. This is the
//! cheapest new look in the set, and the reason it exists rather than a
//! "Circular Bars" label on the existing view.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;
use std::f32::consts::{FRAC_PI_2, TAU};

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.radial")
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

    // Leave a ring of padding so a full-scale band never touches the pane edge.
    const PAD_FRAC: f32 = 0.08;
    // Drawn as a ring, not a filled pie: the drawn length is the level, and a
    // ring keeps the band count readable the way bars.rs does.
    const RING_W_FRAC: f32 = 0.35;
    /// Arc resolution — 4 per band is indistinguishable from smooth at 32 bands.
    const SEGMENTS: usize = 4;

    let radius = rect.width().min(rect.height()) * 0.5 * (1.0 - PAD_FRAC);
    let inner = radius * (1.0 - RING_W_FRAC);
    let center = rect.center();

    let step = TAU / VIZ_BANDS as f32;

    for (i, &db) in prev.iter().enumerate() {
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        // A floor keeps a silent band a visible tick, so a quiet passage reads as
        // "32 quiet bands" rather than as "no data".
        let len = inner + level * (radius - inner);
        let a0 = i as f32 * step - FRAC_PI_2;
        // A gap between bands, matching the one bars.rs leaves.
        let a1 = a0 + step * 0.9;

        let color = egui::Color32::from_rgba_unmultiplied(
            palette.accent.r(),
            palette.accent.g(),
            palette.accent.b(),
            (80.0 + 175.0 * level) as u8,
        );

        let arc = |from: f32, to: f32, r: f32| {
            (0..=SEGMENTS)
                .map(|s| {
                    let a = from + (to - from) * s as f32 / SEGMENTS as f32;
                    center + egui::vec2(a.cos(), a.sin()) * r
                })
                .collect::<Vec<_>>()
        };
        // Out and back: a ring sector is a simple (non-self-intersecting)
        // polygon, so egui's tessellator fills it correctly.
        let sector = arc(a0, a1, len)
            .into_iter()
            .rev()
            .chain(arc(a1, a0, inner))
            .collect();
        painter.add(egui::Shape::convex_polygon(
            sector,
            color,
            egui::Stroke::NONE,
        ));
    }
}
