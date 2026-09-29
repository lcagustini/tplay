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

use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.bars")
}

pub const FRAG: &str = r#"
void main() {
    vec2 uv = v_uv;

    // Which column this fragment is in, and where inside it. The cell is
    // VIZ_BANDS wide, so the bar occupies 10%..90% of its cell — the gap the CPU
    // version left by insetting the rect by a fifth on each side.
    float x = uv.x * float(VIZ_BANDS);
    int idx = min(int(floor(x)), VIZ_BANDS - 1);
    float within = fract(x);
    float in_bar = step(0.1, within) * step(within, 0.9);

    // 0 at the centre line, 1 at the pane's top and bottom edges, so the mirrored
    // half-height is a direct comparison against the level.
    float across = abs(uv.y - 0.5) * 2.0;
    float lvl = level(u_bands[idx]);
    // 0.9 of the full height, which is the 0.45 half-height the CPU version
    // scaled its rects by.
    float body = step(across, lvl * 0.9) * in_bar;

    // The centre tick every fourth column. `mod` rather than an integer cast
    // because `idx` is already an int and `%` on a negative is not a thing here
    // — but the value is fractional-safe either way at this magnitude.
    float tick_col = step(0.5, mod(float(idx), 4.0));
    float tick = step(across, 0.012) * in_bar * tick_col;

    // Alpha composited by hand, because the harness disables blending for a
    // callback (egui blends its own primitives; a fullscreen quad must not be
    // blended over the background it is replacing).
    vec3 col = u_bg.rgb;
    col = mix(col, u_accent.rgb, body * (0.4 + 0.6 * lvl));
    col = mix(col, u_accent.rgb * 0.5, tick);
    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Attack/release constants (per-frame, 60 FPS assumed)
    const ATTACK: f32 = 0.3;
    const RELEASE: f32 = 0.92;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    gpu::add_fullscreen(
        painter,
        rect,
        FRAG,
        gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx()),
        Target::Screen,
    );
}
