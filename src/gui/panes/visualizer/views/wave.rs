//! Wave view — mirrored time-domain peak envelope silhouette.
//!
//! The one view that needed a **new input kind** rather than a new drawing
//! technique: it is a function of the ring buffer's samples, not of the spectrum,
//! so the harness gained `u_wave` — a fixed-length peak envelope. The bucket
//! count is a constant because a uniform array's length is part of its GLSL
//! declaration and cannot move with the splitter; see `WAVE_BUCKETS`.
//!
//! The envelope is drawn as one filled column with an outline on both edges, so
//! a fragment is a comparison against the interpolated envelope at its own x and
//! nothing else. The CPU version walked `wave.windows(2)` and emitted a
//! `rect_filled` and two `line_segment`s per column — about 1500 primitives a
//! frame for a shape that is, in the end, a function of x.

use crate::audio::viz::{DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

pub const FRAG: &str = r#"
// The envelope at this fragment's x, interpolated between the two buckets around
// it. `u_wave` holds peak |sample| in 0..1 per bucket, oldest at 0.
float envelope(float x) {
    float f = clamp(x, 0.0, 1.0) * float(WAVE_BUCKETS - 1);
    int lo = int(floor(f));
    int hi = min(lo + 1, WAVE_BUCKETS - 1);
    return mix(u_wave[lo], u_wave[hi], fract(f));
}

void main() {
    vec2 uv = v_uv;

    float w = envelope(uv.x);
    // 0 at the centre line, 1 at the pane's top and bottom edges. The 0.8 factor
    // is the 0.4 half-height the CPU version scaled its columns by.
    float across = abs(uv.y - 0.5) * 2.0;
    float edge = w * 0.8;

    float body = step(across, edge);
    // The outline, as a distance to the envelope rather than as a stroke between
    // two sampled points — which is what made it robust at any pane width. Half a
    // pixel in pixels, so the line is the same weight at any scale.
    float half_px = 0.5 / u_resolution.y * 2.0;
    float outline = 1.0 - smoothstep(half_px, half_px * 2.0, abs(across - edge));

    // 80/255 is the CPU version's fill alpha.
    vec3 col = mix(u_bg.rgb, u_accent.rgb, body * 0.314);
    col = mix(col, u_progress_fill.rgb, outline);
    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    viz: &crate::audio::viz::VizBuf,
    palette: &Palette,
) {
    gpu::add_fullscreen(
        painter,
        rect,
        FRAG,
        gpu::Uniforms::pack(viz, [DB_FLOOR; VIZ_BANDS], palette, rect, painter.ctx()),
        Target::Screen,
    );
}
