//! Chladni view — a cymatic standing-wave figure, the one view here that is not
//! a re-projection of the spectrum.
//!
//! The two loudest bands become the mode numbers `(n, m)` of the plate, and the
//! figure is the *nodal set*: sand collects where the field is at rest, so what
//! gets drawn is the `chladni_field` zero set. It is rendered as a contour —
//! marching squares over the field, one segment per pair of interpolated edge
//! crossings — rather than one filled dot per cell. A cell that straddles a
//! nodal line would spend all its quads painting background.

use crate::audio::viz::{chladni_field, compute_bands, VizBuf, VIZ_BANDS};
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

/// The highest band index eligible to be a mode number, and so the highest mode
/// number drawn. Not a taste call: `GRID` cannot resolve a mode much above this
/// (at mode 30 an oscillation is 1.6 cells, so the interpolated lines land in
/// the wrong place), and the high bands are single FFT bins of noise with no
/// music in them, so they are the last thing that should be choosing a figure.
pub const MAX_MODE: usize = 10;
/// `pick_mode` seeds two slots from one band, so it needs a second eligible
/// band, and it indexes `bands` directly, so it needs a band that exists.
const _: () = assert!(MAX_MODE >= 2 && MAX_MODE < VIZ_BANDS);

/// How much louder a challenger has to be before the figure changes, in dB.
pub const MARGIN_DB: f32 = 3.0;
/// Frames the figure is pinned after a switch. The insurance against a genuine
/// two-cycle, which hysteresis always permits.
const HOLD_FRAMES: u32 = 45;

/// Pick the plate's mode pair from the smoothed band levels, sticking to the
/// current one.
///
/// The mode numbers are a **discrete** pick, so a bare top-two repaints the
/// whole plate as a different figure whenever two bands are near-equal at the
/// top — most frames of most music, and every frame of a quiet passage, where
/// all 32 bands sit on the `-60` floor and the order is decided by hundredths of
/// a dB of FFT noise. Measured against the real smoothing constants over a
/// drifting bass line, a bare top-two switched 60 times in 15 s: four
/// whole-figure redraws a second.
///
/// Smoothing cannot fix that, and the cost of it here is not a taste call
/// either — a discrete pick has nothing for a smoother to average. So the figure
/// sticks: keep the current pair unless a challenger is louder by
/// `MARGIN_DB`, and pin it for `hold` frames after a switch. `hold` is a frame
/// count rather than a clock, so the caller owns the decrement.
///
/// Candidates are band indices `1..=MAX_MODE`, and **the band index is the mode
/// number**. Band 0 is excluded because `chladni_field` clamps mode 0 to 1, so
/// it could only ever redraw mode 1's figure — a free source of the exact
/// strobing this exists to stop.
pub fn pick_mode(
    bands: &[f32; VIZ_BANDS],
    current: Option<(usize, usize)>,
    hold: u32,
) -> (usize, usize) {
    // The top two eligible bands, in one pass and without allocating: this runs
    // every frame. Seeding both slots with band 1 costs nothing because the
    // second slot is necessarily overwritten by band 2 (MAX_MODE >= 2).
    let mut top: [(usize, f32); 2] = [(1, f32::MIN); 2];
    for (i, &level) in bands.iter().enumerate().skip(1).take(MAX_MODE) {
        if level > top[0].1 {
            top[1] = top[0];
            top[0] = (i, level);
        } else if level > top[1].1 {
            top[1] = (i, level);
        }
    }
    let cand = if top[0].0 > top[1].0 {
        (top[1].0, top[0].0)
    } else {
        (top[0].0, top[1].0)
    };

    let Some((n, m)) = current else { return cand };
    let eligible = |i: usize| (1..=MAX_MODE).contains(&i);
    // A stored pair outside the range, or degenerate, is stale — an edited
    // MAX_MODE, or egui memory that outlived the build that wrote it — and
    // re-picking beats drawing a blank or an unresolvable plate.
    if !eligible(n) || !eligible(m) || n == m {
        return cand;
    }
    if hold > 0 || cand == (n, m) {
        return (n, m);
    }
    // The *weaker* band of each pair, not the louder one: a pair is only as
    // loud as its quieter member, so that is the honest comparison to make.
    let weak = |p: (usize, usize)| f32::min(bands[p.0], bands[p.1]);
    if weak(cand) > weak((n, m)) + MARGIN_DB {
        cand
    } else {
        (n, m)
    }
}

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

    let mut segs = Vec::new();
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

    const ATTACK: f32 = 0.08;
    const RELEASE: f32 = 0.08;
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
