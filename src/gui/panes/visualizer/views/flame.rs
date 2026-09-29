//! Flame view — the band array as a filled ridgeline.
//!
//! No new DSP: one `compute_bands` call. Where Bars draws 32 separate columns,
//! this closes them into one contour, which reads as a spectrum's *shape* and
//! its evolution rather than as 32 unrelated heights.
//!
//! The look is the ridgeline used by audio-spectrum visualisers (the effect
//! usually credited to the `flam3` renderer). Named in the past with a link to
//! that name's source; left unnamed here on purpose, because "flame" resolves to
//! Brendan Gregg's *profiling* flame graph, which is a different thing built from
//! stack traces, and a wrong reference is worse than none. What the view draws is
//! the sentence above.
//!
//! **It is tongues, not a wave, and the difference is the point.** The first
//! shader version was a ridgeline — a contour across the pane — which is the
//! *silhouette of a wave*: it read as a wave because that is what it was, and the
//! name had been doing work the drawing was not. What separates a flame is
//! vertical, not horizontal: it stands on a floor, it comes to a point, it is
//! hottest where the fuel is and cools toward the tips, and it licks sideways with
//! the draught. So the view is three Gaussian tongues at different positions,
//! each driven by a different part of the spectrum and flickering on its own
//! phase, sheared sideways by an amount that grows with height. The band array
//! still supplies the heights, so it is still a music visualisation — it just no
//! longer looks like its own name.
//!
//! **This view is the second half of the convexity story.** The old CPU ridgeline
//! was a wiggly mass, so it was decomposed into `VIZ_BANDS - 1` trapezoids for a
//! hand-built mesh — and a sawtooth spectrum, the case that actually broke, made
//! every one of those a different shape. A fragment shader asks "is my y inside
//! this tongue at my x", and the answer is a comparison.

use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.flame")
}

pub const FRAG: &str = r#"
// The level at this fragment's x, interpolated between the two bands around it.
float band(float x) {
    float f = clamp(x, 0.0, 1.0) * float(VIZ_BANDS - 1);
    int lo = int(floor(f));
    int hi = min(lo + 1, VIZ_BANDS - 1);
    return mix(level(u_bands[lo]), level(u_bands[hi]), fract(f));
}

// One tongue's cross-section. A Gaussian rather than a rectangle, so the silhouette
// has rounded shoulders and comes to a point at the top — which is most of what
// separates a flame from a bar. The exponent is what sharpens the point: at 1.0
// this is a soft blob, and the tip has to be a corner to read as fire.
float tongue(float x, float centre, float width) {
    float d = (x - centre) / width;
    return exp(-d * d * 3.2);
}

void main() {
    vec2 uv = v_uv;
    float t = u_time;

    // The floor the tongues stand on, and how far up this fragment is inside them:
    // 0 at the floor, 1 at the tip of a full-height tongue. **Height in units of
    // the pane's height**, so the vertical scale is the one the figure is drawn in.
    float base = 0.97;
    float rise = (base - uv.y) / 0.90;

    // Fire licks *sideways*, and the shear grows with height — the further from
    // the fuel the more the draught moves it. Without this the tongues are static
    // silhouettes that only scale, which is what reads as a bar chart.
    //
    // **`x` is in units of the pane's height, not its width.** A tongue's
    // cross-section is a Gaussian and its height is measured in the same units,
    // so a `tongue()` fed a raw `uv.x` draws a *stretched* flame on a wide pane
    // and a pinched one on a tall one: the shoulders and the tip change shape
    // with the splitter, which is the whole of what a flame's silhouette is.
    // Scaling x by the aspect ratio is the same correction radial.rs, chladni.rs
    // and trails.rs make, and the three tongue positions below are in the same
    // units as their widths.
    float x = (uv.x - 0.5) * (u_resolution.x / max(u_resolution.y, 1.0));
    float lick = x + 0.020 * sin(t * 1.3 + rise * 4.5);

    // Three tongues, each driven by a different part of the spectrum and
    // flickering on its own phase, so the shape changes rather than the scale.
    float h = 0.0;
    h = max(h, 0.80 * band(0.08) * (0.5 + 0.5 * sin(t * 2.1))      * tongue(lick, -0.11, 0.115));
    h = max(h, 1.00 * band(0.30) * (0.5 + 0.5 * sin(t * 1.7 + 2.1)) * tongue(lick, 0.06, 0.130));
    h = max(h, 0.66 * band(0.58) * (0.5 + 0.5 * sin(t * 2.6 + 4.2)) * tongue(lick, 0.26, 0.095));

    float inside = step(0.0, rise);
    float body = inside * step(rise, h);
    // Hot at the base and cooling toward the tips. A flame reads as fire because
    // of this gradient; without it the shape is just a lit region.
    float heat = body * (1.0 - smoothstep(0.0, 0.55, rise)) * (0.55 + 0.45 * h);
    // The glow just outside the surface, which is the other half of why fire
    // looks like fire. It reaches slightly below the surface, so it reads as a
    // halo rather than as a second, inner band.
    float glow = smoothstep(-0.13, 0.03, h - rise) * inside;

    // A dim bed along the floor, so the tongues stand on something and the bottom
    // of the pane is never empty. **Full width, so it is a bed rather than a
    // third one** — a bed under three tongues only has to reach the outer two.
    float bed = inside * (1.0 - smoothstep(0.0, 0.10, rise)) * (0.25 + 0.75 * band(0.5));

    vec3 col = u_bg.rgb;
    col = mix(col, u_accent.rgb, glow * 0.22);
    col = mix(col, u_accent.rgb, body * 0.75);
    col = mix(col, u_progress_fill.rgb, heat * 0.85);
    col = mix(col, u_accent.rgb, bed * 0.30);
    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Brisker than bars.rs: a contour's edge is the signal, and the fast release
    // is what makes the decay trail read as a separate ridge.
    const ATTACK: f32 = 0.5;
    const RELEASE: f32 = 0.85;
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
