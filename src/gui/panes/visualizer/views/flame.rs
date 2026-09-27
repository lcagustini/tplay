//! Flame view — the band array as a filled ridgeline.
//!
//! No new DSP: one `compute_bands` call. Where Bars draws 32 separate columns,
//! this closes them into one contour, which reads as a spectrum's *shape* and
//! its evolution rather than as 32 unrelated heights.
//!
//! The look is the ridgeline used by audio-spectrum visualisers (the effect
//! usually credited to the `flam3` renderer). Named in the past with a link to
//! that name's source; left unnamed here on purpose, because "flame" resolves to
//! Brendan Gregg's *profiling* flame graph, which is a different thing built from
//! stack traces, and a wrong reference is worse than none. What the view draws is
//! the sentence above.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.flame")
}

/// The flat floor the contour rises from.
const BASE_FRAC: f32 = 0.12;

/// The fill down to the baseline, as convex pieces: one trapezoid per contour
/// step, so `levels.len() - 1` of them.
///
/// The whole ridgeline plus a baseline edge is a wiggly mass, so it is **not**
/// convex, and the mesh `draw` builds fans each piece from its first corner, so
/// a reflex corner spills outside it. Each trapezoid is convex because `x` only
/// ever increases and the top edge interpolates between two heights at or above
/// the baseline; a floor-level step degenerates to zero area, which a triangle
/// fan emits as nothing.
///
/// Pure and `Ui`-free so the convexity is testable without a window; `draw` is
/// the only caller. The top edge of each quad *is* the contour, so the caller
/// reads its stroke back off `quad[0]`/`quad[1]` rather than recomputing it.
pub fn fill_quads(rect: egui::Rect, levels: &[f32]) -> Vec<[egui::Pos2; 4]> {
    let n = levels.len();
    if n < 2 {
        return Vec::new();
    }
    let base_y = rect.bottom() - rect.height() * BASE_FRAC;
    let amp = rect.height() * (1.0 - BASE_FRAC) * 0.9;
    // Walk the array rather than indexing a fixed range: a 31-point contour
    // cannot close, and an off-by-one here would silently draw a gap.
    let x = |i: usize| rect.left() + i as f32 / (n - 1) as f32 * rect.width();
    let y = |i: usize| base_y - levels[i] * amp;

    (0..n - 1)
        .map(|i| {
            [
                egui::pos2(x(i), y(i)),
                egui::pos2(x(i + 1), y(i + 1)),
                egui::pos2(x(i + 1), base_y),
                egui::pos2(x(i), base_y),
            ]
        })
        .collect()
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

    let levels: Vec<f32> = prev
        .iter()
        .map(|&db| ((db + 60.0) / 60.0).clamp(0.0, 1.0))
        .collect();
    let quads = fill_quads(rect, &levels);

    // One mesh, not one `Shape` per trapezoid. epaint insets a closed path's
    // fill by half its 1px feathering and feathers the rim outward, so 31
    // adjacent translucent pieces leave a real ~1px unpainted gap along each of
    // the 30 shared edges — the pane background showing through as a line, worst
    // where the mass is dimmest. A mesh draws the triangles exactly: the shared
    // edge belongs to the two triangles that meet on it and to nothing else.
    let mut mesh = egui::epaint::Mesh::default();
    mesh.reserve_vertices(4 * quads.len());
    mesh.reserve_triangles(4 * quads.len());
    for quad in &quads {
        let base = mesh.vertices.len() as u32;
        for corner in quad {
            mesh.colored_vertex(*corner, palette.accent.gamma_multiply(0.45));
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));

    // The contour itself, in the brighter token — the same split bars.rs makes
    // between a dim body and an accent edge. Read straight off the fill's top
    // edge, which is the same polyline, so there is no second copy of the
    // geometry to drift.
    let contour: Vec<egui::Pos2> = quads
        .iter()
        .map(|q| q[0])
        .chain(quads.last().map(|q| q[1]))
        .collect();
    painter.add(egui::Shape::line(
        contour,
        egui::Stroke::new(1.5_f32, palette.progress_fill),
    ));
}
