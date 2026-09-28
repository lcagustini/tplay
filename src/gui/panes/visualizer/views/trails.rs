//! Trails view — a feedback accumulation, which is the one effect in this set
//! that egui cannot express at all.
//!
//! Everything else here is a function of the current frame's inputs. This one
//! samples **its own previous output**: each frame draws a small amount of new
//! energy and keeps `feedback_for_a_dt(dt, HOLD_SECS)` of what was already there,
//! through a slow zoom and rotation. Nothing in the pane can express that — egui
//! hands a view a `Painter` for one frame, with no render target a view may keep
//! reading from — so it is the one case where the shader views earn the extra
//! machinery.
//!
//! **The decay is time-based, not per-frame.** See [`gpu::feedback_for_a_dt`],
//! and the tests beside it. A per-frame multiplier would make the trail twice as
//! long at 30 Hz as at 60 Hz.
//!
//! **Two passes, one callback.** The accumulate pass renders into an owned RGBA8
//! target sampling the other; the present pass draws that onto the pane. They
//! cannot be two `Shape::Callback`s, because egui restores GL state between
//! primitives and would draw the second pass over whatever landed in between.

use crate::audio::viz::{compute_bands, VizBuf, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Feedback};
use crate::gui::theme::Palette;
use eframe::egui;
use std::sync::LazyLock;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.trails")
}

/// The ping-pong pair and its parity, in egui memory so the two targets outlive
/// the frame that asked for them.
fn feedback_id() -> egui::Id {
    egui::Id::new("tplay.viz.trails.feedback")
}

/// The trail's visible length, in seconds. This is the number the whole decay is
/// expressed in: after this long the oldest frame is down to `1/e` of itself, at
/// any frame rate.
///
/// Deliberately unpinned, like every view's smoothing constants — a test over a
/// duration constant can only assert the value, and the property that matters
/// (frame-rate independence) is pinned on `feedback_for_a_dt` directly.
const HOLD_SECS: f32 = 1.6;

/// The per-frame zoom, as a fraction. Kept small and **constant** rather than
/// tied to `dt`: a scale that varied with the frame rate would make the trail's
/// *shape* frame-rate dependent too, and unlike its length that is not something
/// a user would read as "the same picture, slower".
const ZOOM: f32 = 0.0035;
/// The per-frame rotation, radians. Slow enough to read as a drift.
const SPIN: f32 = 0.004;

/// Pass one: the new frame, mixed with a transformed copy of the previous one.
///
/// The two constants are substituted in by [`accumulate_shader`] rather than
/// written into the GLSL, so the numbers this file reasons about and the ones the
/// shader uses cannot drift. The substitution is textual and runs **once**, into a
/// `LazyLock`, because the program cache is keyed by the assembled source and a
/// per-frame `String` would both defeat it and allocate.
pub const ACCUMULATE_BODY: &str = r#"
void main() {
    vec2 uv = gl_FragCoord.xy / u_resolution;
    vec2 centred = uv - 0.5;

    float c = cos(SPIN);
    float s = sin(SPIN);
    vec2 spun = vec2(centred.x * c - centred.y * s, centred.x * s + centred.y * c);
    // The inverse transform, sampled: a point in the *new* frame asks where it
    // was in the old one. A positive ZOOM shrinks the trail inward, so the image
    // appears to expand — which is the direction a zoom reads correctly.
    vec2 prev = (spun * (1.0 - ZOOM) - centred) + 0.5;

    // Outside the old frame there is nothing to keep. Zeroed rather than
    // discarded so the edge does not sample a clamp-to-edge texel and smear a
    // bright border inward every frame.
    float inside = step(0.0, prev.x) * step(prev.x, 1.0)
                 * step(0.0, prev.y) * step(prev.y, 1.0);
    vec3 old = texture(u_prev, clamp(prev, 0.0, 1.0)).rgb * inside * u_feedback;

    // The new frame's shape: a ring of energy whose brightness and thickness
    // follow the spectrum, so this is still a music visualisation and not a
    // screensaver.
    float energy = 0.0;
    for (int i = 0; i < VIZ_BANDS; i++) {
        energy += clamp((u_bands[i] + 60.0) / 60.0, 0.0, 1.0);
    }
    energy /= float(VIZ_BANDS);

    // The band at this pixel's angle, so the ring varies with the spectrum's
    // *shape* rather than pulsing on its level alone.
    float f = clamp(atan(centred.y, centred.x) / 6.2831853 + 0.5, 0.0, 1.0)
            * float(VIZ_BANDS - 1);
    int lo = int(floor(f));
    int hi = min(lo + 1, VIZ_BANDS - 1);
    float band = mix(
        clamp((u_bands[lo] + 60.0) / 60.0, 0.0, 1.0),
        clamp((u_bands[hi] + 60.0) / 60.0, 0.0, 1.0),
        fract(f)
    );

    float r = length(centred);
    float ring = exp(-pow((r - 0.34) * 7.0, 2.0));
    vec3 fresh = mix(u_bg.rgb, u_accent.rgb, band) * ring * (0.25 + 1.5 * energy);

    frag_color = vec4(old + fresh, 1.0);
}
"#;

/// The assembled accumulate pass, with the two constants in.
static ACCUMULATE: LazyLock<String> = LazyLock::new(|| {
    ACCUMULATE_BODY
        .replace("SPIN", &format!("{SPIN:.6}"))
        .replace("ZOOM", &format!("{ZOOM:.6}"))
});

/// Pass two: the accumulated target, presented.
///
/// A straight read-back with a gentle tone curve. Without it the accumulation's
/// repeated `+` drives every channel to 1 within a few seconds and the trails
/// stop being trails — which is the one failure mode a feedback view has that no
/// other view in this set can have.
pub const PRESENT: &str = r#"
void main() {
    vec3 c = texture(u_prev, gl_FragCoord.xy / u_resolution).rgb;
    // Reinhard on the accumulated value: keeps a long trail inside the palette's
    // range instead of clipping to white.
    c = c / (c + 1.0);
    frag_color = vec4(mix(u_bg.rgb, c, 0.92), 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // Snappy in both directions: an accumulation smears whatever it is given, so
    // a slow release would read as the trail lagging the music by a visible margin.
    const ATTACK: f32 = 0.5;
    const RELEASE: f32 = 0.9;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(prev_id(), prev));

    let mut feedback: Feedback = painter
        .ctx()
        .memory_mut(|m| m.data.get_temp(feedback_id()).unwrap_or_default());

    // A pane that has just been resized, or a view that has just been switched
    // to, must not present the *other* one's trail. A size change is caught
    // inside `draw` (the targets no longer match the pane), but a view switch
    // cannot be: the targets are the right size and hold the wrong picture, and
    // nothing but this call knows the view stopped being the selected one. So it
    // is stated here, once, where the reason is.
    if painter
        .ctx()
        .memory(|m| m.data.get_temp::<bool>(was_selected_id()))
        != Some(true)
    {
        feedback.invalidate();
    }
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(was_selected_id(), true));

    let mut uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    uniforms.feedback = gpu::feedback_for_a_dt(uniforms.dt, HOLD_SECS);
    feedback.draw(painter, rect, ACCUMULATE.as_str(), PRESENT, uniforms);
    painter
        .ctx()
        .memory_mut(|m| m.data.insert_temp(feedback_id(), feedback));
}

/// Whether [`draw`] ran on the previous frame.
///
/// A view switch is a gap in a view's own `draw` calls, and that gap is the only
/// signal any view gets — nothing tells a pane "you are no longer selected". One
/// boolean in egui memory is the whole detection.
fn was_selected_id() -> egui::Id {
    egui::Id::new("tplay.viz.trails.selected")
}
