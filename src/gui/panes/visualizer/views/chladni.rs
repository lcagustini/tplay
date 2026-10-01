//! Chladni view — a cymatic standing-wave figure, the one view here that is not
//! a re-projection of the spectrum.
//!
//! The plate's mode numbers `(n, m)` are read straight off the band levels every
//! frame, and the figure is the *nodal set*: sand collects where the field is at
//! rest, so what gets drawn is the zero set of
//!
//! ```text
//! |sin(pi n x) sin(pi m y) - sin(pi m x) sin(pi n y)|
//! ```
//!
//! **This view is what the whole conversion was for.** The CPU version did not
//! draw the field; it *searched* it. A 48×48 lattice of 2 401 field samples, a
//! marching-squares pass over 2 304 cells emitting a segment per pair of
//! interpolated edge crossings, then ~1 200 segments each expanded to a quad and
//! a triangle fan inside a hand-built `Mesh` — about 71 µs a frame, the most
//! expensive view in the set by an order of magnitude, all of it to decide which
//! fragments to paint. A fragment shader does the same work in four sines and no
//! search at all, which is the shape of problem the harness exists for.
//!
//! **`Chladni3D` used to be a second view here and was deleted**, and the reason
//! is worth keeping: it claimed to march the field through a volume, but the field
//! is a function of `(x, y)` alone, so all six of its z-slices returned the
//! identical value and its "slab" was the arithmetic mean of six copies of one
//! number. Six times the work for a bit-identical picture. Once both views were
//! per-fragment they were the same drawing with a glow on it, and a duplicate is
//! a maintenance cost rather than a feature. What survived is the good half of
//! each: `fwidth` for the line's width, and the glow and edge fade from the
//! volumetric one.
//!
//! **The mode read is continuous, and that is the whole design.** It was a
//! hysteretic argmax over the two loudest band indices, which made the pair the
//! *only* channel the music had into the picture — and a discrete pick has
//! discrete failure modes: two near-equal bands repainted the whole plate, so it
//! needed a `MARGIN_DB` and a 1.5 s hold, and the hold capped the figure at one
//! change per 1.5 s by construction. The image was bit-identical between
//! switches. Reported as "the chladni viz is rendering at 5fps" and then, once
//! that turned out to be a still image rather than a slow one, as "kinda still
//! and unrelated to the music". Neither was a drawing problem: the field is
//! continuous in both mode numbers, so any real `(n, m)` is a valid cymatic
//! figure and a continuous read of the spectrum needs no margin, no hold and no
//! stored slide state at all. All of that machinery is deleted, along with the
//! `Slide` struct that existed only to animate between two committed pairs.
//! The one state left is the previous pair, and it is read on silence and for
//! nothing else — `audio::viz::mode_now` is where the reasoning lives.
//!
//! The four tests that pinned the old Rust `chladni_field` went with the field's
//! deletion. They were real properties — zero on the diagonal, `n == m`
//! degenerate — but they described a *search* whose failure modes (a cell
//! straddling a nodal line, a dangling segment end) a per-fragment evaluation
//! cannot have. The degenerate pair is now *unreachable* rather than untested:
//! `mode_now` reads two disjoint halves of the band range, so `n < m` holds by
//! construction.

use super::frame_dt;
use crate::audio::viz::{compute_bands, mode_now, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu;
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.chladni")
}

fn mode_id() -> egui::Id {
    egui::Id::new("tplay.viz.chladni.mode")
}

/// This view's own functions. See `ShaderView::helpers` for why they are a
/// separate slot from the body.
pub const HELPERS: &str = r#"
// The plate as a fraction of the pane's short side. A square, because stretching
// it distorts the figure's symmetry, which is most of the content.
const PLATE_FRAC : f32 = 0.94;

// The nodal set: |sin(pi n x) sin(pi m y) - sin(pi m x) sin(pi n y)|. `u_modes`
// is the (n, m) pair `mode_now` read off the spectrum, which is where this view's
// relationship to the music lives; the GPU is only asked to draw it.
//
// **The pair is fractional, and that is the point.** The sines are continuous in
// both numbers, so any real `n, m` is a valid cymatic figure rather than an
// error — which is what lets the plate follow the music between one integer pair
// and the next instead of waiting to be handed one.
fn field(p : vec2<f32>) -> f32 {
    let n = u_modes().x;
    let m = u_modes().y;
    let px = 3.14159265 * n * p.x;
    let py = 3.14159265 * n * p.y;
    let qx = 3.14159265 * m * p.x;
    let qy = 3.14159265 * m * p.y;
    return abs(sin(px) * sin(qy) - sin(qx) * sin(py));
}
"#;

pub const FRAG: &str = r#"
    let uv = v_uv;
    let centred = (uv - 0.5) * vec2<f32>(u_resolution().x / max(u_resolution().y, 1.0), 1.0);
    // The plate, square and centred, in 0..1 over the pane's short side.
    let p = centred / PLATE_FRAC * 0.5;

    let d = field(p);

    // The nodal line's half-width, from the field's own screen-space gradient.
    // This is what a fixed field-space cutoff cannot do: it makes the stroke fat
    // where the field is flat and hairline where it is steep, so the same figure
    // would be drawn at two different weights.
    let w = max(fwidth(d) * 0.75, 1e-5);
    let line = 1.0 - smoothstep(0.0, w, d);

    // A second, wider band around the same line. One threshold alone reads as a
    // wireframe; the falloff is what reads as sand settled into a plate, and it
    // is free here because the field is already evaluated.
    let glow = 1.0 - smoothstep(0.0, 0.42, d);

    // Off the plate, and a fade at its edge so the figure does not end on a hard
    // rectangle. Both multiply out rather than branch, because `fwidth` above has
    // to be reached by *every* fragment — a derivative taken in non-uniform
    // control flow is undefined, and a shader that returned early for the
    // off-plate fragments would never reach it at all.
    let extent = max(abs(p.x), abs(p.y));
    let on_plate = step(extent, 1.0);
    let edge = 1.0 - smoothstep(0.92, 1.0, extent);

    // The same body/edge split bars.rs and the flame view make: a dim mass in the
    // accent, the sharp nodal set in the brighter token.
    var col = mix(u_bg().rgb, u_accent().rgb, glow * 0.30);
    col = mix(col, u_progress_fill().rgb, line);
    frag_color = vec4<f32>(mix(u_bg().rgb, col, edge * on_plate), 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Aimed at the *spectrum*, not at the figure — and now that the figure *is* a
    // continuous read of the spectrum, this is the only thing standing between a
    // plate and the raw bin-to-bin jitter of the FFT. It was 0.08 back when the
    // justification was "blur the levels so the top-two pick survives", which
    // could not work: a **discrete** pick has nothing for a smoother to average,
    // so 0.08 only delayed the input to the decision and a real change of music
    // waited ~208 ms to be seen. It has a real job now.
    //
    // Release is faster than attack, so a band that stops being loud stops holding
    // the figure. The other direction is a transient, and a transient should not
    // win a plate.
    const ATTACK_MS: f32 = 46.7;
    const RELEASE_MS: f32 = 18.2;
    compute_bands(viz, &mut prev, frame_dt(painter), ATTACK_MS, RELEASE_MS);

    // Read the plate off the smoothed levels, every frame. `prev` is here only to
    // be held on silence, where there is no energy to have an opinion — there is
    // no countdown, no committed target and no anti-strobe state, because a
    // continuous read cannot strobe.
    let dt = painter.ctx().input(|i| i.predicted_dt);
    let held: Option<(f32, f32)> = painter.ctx().memory_mut(|m| m.data.get_temp(mode_id()));
    let modes = mode_now(&prev, held.unwrap_or((1.0, 6.0)), dt);
    painter.ctx().memory_mut(|mem| {
        mem.data.insert_temp(prev_id(), prev);
        mem.data.insert_temp(mode_id(), modes);
    });

    let mut uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    uniforms.modes = modes.into();
    gpu::add_fullscreen(painter, rect, HELPERS, FRAG, uniforms);
}
