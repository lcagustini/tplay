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

use super::frame_dt;
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
pub const HELPERS: &str = r#"
// The history's length in columns, and the *fewest* columns there can be: a
// column is a whole number of texels (see below), so the target's own width
// decides how many of them there are. As a WGSL literal rather than an
// interpolated value — the harness owns no view's constants, and the version with
// substitution machinery is what once made the test table name a shader the app
// never ran.
const COLUMNS : i32 = 256;
"#;

pub const ACCUMULATE: &str = r#"
    // The target's own size, which the shader cannot be handed: it is quantised
    // to a 64px grid, so it is neither the pane's size nor `u_resolution`.
    let sz = textureDimensions(u_prev);

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
    let colw = max(1, i32(sz.x) / COLUMNS);

    // The texel this fragment writes. **There is no `y` flip here, and that is
    // the load-bearing half of this view.** Row 0 of a framebuffer is at NDC
    // `y = +1` in wgpu, which is the pane's *top* — the same place `v_uv.y = 0`
    // is — so the texel a fragment lands in is simply `v_uv * sz`, with no
    // conversion. The OpenGL habit (`gl_FragCoord` is bottom-up, and GL row 0 is
    // at NDC `-1`) says otherwise, and taking it here is what produced the
    // vertical striping this view had: the pass read `sz.y - 1 - …` while the
    // rasteriser wrote `int(v_uv.y * sz.y)`, so each row read the row its mirror
    // image read. Mirroring is an involution, so that did not smear the picture —
    // it made **alternate columns** of the history the mirror of their
    // neighbours, which at two pixels a column is a stripe. `trails.rs` samples
    // its target with no flip anywhere and is therefore immune, because its
    // transform is a spin and a zoom, which a whole-image flip happens to
    // commute with.
    // Clamped before the cast, because `v_uv` reaches 1.0 at the far edge.
    let p = vec2<i32>(
        clamp(i32(v_uv.x * f32(sz.x)), 0, i32(sz.x) - 1),
        clamp(i32(v_uv.y * f32(sz.y)), 0, i32(sz.y) - 1)
    );

    // The new column: the band at this fragment's height, low bands at the
    // bottom the way the spectrum is read everywhere else. The heat axis is
    // bg -> accent, so the two ends are already in the palette and no view adds a
    // token for it.
    // **A band is a flat step, not a ramp, and that is what makes the waterfall
    // read as bands.** This interpolated between the two neighbouring bands, so a
    // 32-band history over a 1150px pane painted 36 rows of gradient per band —
    // every band boundary was a smooth blend and the picture came out looking
    // out of focus. The eye reads a waterfall's frequency axis as discrete
    // regions, and a ramp erases exactly the edges it is there to show. `step`
    // picks one band per row; the 1px boundary is left crisp for the same reason.
    let f = (1.0 - v_uv.y) * f32(VIZ_BANDS);
    let lo = clamp(i32(floor(f)), 0, i32(VIZ_BANDS) - 1);
    let lvl = level(band_at(u32(lo)));
    let fresh = mix(u_bg().rgb, u_accent().rgb, lvl);

    // The history, one column to the right, so the picture moves **left** and
    // the rightmost column is the one the shift vacates. The modulo is what makes
    // it a ring buffer: a clamped read of column -1 *is* column 0, so the one
    // column the shift should have consumed is the one it re-reads, and the
    // leftmost column freezes for ever.
    // **`textureLoad`, the WGSL `texelFetch`**: whole texels, no sampler, no
    // resampling. An interpolated read here is a low-pass filter applied to the
    // whole history once a frame, 256 times over.
    let old = textureLoad(u_prev, vec2<i32>((p.x + colw) % i32(sz.x), p.y), 0).rgb;

    // ... and the new column lands there, so a transient is a vertical streak
    // that drifts left as it ages. **Writing it on the other edge is the bug this
    // shape exists to prevent:** the leftmost column was overwritten every frame
    // and never shifted, so it sat on the pane as a permanent bright bar, while
    // the rightmost column held a copy of the previous frame's — a second one.
    // The column the shift consumes and the column that gets overwritten are the
    // same column; that is the invariant, and it is the one line of arithmetic
    // that says which edge the new data belongs on.
    if (p.x >= i32(sz.x) - colw) {
        frag_color = vec4<f32>(fresh, 1.0);
    } else {
        frag_color = vec4<f32>(old, 1.0);
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
    // A straight read-back, **with no `y` flip** — the same reason the accumulate
    // pass has none: `textureSampleLevel`'s `uv.y = 0` is texture row 0, row 0 is
    // at NDC `+1`, and that is the pane's top. So the coordinate the accumulate
    // pass wrote with is the coordinate this one reads with, and the round trip is
    // the identity. A flip here would mirror the history on presentation without
    // the accumulate pass agreeing, which is the mirror-image of the striping
    // above and just as invisible until it is not.
    // `textureSampleLevel` rather than `textureSample`: the level is explicit
    // because this pass reads a single-mip target and the two are only equivalent
    // when there is no mip chain to pick a level from.
    var c = textureSampleLevel(u_prev, u_samp, v_uv, 0.0).rgb;
    // The `u_bg` floor is `Trails`' trick and here it carries a second job. A
    // fresh target is cleared to transparent black, so the history reads as a
    // black hole in the pane for the 256 frames it takes to fill. Flooring at the
    // pane's own background makes an unwarmed waterfall read as the surface it is
    // drawn on rather than as a gap in it, and a cleared column is the same
    // colour as a silent one, so the warm-up is invisible.
    // **`1.0`, not a floor below it.** The floor was there so an unwarmed target
    // read as the pane's surface rather than as a hole in it, and it also washed
    // 8% of the background into every column of the history — which is a second
    // blur, and a subtler one than the ramp above because it is uniform. The
    // warm-up problem it was solving does not need it: a fresh target is cleared
    // to the background colour, so an empty history *is* the surface, and a
    // cleared column is the background rather than a black bar.
    frag_color = vec4<f32>(c, 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // No release smoothing: a waterfall is the place the raw transient is the
    // point, and the history already shows decay over the columns behind.
    const ATTACK_MS: f32 = 18.2;
    const RELEASE_MS: f32 = 18.2;
    compute_bands(viz, &mut prev, frame_dt(painter), ATTACK_MS, RELEASE_MS);
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
        HELPERS,
        ACCUMULATE,
        PRESENT,
        gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx()),
        viz.take_arrival(),
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
