//! Bars view — FFT bands (32 log-spaced, dB-normalized) with attack/release
//! smoothing for the classic WMP bars feel.
//!
//! The classic 32-column bar chart is the one view here that was never a
//! candidate for a field problem, and that was the point of converting it
//! anyway: **everything** in this set is now one fragment shader over the pane,
//! so there is one code path to understand, one set of failure modes, and no
//! question about which views work on a driver with no GL.
//!
//! What the CPU version cost here: 32 `rect_filled` calls plus 8 `hline`s a
//! frame, and a smooth `fbm`-free shader is ~15 instructions per fragment, so
//! the whole thing went from geometry to a comparison.

use super::frame_dt;
use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu;
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.bars")
}

pub const HELPERS: &str = "";

pub const FRAG: &str = r#"
    let uv = v_uv;

    // Which column this fragment is in, and where inside it. The cell is
    // VIZ_BANDS wide, so the bar occupies most of its cell — the gap the CPU
    // version left by insetting the rect by a fifth on each side.
    let x = uv.x * f32(VIZ_BANDS);
    let idx = min(i32(floor(x)), i32(VIZ_BANDS) - 1);
    let within = fract(x);

    // **The gap is a pixel quantity, not a fraction of a cell, and that is the
    // whole fix for a small pane.** A fixed 10% gap is 0.6px once a cell is 6px
    // wide, so on a narrow pane the bars and their gaps land on the same pixels
    // and the row shimmers as the splitter moves — a hard `step` on a sub-pixel
    // feature is sampling a function faster than the screen can show it. Half a
    // pixel of gap is the smallest a gap can be and still be a gap; below that
    // it stops pretending.
    let px = fwidth(x);
    let gap = max(0.1, 0.5 * px);
    let in_bar = step(gap, within) * step(within, 1.0 - gap);

    // 0 at the centre line, 1 at the pane's top and bottom edges, so the mirrored
    // half-height is a direct comparison against the level.
    let across = abs(uv.y - 0.5) * 2.0;
    let lvl = level(band_at(u32(idx)));
    // 0.9 of the full height, which is the 0.45 half-height the CPU version
    // scaled its rects by.
    let body = step(across, lvl * 0.9) * in_bar;

    // The centre tick every fourth column. `%` on the integer index, which is
    // never negative here, rather than a float modulus.
    //
    // A pixel tall, for the same reason as the gap: a fixed 1.2% of the pane's
    // height is 5px in a tall pane and a third of one in a short one, and a tick
    // that is sub-pixel is either absent or aliased depending on where the
    // splitter happened to be.
    let tick_h = max(0.012, 0.5 / max(u_resolution().y, 1.0) * 2.0);
    let tick_col = step(0.5, f32(idx % 4));
    let tick = step(across, tick_h) * in_bar * tick_col;

    // **Below about two pixels per cell there is no longer a bar chart, and
    // drawing one anyway is what shimmers.** A 32-band row needs room to be a row;
    // rather than alias into noise, the whole thing dims away as the cells
    // approach the sampling limit, so a pane too small for this view reads as
    // "too small" instead of as noise. This is the one degradation here that is
    // *deliberate* rather than a consequence — every other property of the view
    // is resolution-independent, and this is the single case where the honest
    // answer is to draw less rather than draw differently.
    let cells_px = 1.0 / max(px, 1e-6);
    let legible = smoothstep(1.0, 2.5, cells_px);

    // Alpha composited by hand, because the pipeline declares no blend state
    // (egui blends its own primitives; a fullscreen quad must not be blended over
    // the background it is replacing).
    var col = u_bg().rgb;
    col = mix(col, u_accent().rgb, body * (0.4 + 0.6 * lvl) * legible);
    col = mix(col, u_accent().rgb * 0.5, tick * legible);
    frag_color = vec4<f32>(col, 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Attack/release constants (per-frame, 60 FPS assumed)
    const ATTACK_MS: f32 = 46.7;
    const RELEASE_MS: f32 = 6.6;
    compute_bands(viz, &mut prev, frame_dt(painter), ATTACK_MS, RELEASE_MS);

    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    gpu::add_fullscreen(
        painter,
        rect,
        HELPERS,
        FRAG,
        gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx()),
    );
}
