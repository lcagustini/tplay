//! Wave view — mirrored time-domain peak envelope silhouette.
//!
//! The one view that needed a **new input kind** rather than a new drawing
//! technique: it is a function of the ring buffer's samples, not of the spectrum,
//! so the harness gained `u_wave` — a fixed-length peak envelope. The bucket
//! count is a constant because an array's length is part of its WGSL declaration
//! and cannot move with the splitter; see `WAVE_BUCKETS`.
//!
//! The envelope is drawn as one filled column with an outline on both edges, so
//! a fragment is a comparison against the interpolated envelope at its own x and
//! nothing else. The CPU version walked `wave.windows(2)` and emitted a
//! `rect_filled` and two `line_segment`s per column — about 1500 primitives a
//! frame for a shape that is, in the end, a function of x.

use crate::audio::viz::{DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu;
use crate::gui::panes::visualizer::views::frame_dt;
use crate::gui::theme::Palette;
use eframe::egui;

/// This view's own functions. See `ShaderView::helpers` for why they are a
/// separate slot from the body.
pub const HELPERS: &str = r#"
// The envelope at this fragment's x, interpolated between the two buckets around
// it. `wave_at` holds peak |sample| in 0..1 per bucket, oldest at 0.
fn envelope(x : f32) -> f32 {
    let f = clamp(x, 0.0, 1.0) * f32(WAVE_BUCKETS - 1u);
    let lo = i32(floor(f));
    let hi = min(lo + 1, i32(WAVE_BUCKETS) - 1);
    return mix(wave_at(u32(lo)), wave_at(u32(hi)), fract(f));
}
"#;

pub const FRAG: &str = r#"
    let uv = v_uv;

    let w = envelope(uv.x);
    // 0 at the centre line, 1 at the pane's top and bottom edges. The 0.8 factor
    // is the 0.4 half-height the CPU version scaled its columns by.
    let across = abs(uv.y - 0.5) * 2.0;
    let edge = w * 0.8;

    let body = step(across, edge);
    // The outline, as a distance to the envelope rather than as a stroke between
    // two sampled points — which is what made it robust at any pane width. Half a
    // pixel in pixels, so the line is the same weight at any scale.
    let half_px = 0.5 / u_resolution().y * 2.0;
    let outline = 1.0 - smoothstep(half_px, half_px * 2.0, abs(across - edge));

    // 80/255 is the CPU version's fill alpha.
    var col = mix(u_bg().rgb, u_accent().rgb, body * 0.314);
    col = mix(col, u_progress_fill().rgb, outline);
    frag_color = vec4<f32>(col, 1.0);
"#;

pub fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    viz: &crate::audio::viz::VizBuf,
    palette: &Palette,
) {
    // This view's one advancing read, and the only reason `VizBuf::advance`
    // exists. See its doc: without it the envelope is a frozen burst.
    viz.advance(frame_dt(painter));
    gpu::add_fullscreen(
        painter,
        rect,
        HELPERS,
        FRAG,
        gpu::Uniforms::pack(viz, [DB_FLOOR; VIZ_BANDS], palette, rect, painter.ctx()),
    );
}
