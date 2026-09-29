//! Spectrogram view — the band array over *time* instead of frequency, as a
//! scrolling waterfall.
//!
//! No new DSP: one `compute_bands` call per frame produces a column. The
//! history is what makes this a different read of the same spectrum rather than
//! a repaint of Bars — a transient shows as a vertical streak, which a bar chart
//! cannot show at all.
//!
//! **The history is the history in a texture, not a `VecDeque`.** The CPU
//! version kept 256 columns of 32 bands in egui memory and repainted all 8 192
//! cells every frame, 60 times a second, to show a picture that had changed in
//! one column. Here the previous frame's columns are already a rendered image, so
//! the one pass shifts the image left by a column, writes the new column down its
//! right-hand edge, and presents the result — and the cost is the same 32 band
//! reads either way, because the fragment shader reads the band at its own `y`
//! regardless of how many columns are in the picture.
//!
//! **The scroll is one column per frame, which is what the CPU version did and is
//! deliberately not changed.** It makes the waterfall's *duration in seconds*
//! frame-rate dependent — 256 columns is 4.3 s at 60 fps and 8.5 s at 30 — and
//! that was already true. The alternative is to scroll by `dt` and the column
//! count then stops being a fixed-width history, which is a larger change to what
//! the view means than a frame-rate wart is worth. `Trails` is the view where the
//! time-based version matters, and it is time-based.

use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Feedback};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.spectrogram")
}

/// The ping-pong pair and its parity, in egui memory so the two targets outlive
/// the frame that asked for them.
fn history_id() -> egui::Id {
    egui::Id::new("tplay.viz.history.spectrogram")
}

/// Pass one: the shifted history plus the new column, into the owned target.
pub const ACCUMULATE: &str = r#"
// The history's length in columns, and the *fewest* columns there can be: a
// column is a whole number of texels (see below), so the target's own width
// decides how many of them there are. As a GLSL literal rather than an
// interpolated value — the harness owns no view's constants, and the version with
// substitution machinery is what once made the test table name a shader the app
// never ran.
const int COLUMNS = 256;

void main() {
    // The target's own size, which the shader cannot be handed: it is quantised
    // to a 64px grid, so it is neither the pane's size nor `u_resolution`.
    ivec2 sz = textureSize(u_prev, 0);

    // **A column is a whole number of texels, and that is load-bearing rather
    // than a rounding preference.** This shifted by a fixed `1.0 / 256.0` of the
    // width and sampled with `texture()`, which is a *fractional* texel offset
    // for every target width that is not a multiple of 256 — and a LINEAR sample
    // at a fractional offset interpolates between two neighbours. Applied to the
    // whole history once a frame, that is a low-pass filter applied 256 times
    // over, so the waterfall arrived uniformly smeared; and because the offset
    // was fractional only on some widths, it looked like a property of the pane
    // rather than of the code. `texelFetch` reads whole texels, so nothing is
    // resampled and the shift is exact at every width. The price is that the
    // column count is the target's width over the column width, so the history
    // spans a different number of seconds on a narrow pane than on a wide one —
    // the honest consequence of being sharp.
    int colw = max(1, sz.x / COLUMNS);

    // The texel this fragment writes, and it is **not** `v_uv * sz` — because a
    // framebuffer's rows run *bottom-up* (row 0 is at NDC y = -1) while `v_uv.y`
    // is 0 at the pane's *top*. Using the top-down index reads one row and writes
    // the mirrored one, which mirrors the whole history on every frame; the
    // present pass then samples that bottom-up texture with a top-down uv, so the
    // two flips alternate and the picture reads as its own mirror image, softened
    // by the present pass's interpolation. x needs no flip: column 0 is the left
    // in both conventions, which is the only reason this was a y-only trap.
    // Clamped before the cast, because `v_uv` reaches 1.0 at the far edge.
    ivec2 p = ivec2(
        clamp(int(v_uv.x * float(sz.x)), 0, sz.x - 1),
        clamp(sz.y - 1 - int(v_uv.y * float(sz.y)), 0, sz.y - 1)
    );

    // The new column: the band at this fragment's height, low bands at the
    // bottom the way the spectrum is read everywhere else. The heat axis is
    // bg -> accent, so the two ends are already in the palette and no view adds a
    // token for it.
    float f = (1.0 - v_uv.y) * float(VIZ_BANDS);
    int lo = min(int(floor(f)), VIZ_BANDS - 1);
    int hi = min(lo + 1, VIZ_BANDS - 1);
    float lvl = mix(level(u_bands[lo]), level(u_bands[hi]), fract(f));
    vec3 fresh = mix(u_bg.rgb, u_accent.rgb, lvl);

    // The history, one column to the right, so the picture moves **left** and
    // the rightmost column is the one the shift vacates. The modulo is what makes
    // it a ring buffer: a clamped read of column -1 *is* column 0, so the one
    // column the shift should have consumed is the one it re-reads, and the
    // leftmost column freezes for ever.
    vec3 old = texelFetch(u_prev, ivec2((p.x + colw) % sz.x, p.y), 0).rgb;

    // ... and the new column lands there, so a transient is a vertical streak
    // that drifts left as it ages. **Writing it on the other edge is the bug this
    // shape exists to prevent:** the leftmost column was overwritten every frame
    // and never shifted, so it sat on the pane as a permanent bright bar, while
    // the rightmost column held a copy of the previous frame's — a second one.
    // The column the shift consumes and the column that gets overwritten are the
    // same column; that is the invariant, and it is the one line of arithmetic
    // that says which edge the new data belongs on.
    frag_color = vec4(p.x >= sz.x - colw ? fresh : old, 1.0);
}
"#;

/// Pass two: the accumulated target, presented.
///
/// A separate program because the two passes need *different* sampling of the
/// same texture: the accumulate must read whole texels or it resamples the
/// history, while this one must interpolate, because the target is quantised to a
/// 64px grid and is therefore usually a little larger than the pane it is
/// stretched across. One program cannot be both, and the wrong one of the two is
/// a picture that looks plausible and is wrong.
pub const PRESENT: &str = r#"
void main() {
    // The same y flip as the accumulate pass, at the other end of the round trip.
    // A texture's row 0 is its bottom and `v_uv.y` is 0 at the pane's top, so
    // sampling `v_uv` straight through would present the target upside down. The
    // two passes must agree: flip in both and the target survives the round trip
    // unchanged, flip in neither and the same, flip in one and every frame is the
    // mirror of the last. That is the whole requirement, and it is invisible
    // until it is not.
    vec3 c = texture(u_prev, vec2(v_uv.x, 1.0 - v_uv.y)).rgb;
    // The `u_bg` floor is `Trails`' trick and here it carries a second job. A
    // fresh target is cleared to transparent black, so the history reads as a
    // black hole in the pane for the 256 frames it takes to fill. Flooring at the
    // pane's own background makes an unwarmed waterfall read as the surface it is
    // drawn on rather than as a gap in it, and a cleared column is the same
    // colour as a silent one, so the warm-up is invisible.
    frag_color = vec4(mix(u_bg.rgb, c, 0.92), 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // No release smoothing: a waterfall is the place the raw transient is the
    // point, and the history already shows decay over the columns behind.
    const ATTACK: f32 = 0.6;
    const RELEASE: f32 = 0.6;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let mut history: Feedback = painter
        .ctx()
        .memory_mut(|m| m.data.get_temp(history_id()).unwrap_or_default());

    // A pane that has just been resized, or a view that has just been switched
    // to, must not present the *other* one's history — see `trails.rs`, which
    // says the same about its own targets. A size change is caught inside
    // `Feedback::draw`; a view switch is not, because the targets are the right
    // size and hold the wrong picture.
    if painter
        .ctx()
        .memory(|m| m.data.get_temp::<bool>(was_selected_id()))
        != Some(true)
    {
        history.invalidate();
    }
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(was_selected_id(), true));

    // Two programs, in one callback: the accumulate pass reads one target and
    // writes the other, and the present pass draws exactly what was accumulated.
    history.draw(
        painter,
        rect,
        ACCUMULATE,
        PRESENT,
        gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx()),
    );
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(history_id(), history));
}

/// Whether [`draw`] ran on the previous frame. A view switch is a gap in a view's
/// own `draw` calls, and that gap is the only signal any view gets.
fn was_selected_id() -> egui::Id {
    egui::Id::new("tplay.viz.spectrogram.selected")
}
