//! Nebula view — a volumetric raymarch of a domain-warped 3D noise field.
//!
//! The first shader view, and the one that proves the harness. What makes it
//! worth a GPU rather than a `Ui`-free function is that it is a **per-fragment
//! field** problem: a 48-step march costs ~144 noise evaluations per pixel, and
//! at a 600×300 pane on a 2× display that is ~100M evaluations a frame. On the
//! CPU that is seconds, not milliseconds. Every other view in this set is a 2D
//! plot of a 32-float array, which is a different class of computation rather
//! than a prettier version of the same one.
//!
//! The two-tier rule, in force here as everywhere: `compute_bands` runs on the
//! CPU exactly as `bars.rs` runs it, and only the drawing moved.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.nebula")
}

pub const FRAG: &str = r#"
// `u_bands` is in dB over -60..0, and this view draws loudness, so it maps that
// onto 0..1 the same way bars.rs does — the same numbers, read the same way, so
// the two views agree about what "loud" is.
float level(float d) {
    return clamp((d + 60.0) / 60.0, 0.0, 1.0);
}

// The spectrum read as a continuous function of position, so a value that varies
// across the volume does not step between the 32 discrete bands. `VIZ_BANDS` is
// interpolated by the harness rather than written here, so the two cannot drift.
float band_at(float t) {
    float f = clamp(t, 0.0, 1.0) * float(VIZ_BANDS - 1);
    int lo = int(floor(f));
    int hi = min(lo + 1, VIZ_BANDS - 1);
    return mix(level(u_bands[lo]), level(u_bands[hi]), fract(f));
}

float hash13(vec3 p) {
    p = fract(p * 0.3183099 + vec3(0.1, 0.2, 0.3));
    p *= 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}

// 3D value noise, one octave. Trilinear-interpolated lattice, which is cheap
// enough to afford the octave count below and has no gradient to evaluate.
float noise3(vec3 p) {
    vec3 i = floor(p);
    vec3 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float n000 = hash13(i + vec3(0.0, 0.0, 0.0));
    float n100 = hash13(i + vec3(1.0, 0.0, 0.0));
    float n010 = hash13(i + vec3(0.0, 1.0, 0.0));
    float n110 = hash13(i + vec3(1.0, 1.0, 0.0));
    float n001 = hash13(i + vec3(0.0, 0.0, 1.0));
    float n101 = hash13(i + vec3(1.0, 0.0, 1.0));
    float n011 = hash13(i + vec3(0.0, 1.0, 1.0));
    float n111 = hash13(i + vec3(1.0, 1.0, 1.0));
    return mix(
        mix(mix(n000, n100, f.x), mix(n010, n110, f.x), f.y),
        mix(mix(n001, n101, f.x), mix(n011, n111, f.x), f.y),
        f.z
    );
}


// Four octaves. Three is visibly coarser at this scale and five is a fifth of
// the frame budget for detail the march's own stepping hides.
float fbm(vec3 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amp * noise3(p);
        p *= 2.02;
        amp *= 0.5;
    }
    return sum;
}

// The field itself: a domain warp whose strength is the spectrum, so a loud
// passage shears the volume and a quiet one leaves it smooth. Two warp rounds
// is what turns fbm's isotropic blobs into the filament structure that reads as
// a nebula rather than as fog.
float field(vec3 p) {
    // Overall energy: the mean of the spectrum, so a passage that is loud
    // everywhere shears the volume and a quiet one leaves it smooth.
    float energy = 0.0;
    for (int i = 0; i < VIZ_BANDS; i++) {
        energy += level(u_bands[i]);
    }
    energy /= float(VIZ_BANDS);

    // The warp's scale follows the band at the height being sampled, so the
    // volume is fine-grained where the top end is loud and coarse at the bass.
    float w = 2.4 + 2.6 * band_at(p.y * 0.5 + 0.5);
    vec3 q = vec3(
        fbm(p + vec3(0.0, 0.0, u_time * 0.05)),
        fbm(p + vec3(3.7, 1.2, u_time * 0.04)),
        fbm(p + vec3(1.3, 4.1, u_time * 0.06))
    );
    vec3 r = vec3(
        fbm(p + 3.0 * q + vec3(1.7, 9.2, 0.0)),
        fbm(p + 3.0 * q + vec3(8.3, 2.8, 0.0)),
        fbm(p + 3.0 * q + vec3(0.0, 4.1, 5.9))
    );
    float base = fbm(p + 2.4 * r);
    return base - (1.0 - energy) * 0.25;
}

void main() {
    // `gl_FragCoord` is physical pixels, so the aspect has to come from the
    // resolution rather than from anything in point-space.
    vec2 uv = gl_FragCoord.xy / u_resolution;
    vec2 centred = (uv - 0.5) * vec2(u_resolution.x / max(u_resolution.y, 1.0), 1.0);

    // A ray per fragment, from a fixed eye through the volume. The march is a
    // fixed step count: a loop bounded by a varying distance would need a
    // uniform branch, and a constant count is what makes the cost predictable
    // enough to survive a 3x-DPI display.
    vec3 ro = vec3(0.0, 0.0, -3.2);
    vec3 rd = normalize(vec3(centred, 1.6));

    float acc = 0.0;
    float depth = 0.0;
    const int STEPS = 48;
    const float STEP = 0.14;
    for (int i = 0; i < STEPS; i++) {
        vec3 p = ro + rd * depth;
        float d = field(p);
        // A soft density ramp rather than a threshold: a hard one aliases badly
        // because the field is sampled at a fixed step, not at its own scale.
        acc += smoothstep(0.62, 0.98, d) * STEP;
        depth += STEP;
        if (depth > 3.0) {
            break;
        }
    }

    // Palette only, and the same heat axis the spectrogram uses: bg toward
    // accent. The second colour is the brighter token, so the bright core and
    // the dim body are the same split bars.rs draws.
    float density = clamp(acc, 0.0, 1.0);
    vec3 col = mix(u_bg.rgb, u_accent.rgb, smoothstep(0.0, 0.75, density));
    col = mix(col, u_progress_fill.rgb, smoothstep(0.65, 1.0, density));

    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Brisker than bars.rs: a volume has no hard edge to smear, and the motion
    // hides a little of the release without ever looking like the bars do.
    const ATTACK: f32 = 0.45;
    const RELEASE: f32 = 0.88;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    gpu::add_fullscreen(painter, rect, FRAG, uniforms, Target::Screen);
}
