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

use super::frame_dt;
use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu;
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.flame")
}

/// This view's own functions. See `ShaderView::helpers` for why they are a
/// separate slot from the body.
pub const HELPERS: &str = r#"
// The level at this fragment's x, interpolated between the two bands around it.
fn band(x : f32) -> f32 {
    let f = clamp(x, 0.0, 1.0) * f32(VIZ_BANDS - 1u);
    let lo = i32(floor(f));
    let hi = min(lo + 1, i32(VIZ_BANDS) - 1);
    return mix(level(band_at(u32(lo))), level(band_at(u32(hi))), fract(f));
}

// One tongue's cross-section. A Gaussian rather than a rectangle, so the silhouette
// has rounded shoulders and comes to a point at the top — which is most of what
// separates a flame from a bar. The exponent is what sharpens the point: at 1.0
// this is a soft blob, and the tip has to be a corner to read as fire.
fn tongue(x : f32, centre : f32, width : f32) -> f32 {
    let d = (x - centre) / width;
    return exp(-d * d * 3.2);
}
"#;

pub const FRAG: &str = r#"
    let uv = v_uv;
    let t = u_time();

    // The floor the tongues stand on, and how far up this fragment is inside them:
    // 0 at the floor, 1 at the tip of a full-height tongue. **Height in units of
    // the pane's height**, so the vertical scale is the one the figure is drawn in.
    let base = 0.97;
    let rise = (base - uv.y) / 0.90;

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
    // and trails.rs make.
    let x = (uv.x - 0.5) * (u_resolution().x / max(u_resolution().y, 1.0));
    // The draught's reach, in those same units. Small on purpose: it is a lean,
    // not a displacement, and scaling it with `span` would make a wide pane's
    // tongues shear further than a narrow one's.
    let lick = x + 0.020 * sin(t * 1.3 + rise * 4.5);

    // **How far the figure spreads, as a fraction of the pane's width.**
    //
    // The aspect correction above makes `x` measure height, which is what keeps a
    // Gaussian round — but it also means the drawing space is ±half the *aspect
    // ratio*, so on a wide pane it is very much wider than it is tall. Positions
    // written as plain numbers are therefore fractions of the pane's *height*: at
    // −0.11, 0.06 and 0.26 the three tongues occupied the middle tenth of a 6:1
    // pane and left the rest empty. Every position and width below is a fraction
    // of `span` so the flame fills the pane and stays round at any shape.
    //
    // The floor on `span` is what keeps a very tall pane from collapsing the
    // tongues into one another: below it the fraction would exceed the height the
    // figure is drawn in, and the tongues would overlap into a single blob.
    let span = max(u_resolution().x / max(u_resolution().y, 1.0), 0.42);

    // Three tongues, each driven by a different part of the spectrum and
    // flickering on its own phase, so the shape changes rather than the scale.
    var h = 0.0;
    h = max(h, 0.80 * band(0.08) * (0.5 + 0.5 * sin(t * 2.1))      * tongue(lick, span * -0.26, span * 0.115));
    h = max(h, 1.00 * band(0.30) * (0.5 + 0.5 * sin(t * 1.7 + 2.1)) * tongue(lick, span * 0.04,  span * 0.130));
    h = max(h, 0.66 * band(0.58) * (0.5 + 0.5 * sin(t * 2.6 + 4.2)) * tongue(lick, span * 0.30,  span * 0.095));

    let inside = step(0.0, rise);
    let body = inside * step(rise, h);
    // Hot at the base and cooling toward the tips. A flame reads as fire because
    // of this gradient; without it the shape is just a lit region.
    let heat = body * (1.0 - smoothstep(0.0, 0.55, rise)) * (0.55 + 0.45 * h);
    // The glow just outside the surface, which is the other half of why fire
    // looks like fire. It reaches slightly below the surface, so it reads as a
    // halo rather than as a second, inner band.
    let glow = smoothstep(-0.13, 0.03, h - rise) * inside;

    // A dim bed along the floor, so the tongues stand on something and the bottom
    // of the pane is never empty. **Full width, so it is a bed rather than a
    // third one** — a bed under three tongues only has to reach the outer two.
    let bed = inside * (1.0 - smoothstep(0.0, 0.10, rise)) * (0.25 + 0.75 * band(0.5));

    var col = u_bg().rgb;
    col = mix(col, u_accent().rgb, glow * 0.22);
    col = mix(col, u_accent().rgb, body * 0.75);
    col = mix(col, u_progress_fill().rgb, heat * 0.85);
    col = mix(col, u_accent().rgb, bed * 0.30);
    frag_color = vec4<f32>(col, 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Brisker than bars.rs: a contour's edge is the signal, and the fast release
    // is what makes the decay trail read as a separate ridge.
    const ATTACK_MS: f32 = 24.1;
    const RELEASE_MS: f32 = 8.8;
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
