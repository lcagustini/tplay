//! Chladni view — a cymatic standing-wave figure, the one view here that is not
//! a re-projection of the spectrum.
//!
//! The two loudest bands become the mode numbers `(n, m)` of the plate, and the
//! figure is the *nodal set*: sand collects where the field is at rest, so what
//! gets drawn is the zero set of
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
//! The four tests that pinned the old Rust `chladni_field` went with the field's
//! deletion. They were real properties — zero on the diagonal, `n == m`
// degenerate — but they described a *search* whose failure modes (a cell
//! straddling a nodal line, a dangling segment end) a per-fragment evaluation
//! cannot have. `pick_mode` stays tested, because the hysteresis is a different
//! problem from evaluating a formula.

use crate::audio::viz::{
    compute_bands, hold_tick, pick_mode, VizBuf, DB_FLOOR, HOLD_SECS, VIZ_BANDS,
};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.chladni")
}

fn mode_id() -> egui::Id {
    egui::Id::new("tplay.viz.chladni.mode")
}

pub const FRAG: &str = r#"
// The plate as a fraction of the pane's short side. A square, because stretching
// it distorts the figure's symmetry, which is most of the content.
const float PLATE_FRAC = 0.94;

// The nodal set: |sin(pi n x) sin(pi m y) - sin(pi m x) sin(pi n y)|. `u_modes`
// is the (n, m) pair `pick_mode` chose on the CPU, which is where the hysteresis
// lives; the GPU is only asked to draw the figure that pick names.
float field(vec2 p) {
    float n = u_modes.x;
    float m = u_modes.y;
    float px = 3.14159265 * n * p.x;
    float py = 3.14159265 * n * p.y;
    float qx = 3.14159265 * m * p.x;
    float qy = 3.14159265 * m * p.y;
    return abs(sin(px) * sin(qy) - sin(qx) * sin(py));
}

void main() {
    vec2 uv = v_uv;
    vec2 centred = (uv - 0.5) * vec2(u_resolution.x / max(u_resolution.y, 1.0), 1.0);
    // The plate, square and centred, in 0..1 over the pane's short side.
    vec2 p = centred / PLATE_FRAC * 0.5;

    float d = field(p);

    // The nodal line's half-width, from the field's own screen-space gradient.
    // This is what a fixed field-space cutoff cannot do: it makes the stroke fat
    // where the field is flat and hairline where it is steep, so the same figure
    // would be drawn at two different weights.
    float w = max(fwidth(d) * 0.75, 1e-5);
    float line = 1.0 - smoothstep(0.0, w, d);

    // A second, wider band around the same line. One threshold alone reads as a
    // wireframe; the falloff is what reads as sand settled into a plate, and it
    // is free here because the field is already evaluated.
    float glow = 1.0 - smoothstep(0.0, 0.42, d);

    // Off the plate, and a fade at its edge so the figure does not end on a hard
    // rectangle. Both multiply out rather than branch, because `fwidth` above has
    // to be reached by *every* fragment — a derivative taken in non-uniform
    // control flow is undefined, and a shader that returned early for the
    // off-plate fragments would never reach it at all.
    float extent = max(abs(p.x), abs(p.y));
    float on_plate = step(extent, 1.0);
    float edge = 1.0 - smoothstep(0.92, 1.0, extent);

    // The same body/edge split bars.rs and the flame view make: a dim mass in the
    // accent, the sharp nodal set in the brighter token.
    vec3 col = mix(u_bg.rgb, u_accent.rgb, glow * 0.30);
    col = mix(col, u_progress_fill.rgb, line);
    frag_color = vec4(mix(u_bg.rgb, col, edge * on_plate), 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
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
    // The countdown is **seconds**, spent through `hold_tick` on egui's frame
    // time. It was a frame count, which is a duration only at 60 Hz — see
    // `HOLD_SECS`.
    let dt = painter.ctx().input(|i| i.predicted_dt);
    let held: Option<(usize, usize, f32)> =
        painter.ctx().memory_mut(|m| m.data.get_temp(mode_id()));
    let (current, hold) = match held {
        Some((n, m, hold)) => (Some((n, m)), hold_tick(hold, dt)),
        None => (None, 0.0),
    };
    let (n, m) = pick_mode(&prev, current, hold);
    let switched = Some((n, m)) != current;
    painter.ctx().memory_mut(|mem| {
        mem.data.insert_temp(prev_id(), prev);
        mem.data
            .insert_temp(mode_id(), (n, m, if switched { HOLD_SECS } else { hold }));
    });

    let mut uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    uniforms.modes = [n as f32, m as f32];
    gpu::add_fullscreen(painter, rect, FRAG, uniforms, Target::Screen);
}
