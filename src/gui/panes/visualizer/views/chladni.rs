//! Chladni view — a cymatic standing-wave figure, the one view here that is not
//! a re-projection of the spectrum.
//!
//! The two loudest bands become the mode numbers `(n, m)` of the plate, and the
//! figure is the *nodal set*: sand collects where the field is at rest, so what
//! gets drawn is the `chladni_field` zero set. It is rendered as a contour —
//! marching squares over the field, one segment per pair of interpolated edge
//! crossings — rather than one filled dot per cell. A cell that straddles a
//! nodal line would spend all its quads painting background.

use crate::audio::viz::{chladni_field, compute_bands, pick_mode, VizBuf, HOLD_FRAMES, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.chladni")
}

fn mode_id() -> egui::Id {
    egui::Id::new("tplay.viz.chladni.mode")
}

/// Cells per axis for the contour search. 48 is what bounds the work, and it is
/// also what makes the interpolation trustworthy: at the highest mode this ships
/// there are ~10 cells per oscillation, so a crossing placed between two
/// samples is within a fraction of a cell of the truth.
pub const GRID: usize = 48;
/// A cell edge counts as a crossing where the field drops below this.
const NODAL_CUTOFF: f32 = 0.10;
/// Stroke width of the nodal line. Fixed rather than tied to the cell size, so
/// the grid decides where the line goes and not how fat it draws.
const STROKE_W: f32 = 1.5;
/// The plate as a fraction of the pane's short side: a square one, because
/// stretching it distorts the figure's symmetry, which is most of the content.
const PLATE_FRAC: f32 = 0.94;

/// The nodal set at mode `(n, m)` over the plate, as marching-squares segments.
///
/// The field is sampled once across the whole `GRID + 1` lattice rather than
/// four times per cell: 2 401 evaluations instead of 9 216, and — the reason it
/// matters — two cells sharing an edge then read the *same* stored f32, so their
/// interpolated join points are bit-identical and the contour is continuous
/// rather than a field of dots that only looks joined.
///
/// A 4-crossing cell is genuinely ambiguous: the field can pass through it as
/// two separate arcs or as both diagonals crossing, and a 2×2 sample cannot
/// tell them apart. Pairing the crossings in boundary order is the honest
/// answer, and it is only wrong in the cells where the question is undecidable
/// from the samples taken (36–104 of ~1 000 at the modes this ships).
///
/// Pure and `Ui`-free, so continuity and the work bound are testable without a
/// window; `draw` is the only caller.
pub fn segments(n: usize, m: usize, rect: egui::Rect) -> Vec<[egui::Pos2; 2]> {
    let size = rect.width().min(rect.height()) * PLATE_FRAC;
    let plate = egui::Rect::from_center_size(rect.center(), egui::vec2(size, size));

    let mut field = [[0.0f32; GRID + 1]; GRID + 1];
    for (gx, column) in field.iter_mut().enumerate() {
        let fx = gx as f32 / GRID as f32;
        for (gy, value) in column.iter_mut().enumerate() {
            *value = chladni_field(n, m, fx, gy as f32 / GRID as f32);
        }
    }
    let at = |gx: usize, gy: usize| {
        egui::pos2(
            plate.left() + gx as f32 * plate.width() / GRID as f32,
            plate.top() + gy as f32 * plate.height() / GRID as f32,
        )
    };

    // Reserved up front: the count is ~GRID²/2, so growing into it costs ten
    // reallocations and a copy of 32 KB, on a per-frame path.
    let mut segs = Vec::with_capacity(2 * GRID * GRID);
    for gy in 0..GRID {
        for gx in 0..GRID {
            // Boundary order — top, right, bottom, left — is what makes an
            // ambiguous cell pair its crossings the same way from every side.
            let corner = [
                field[gx][gy],
                field[gx + 1][gy],
                field[gx + 1][gy + 1],
                field[gx][gy + 1],
            ];
            let node = [
                at(gx, gy),
                at(gx + 1, gy),
                at(gx + 1, gy + 1),
                at(gx, gy + 1),
            ];

            // The crossings of the nodal threshold on the four edges, linearly
            // interpolated — the field is smooth, so this puts the line between
            // samples instead of snapping it to the grid.
            let mut cuts = [None; 4];
            let mut count = 0;
            for e in 0..4 {
                let next = (e + 1) % 4;
                let (da, db) = (corner[e] - NODAL_CUTOFF, corner[next] - NODAL_CUTOFF);
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let t = da / (da - db);
                cuts[count] = Some(egui::pos2(
                    node[e].x + (node[next].x - node[e].x) * t,
                    node[e].y + (node[next].y - node[e].y) * t,
                ));
                count += 1;
            }

            // A closed walk crosses a threshold an even number of times, so
            // `count` is 0, 2 or 4 and every crossing is paired.
            let mut i = 0;
            while i + 1 < count {
                if let (Some(a), Some(b)) = (cuts[i], cuts[i + 1]) {
                    segs.push([a, b]);
                }
                i += 2;
            }
        }
    }
    segs
}

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Aimed at the *spectrum*, not at the figure. The stickiness lives in
    // `pick_mode`, and the 0.08 that used to be here was a leftover from when the
    // bare top-two needed the levels blurred to survive — but a 0.08 coefficient
    // is a 12.5-frame time constant (~208 ms per band), and it cannot average a
    // *discrete* pick, so it bought no stability at all. It only blurred the input
    // to the decision, which is where a real change of the music then had to wait
    // to show up. At 0.3 that wait is ~4 frames instead of ~12.
    //
    // Release is faster than attack so a band that stops being loud stops
    // holding the figure hostage. The other direction is a transient, and a
    // transient should not win a plate.
    const ATTACK: f32 = 0.3;
    const RELEASE: f32 = 0.6;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    // Which figure is showing, and how many frames it is still pinned for, is
    // one piece of state: two keys could disagree and there would be no way to
    // tell which of them won.
    let held: Option<(usize, usize, u32)> =
        painter.ctx().memory_mut(|m| m.data.get_temp(mode_id()));
    let (current, hold) = match held {
        Some((n, m, hold)) => (Some((n, m)), hold.saturating_sub(1)),
        None => (None, 0),
    };
    let (n, m) = pick_mode(&prev, current, hold);
    let switched = Some((n, m)) != current;
    painter.ctx().memory_mut(|mem| {
        mem.data.insert_temp(prev_id(), prev);
        mem.data
            .insert_temp(mode_id(), (n, m, if switched { HOLD_FRAMES } else { hold }));
    });

    let segs = segments(n, m, rect);
    if segs.is_empty() {
        return;
    }

    // One mesh for the whole figure. A `line_segment` per segment is ~1 200
    // separately tessellated `Shape`s a frame, which is what made this view drop
    // frames; a mesh is one shape and one tessellation. epaint 0.30's `Mesh` has
    // no line primitive, so each segment is expanded to a quad here. Winding is
    // free — egui_glow disables CULL_FACE and wgpu is handed `cull_mode: None`.
    let mut mesh = egui::epaint::Mesh::default();
    mesh.reserve_vertices(4 * segs.len());
    mesh.reserve_triangles(2 * segs.len());
    for [a, b] in segs.iter().copied() {
        let d = b - a;
        // Two crossings can land on the same lattice node, and `normalized()` on
        // a zero vector is NaN — a NaN vertex in the mesh, not a skipped dot.
        if d.length() < 0.01 {
            continue;
        }
        let perp = egui::vec2(-d.y, d.x).normalized() * (STROKE_W * 0.5);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(a + perp, palette.accent);
        mesh.colored_vertex(b + perp, palette.accent);
        mesh.colored_vertex(b - perp, palette.accent);
        mesh.colored_vertex(a - perp, palette.accent);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
}
