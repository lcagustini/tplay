//! Plasma view — a domain-warped 2D fractal field, iterated per fragment.
//!
//! The second shader view, and the one that proves the first was not a special
//! case. Nebula needed the harness; this needed the harness *plus* a `draw` fn
//! and a shader string, and nothing else changed. That is the whole claim of the
//! design, and this file is the evidence for it.
//!
//! Where it differs from Nebula is the shape of the work: Nebula marches a ray
//! through a volume, this evaluates a 2D field once per fragment with a warp
//! applied to its own input between octaves. Cheaper per fragment, and it is a
//! genuinely different *look* rather than a variation — a filled interference
//! pattern against Nebula's depth.

use super::frame_dt;
use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu;
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.plasma")
}

/// This view's own functions. See `ShaderView::helpers` for why they are a
/// separate slot from the body.
pub const HELPERS: &str = r#"
// A local, not the parameter: WGSL parameters are read-only too, and `ptr` is no
// escape because every call site passes an expression.
fn hash21(v : vec2<f32>) -> f32 {
    var p = fract(v * vec2<f32>(123.34, 456.21));
    p = p + vec2<f32>(dot(p, p + 45.32));
    return fract(p.x * p.y);
}

fn vnoise2(p : vec2<f32>) -> f32 {
    let i = floor(p);
    var f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Six octaves of value noise. Four is visibly banded at this scale and eight
// costs a third more for detail the warp already breaks up.
fn fbm(p : vec2<f32>) -> f32 {
    var q = p;
    var sum = 0.0;
    var amp = 0.5;
    for (var i = 0; i < 6; i = i + 1) {
        sum = sum + amp * vnoise2(q);
        q = q * 2.03 + vec2<f32>(17.1);
        amp = amp * 0.5;
    }
    return sum;
}
"#;

pub const FRAG: &str = r#"
    let uv = v_uv;
    let p = (uv - 0.5) * vec2<f32>(u_resolution().x / max(u_resolution().y, 1.0), 1.0) * 3.0;

    // The warp is fed by the spectrum, read as a continuous function of angle so
    // the shear turns around the pane rather than stepping between bands.
    let spin = u_time() * 0.15;
    var energy = 0.0;
    for (var i = 0u; i < VIZ_BANDS; i = i + 1u) {
        energy = energy + level(band_at(i));
    }
    energy = energy / f32(VIZ_BANDS);

    // A radial read of the spectrum: the band at the angle this fragment sits at
    // shears the field it samples from, so the pattern's arms track the music's
    // shape rather than pulsing on its level alone.
    //
    // **The harness's projection, not an `atan2`.** This used to be
    // `atan(p.y, p.x) / TAU + 0.5` indexed straight into the band array, and
    // `atan`'s branch cut runs along `p.x < 0` at `p.y == 0` — the pane's left
    // edge. So `f` stepped 32 -> 0, `band` stepped with it, `w` stepped with that,
    // and the whole `fbm` field stepped: a hard seam, and unfixable by a fade
    // because the break was in the *function*. See `gpu::BEARING`.
    let band = spectrum_at_bearing(p);

    // Two warp rounds is the minimum that produces the interference this is for;
    // one leaves it a plain fbm, which is a different and duller picture.
    let w = 1.2 + 3.0 * band;
    let q = vec2<f32>(fbm(p + vec2<f32>(0.0, spin)), fbm(p + vec2<f32>(5.2, 1.3 - spin)));
    let r = vec2<f32>(fbm(p + w * q + vec2<f32>(1.7, 9.2)), fbm(p + w * q + vec2<f32>(8.3, 2.8)));
    let v = fbm(p + w * r);

    // The same bg-to-accent heat axis the spectrogram and Nebula use, so the
    // three views agree about what "loud" looks like.
    var col = mix(u_bg().rgb, u_accent().rgb, smoothstep(0.25, 0.75, v));
    col = mix(col, u_progress_fill().rgb, smoothstep(0.68, 0.95, v) * (0.4 + 0.6 * energy));
    frag_color = vec4<f32>(col, 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    const ATTACK_MS: f32 = 32.6;
    const RELEASE_MS: f32 = 7.2;
    compute_bands(viz, &mut prev, frame_dt(painter), ATTACK_MS, RELEASE_MS);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    gpu::add_fullscreen(painter, rect, HELPERS, FRAG, uniforms);
}
