//! Radial view — the same 32 log-spaced bands as Bars, painted around a circle.
//!
//! No new DSP: the same `compute_bands` call, a different placement. This is the
//! cheapest new look in the set, and the reason it exists rather than a
//! "Circular Bars" label on the existing view.
//!
//! **This view is why the convexity work disappeared.** A ring sector is *not*
//! convex — it has the middle cut out — and `epaint`'s closed-path fill is a
//! triangle fan from its first vertex, so the CPU version had to decompose every
//! sector into `VIZ_BANDS * 4` convex quads and hand them to a hand-built mesh,
//! with tests pinning the convexity, the piece count and the containment. A
//! fragment shader has no such constraint: a ring sector is exactly the set of
//! fragments whose radius falls in a range, so it is one comparison and the
//! whole decomposition, the fan constraint and its three tests are gone.

use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.radial")
}

pub const FRAG: &str = r#"
void main() {
    vec2 uv = v_uv;
    // Aspect-corrected, so the ring is a circle rather than an ellipse.
    vec2 centred = (uv - 0.5) * vec2(u_resolution.x / max(u_resolution.y, 1.0), 1.0);

    // 0.08 padding, so a full-scale band never touches the pane edge. The radius
    // is in units of the half-short-side, so 0.5 * (1 - 0.08) is the outer edge.
    float outer_r = 0.5 * 0.92;
    float inner_r = outer_r * 0.65;
    float span = outer_r - inner_r;

    float r = length(centred);

    // Band 0 at the top, increasing clockwise, which is what the CPU version's
    // `-PI/2` start gave. The `fract` closes the loop back onto band 0.
    float a = fract(0.25 - atan(centred.y, centred.x) / 6.2831853);
    float f = a * float(VIZ_BANDS);
    int idx = min(int(floor(f)), VIZ_BANDS - 1);
    float within = fract(f);
    // A gap between bands, matching the one bars.rs leaves: 0.9 of each cell.
    float in_band = step(0.05, within) * step(within, 0.95);

    float lvl = level(u_bands[idx]);
    float band = step(inner_r, r) * step(r, inner_r + lvl * span) * in_band;

    // 80..255 alpha over the level, the CPU version's ramp, composited by hand
    // because the harness draws callbacks with blending off.
    vec3 col = mix(u_bg.rgb, u_accent.rgb, band * (0.314 + 0.686 * lvl));
    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Same attack/release as bars.rs: the smoothing is what makes a spectrum
    // readable, so a different view of it should not be jitterier.
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
