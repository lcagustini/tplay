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

use super::frame_dt;
use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu;
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.nebula")
}

/// This view's own functions. See `ShaderView::helpers` for why they are a
/// separate slot from the body.
pub const HELPERS: &str = r#"
// `band_at` is the array in dB over -60..0, and this view draws loudness, so it
// maps that onto 0..1 the same way bars.rs does — the same numbers, read the
// same way, so the two views agree about what "loud" is. This is an interpolation
// over the *array*, and `VIZ_BANDS` is interpolated by the harness rather than
// written here, so the two cannot drift.
fn spectrum_at(t : f32) -> f32 {
    let f = clamp(t, 0.0, 1.0) * f32(VIZ_BANDS - 1u);
    let lo = i32(floor(f));
    let hi = min(lo + 1, i32(VIZ_BANDS) - 1);
    return mix(level(band_at(u32(lo))), level(band_at(u32(hi))), fract(f));
}

// A local, not a parameter. WGSL function parameters are read-only too, and
// `ptr` is not an option either because every call site passes an expression like
// `i + vec3<f32>(1.0, 0.0, 0.0)`.
fn hash13(v : vec3<f32>) -> f32 {
    var p = fract(v * 0.3183099 + vec3<f32>(0.1, 0.2, 0.3));
    p = p * 17.0;
    return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}

// 3D value noise, one octave. Trilinear-interpolated lattice, which is cheap
// enough to afford the octave count below and has no gradient to evaluate.
fn vnoise3(p : vec3<f32>) -> f32 {
    let i = floor(p);
    var f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    let n000 = hash13(i + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = hash13(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3<f32>(1.0, 1.0, 1.0));
    return mix(
        mix(mix(n000, n100, f.x), mix(n010, n110, f.x), f.y),
        mix(mix(n001, n101, f.x), mix(n011, n111, f.x), f.y),
        f.z
    );
}

// Three octaves. See the budget in the body — this is the multiplier, and it is
// the cheapest thing to give up when the picture is too expensive.
//
// **The division is load-bearing, and it is why this view rendered nothing at
// all.** The amplitudes sum to 0.875 at three octaves, so the raw sum is 0..0.875
// with a mean near 0.44 — while the density ramp below was `smoothstep(0.62,
// 0.98, …)`. A field whose mean is 0.44 contributes nothing to a ramp that starts
// above 0.62, so `acc` stayed at zero and the pane was the background colour: a
// working program, a correct picture, and a silent failure. Normalising puts the
// field in 0..1 around a mean of 0.5, so the ramp can be written against a number
// that means something.
fn fbm(p : vec3<f32>) -> f32 {
    var q = p;
    var sum = 0.0;
    var amp = 0.5;
    for (var i = 0; i < 3; i = i + 1) {
        sum = sum + amp * vnoise3(q);
        q = q * 2.02;
        amp = amp * 0.5;
    }
    return sum / 0.875;   // 0.5 + 0.25 + 0.125, the amplitudes above
}

// The field itself: one fbm, at a frequency the spectrum sets.
//
// **No domain warp, and that is the whole point of this function.** The first
// version had two warp rounds, which is seven `fbm` calls per sample — a domain
// warp is quadratic in fbm calls — evaluated inside a 48-step march. That is
// 10752 hash13 per fragment, ~7.7 billion a frame at 720k fragments, and the
// symptom was not a slow view: it was the whole window flashing, because frames
// took longer than a compositor swap. The budget is written out in the body where
// the step count lives; this is the number that has to respect it.
// The field, in 0..1 with a mean near 0.5. `energy` is deliberately **not** a
// bias on the field: subtracting a constant to "dim" it pushes the whole volume
// down the density ramp, so silence lands *below the ramp entirely* and the view
// goes black. Loudness belongs on the contribution, not on the field.
fn field(p : vec3<f32>) -> f32 {
    // The volume's scale follows the band at this sample's height, so it is
    // fine-grained where the top end is loud and coarse at the bass. Two array
    // reads — a warp would have cost seven fbm evaluations to do the same job.
    let w = 2.4 + 2.6 * spectrum_at(p.y * 0.5 + 0.5);
    return fbm(p * w);
}
"#;

pub const FRAG: &str = r#"
    // The budget, written as the arithmetic rather than picked: 24 steps x 1 fbm x
    // 3 octaves x 8 hash13 = 576 hash13 per fragment, which at 720k fragments (a
    // 600x300 pane on a 2x display) is a few milliseconds. Three octaves rather
    // than four because the fourth is invisible at this scale and costs a third
    // more; the march's own stepping hides the rest.
    // `no_shader_marches_further_than_it_can_afford` reads both numbers.
    const STEPS : i32 = 24;
    const STEP : f32 = 0.14;

    // `v_uv` is 0..1 over the pane regardless of its size, so the aspect ratio
    // has to come from the resolution rather than from anything in point-space.
    let uv = v_uv;
    let centred = (uv - 0.5) * vec2<f32>(u_resolution().x / max(u_resolution().y, 1.0), 1.0);

    // The spectrum's mean, **once per fragment**. It is the same number at
    // every step of the march, so computing it inside the loop was 48 redundant
    // passes over the band array per pixel.
    var energy = 0.0;
    for (var i = 0u; i < VIZ_BANDS; i = i + 1u) {
        energy = energy + level(band_at(i));
    }
    energy = energy / f32(VIZ_BANDS);

    // A ray per fragment, from a fixed eye through the volume.
    let ro = vec3<f32>(0.0, 0.0, -3.2);
    let rd = normalize(vec3<f32>(centred, 1.6));

    // Loudness scales the contribution, not the field — see `field`. The floor
    // is what makes silence a dim cloud rather than the background colour.
    let gain = 0.35 + 0.9 * energy;

    var acc = 0.0;
    var depth = 0.0;
    for (var i = 0; i < STEPS; i = i + 1) {
        let d = field(ro + rd * depth);
        // A soft density ramp rather than a threshold: a hard one aliases badly
        // because the field is sampled at a fixed step, not at its own scale.
        // **The lower edge is below the field's mean**, which is the property
        // that makes the volume visible at all: a ramp starting above 0.5
        // discards more than half of every sample and the picture goes flat.
        acc = acc + smoothstep(0.40, 0.72, d) * STEP * gain;
        depth = depth + STEP;
        // Two exits, and the second is the expensive one: a ray that has left
        // the volume, or that has already accumulated more density than the
        // 0..1 ramp can show, can never contribute again. Without the second,
        // every ray pays for the full depth regardless of what it found.
        if (depth > 3.0 || acc > 1.0) {
            break;
        }
    }

    // Palette only, and the same heat axis the spectrogram uses: bg toward
    // accent. The second colour is the brighter token, so the bright core and
    // the dim body are the same split bars.rs draws.
    let density = clamp(acc, 0.0, 1.0);
    var col = mix(u_bg().rgb, u_accent().rgb, smoothstep(0.0, 0.75, density));
    col = mix(col, u_progress_fill().rgb, smoothstep(0.65, 1.0, density));

    frag_color = vec4<f32>(col, 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Brisker than bars.rs: a volume has no hard edge to smear, and the motion
    // hides a little of the release without ever looking like the bars do.
    const ATTACK_MS: f32 = 27.9;
    const RELEASE_MS: f32 = 7.9;
    compute_bands(viz, &mut prev, frame_dt(painter), ATTACK_MS, RELEASE_MS);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    gpu::add_fullscreen(painter, rect, HELPERS, FRAG, uniforms);
}
