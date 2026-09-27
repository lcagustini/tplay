//! Chladni view — a cymatic standing-wave figure, the one view here that is not
//! a re-projection of the spectrum.
//!
//! The two loudest bands become the mode numbers `(n, m)` of the plate, and the
//! figure is the *nodal set*: sand collects where the field is at rest, so what
//! gets drawn is the `chladni_field` zero set. It is rendered as a contour —
//! per grid cell, the crossings of the nodal threshold interpolated along the
//! cell edges — rather than as one filled dot per cell. The nodal lines are thin,
//! so a per-cell draw spends almost all its quads painting background, and the
//! crossings both look better and are far fewer.

use crate::audio::viz::{chladni_field, compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.chladni")
}

/// Cells per axis for the contour search. 48 → the nodal lines are sub-pixel
/// thick at any pane this ships in, and the count is what bounds the work.
const GRID: usize = 48;
/// A cell edge counts as a crossing where the field drops below this.
const NODAL_CUTOFF: f32 = 0.10;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Heavily smoothed, and the reason there is no mode trail: the mode numbers
    // are a *discrete* pick, so a lightly-smoothed top-two changes on every
    // noise band and the figure strobes. Slow smoothing costs the figure its
    // response to real transients, which is the better trade — the two figures
    // a listener actually notices are minutes apart, not frames.
    const ATTACK: f32 = 0.08;
    const RELEASE: f32 = 0.08;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    // The two loudest bands, as a distinct ascending pair. `n == m` is a
    // degenerate mode — the two antisymmetric terms cancel and the figure is
    // blank — so the tie is broken by moving `m` on, never by leaving it.
    let mut ranked: Vec<(usize, f32)> = prev.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (mut n, mut m) = (ranked[0].0, ranked[1].0);
    if n > m {
        std::mem::swap(&mut n, &mut m);
    }
    if n == m {
        m = (m + 1) % VIZ_BANDS;
        if m < n {
            m = n + 1;
        }
    }

    // A square plate, centred and as large as the pane allows: stretching it
    // would distort the figure's symmetry, which is most of the content.
    let size = rect.width().min(rect.height()) * 0.94;
    let plate = egui::Rect::from_center_size(rect.center(), egui::vec2(size, size));
    let cell = size / GRID as f32;
    let at = |gx: usize, gy: usize| {
        (
            plate.left() + (gx as f32 + 0.5) * cell,
            plate.top() + (gy as f32 + 0.5) * cell,
        )
    };
    let dot_r = (cell * 0.6).max(0.75);

    for gy in 0..GRID {
        for gx in 0..GRID {
            let (fx0, fx1) = (gx as f32 / GRID as f32, (gx + 1) as f32 / GRID as f32);
            let (fy0, fy1) = (gy as f32 / GRID as f32, (gy + 1) as f32 / GRID as f32);
            let corners = [
                chladni_field(n, m, fx0, fy0),
                chladni_field(n, m, fx1, fy0),
                chladni_field(n, m, fx0, fy1),
                chladni_field(n, m, fx1, fy1),
            ];
            if corners.iter().all(|&v| v < NODAL_CUTOFF)
                || corners.iter().all(|&v| v >= NODAL_CUTOFF)
            {
                continue; // wholly one side of the threshold: nothing to draw
            }

            // Crossings of the threshold on the four edges, linearly
            // interpolated — the field is smooth, so this puts the line between
            // samples instead of snapping it to the grid.
            let (x, y) = at(gx, gy);
            let edges = [
                (corners[0], corners[1], (x, y), (x + cell, y)),
                (corners[2], corners[3], (x, y + cell), (x + cell, y + cell)),
                (corners[0], corners[2], (x, y), (x, y + cell)),
                (corners[1], corners[3], (x + cell, y), (x + cell, y + cell)),
            ];
            for (va, vb, a, b) in edges {
                let (da, db) = (va - NODAL_CUTOFF, vb - NODAL_CUTOFF);
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let t = da / (da - db);
                painter.circle_filled(
                    egui::pos2(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
                    dot_r,
                    palette.accent,
                );
            }
        }
    }
}
