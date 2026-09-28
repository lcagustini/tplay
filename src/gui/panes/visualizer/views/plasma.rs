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

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.plasma")
}

pub const FRAG: &str = r#"
float level(float d) {
    return clamp((d + 60.0) / 60.0, 0.0, 1.0);
}

float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float noise2(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Six octaves of value noise. Four is visibly banded at this scale and eight
// costs a third more for detail the warp already breaks up.
float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 6; i++) {
        sum += amp * noise2(p);
        p = p * 2.03 + 17.1;
        amp *= 0.5;
    }
    return sum;
}


void main() {
    vec2 uv = gl_FragCoord.xy / u_resolution;
    vec2 p = (uv - 0.5) * vec2(u_resolution.x / max(u_resolution.y, 1.0), 1.0) * 3.0;

    // The warp is fed by the spectrum, read as a continuous function of angle so
    // the shear turns around the pane rather than stepping between bands.
    float spin = u_time * 0.15;
    float energy = 0.0;
    for (int i = 0; i < VIZ_BANDS; i++) {
        energy += level(u_bands[i]);
    }
    energy /= float(VIZ_BANDS);

    // A radial read of the spectrum: the band at the angle this fragment sits at
    // shears the field it samples from, so the pattern's arms track the music's
    // shape rather than pulsing on its level alone.
    float ang = atan(p.y, p.x) / 6.2831853 + 0.5;
    float f = clamp(ang, 0.0, 1.0) * float(VIZ_BANDS - 1);
    int lo = int(floor(f));
    int hi = min(lo + 1, VIZ_BANDS - 1);
    float band = mix(level(u_bands[lo]), level(u_bands[hi]), fract(f));

    // Two warp rounds is the minimum that produces the interference this is for;
    // one leaves it a plain fbm, which is a different and duller picture.
    float w = 1.2 + 3.0 * band;
    vec2 q = vec2(fbm(p + vec2(0.0, spin)), fbm(p + vec2(5.2, 1.3 - spin)));
    vec2 r = vec2(fbm(p + w * q + vec2(1.7, 9.2)), fbm(p + w * q + vec2(8.3, 2.8)));
    float v = fbm(p + w * r);

    // The same bg-to-accent heat axis the spectrogram and Nebula use, so the
    // three views agree about what "loud" looks like.
    vec3 col = mix(u_bg.rgb, u_accent.rgb, smoothstep(0.25, 0.75, v));
    col = mix(col, u_progress_fill.rgb, smoothstep(0.68, 0.95, v) * (0.4 + 0.6 * energy));
    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    const ATTACK: f32 = 0.4;
    const RELEASE: f32 = 0.9;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    gpu::add_fullscreen(painter, rect, FRAG, uniforms, Target::Screen);
}
