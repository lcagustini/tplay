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

use super::frame_dt;
use crate::audio::viz::{compute_bands, VizBuf, DB_FLOOR, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Feedback};
use crate::gui::theme::Palette;
use eframe::egui;

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

/// Pass one: the new frame, mixed with a transformed copy of the previous one.
///
/// The spin and the zoom are written into the WGSL as literals rather than
/// substituted from Rust constants, and that is a deliberate reversal. The first
/// version kept them as `const`s and textually replaced them into a `LazyLock`
/// string, so the pipeline cache keyed on the *substituted* source while
/// `SHADER_VIEWS` listed the *unsubstituted* one — the test table named a shader
/// the app never ran, which is exactly the drift the table exists to prevent, and
/// the substitution is what caused it. Two numbers in a shader body, each with
/// its value and its reason in a comment beside it, is a smaller risk than the
/// machinery that was guarding them.
pub const HELPERS: &str = "";

pub const FRAG: &str = r#"
    let uv = v_uv;

    // **Aspect-corrected, and this view accumulates the error when it is not.**
    // Three separate things are wrong in a non-square pane, and a feedback view
    // is the worst place for all three because the trail *keeps* them: the ring
    // draws as an ellipse rather than a circle, the spin below becomes a shear
    // instead of a rotation, and the zoom shrinks by a different fraction in
    // pixels on each axis. So the picture is not merely a squashed version of
    // itself — it is a different transform each frame, folded into the buffer.
    // The same correction radial.rs and chladni.rs make.
    let aspect = u_resolution().x / max(u_resolution().y, 1.0);
    let centred = (uv - 0.5) * vec2<f32>(aspect, 1.0);

    // SPIN: the per-frame rotation, radians. Slow enough to read as a drift, and
    // constant rather than tied to `dt` — a scale that varied with the frame rate
    // would make the trail's *shape* frame-rate dependent too, and unlike its
    // length that is not something a user reads as "the same picture, slower".
    let c = cos(0.004);
    let s = sin(0.004);
    let spun = vec2<f32>(centred.x * c - centred.y * s, centred.x * s + centred.y * c);
    // The inverse transform, sampled: a point in the *new* frame asks where it
    // was in the old one. A positive ZOOM shrinks the trail inward, so the image
    // appears to expand — which is the direction a zoom reads correctly.
    // ZOOM: the per-frame shrink of the sampled copy, 0.35% a frame.
    //
    // **Un-corrected on the way out**, because `prev` is a coordinate into
    // `u_prev`, which is in the target's own 0..1 texture space — not in the
    // aspect-corrected space the transform was computed in. Dividing by `aspect`
    // is what makes it the same point rather than a mirrored one; sampling the
    // corrected value directly would shear the whole trail across the pane.
    let prev = ((spun * (1.0 - 0.0035) - centred) / vec2<f32>(aspect, 1.0)) + 0.5;

    // Outside the old frame there is nothing to keep — but **a hard cut there is
    // a visible vertical seam down the pane**: the trail stopped dead at one
    // column, which reads as a clipping artefact rather than as the trail leaving
    // the frame. Fading over a few percent of the width makes the boundary a
    // gradient instead. The fade is also why the sample is clamped rather than
    // discarded: a clamp-to-edge texel at zero contribution is free, whereas
    // sampling outside the target is undefined.
    //
    // **The fade is keyed on `uv`, the fragment's own position — not on `prev`,
    // where it was.** The trail is a shrink toward the centre, so the sample
    // coordinate is nowhere near the screen coordinate at the pane's edges: at
    // `centred.x = -0.5`, `prev.x` is about `0.502`, the middle of the pane. So a
    // fade on `prev` put a ramp in the middle of the picture, left the actual left
    // edge at full weight, and the `clamp` then held that edge texel at full
    // strength for the whole fade — which is the hard line that was left. The
    // thing being faded is the *contribution*, and the contribution is a function
    // of where the fragment is, so `uv` is the coordinate it belongs to. This is
    // the same "which space is this coordinate in" trap as the waterfall's `y`
    // flip, and the two views are the only two that reach for a coordinate other
    // than the fragment's own.
    const EDGE : f32 = 0.07;
    let fade = smoothstep(vec2<f32>(0.0), vec2<f32>(EDGE), uv)
             * (vec2<f32>(1.0) - smoothstep(vec2<f32>(1.0 - EDGE), vec2<f32>(1.0), uv));
    // **`textureSampleLevel`, not `textureSample`.** The target is a
    // single-mip RGBA8 texture and this is a plain read of it, and
    // `textureSample` is illegal in non-uniform control flow anyway — the
    // explicit level is the spelling that is correct in both readings.
    // **The clamp's bounds are spelled `vec2`, and that is WGSL's type rule
    // rather than a style choice:** `clamp` needs all three arguments to be the
    // same type, and a bare `0.0` is an abstract float the compiler is not
    // allowed to materialise into a `vec2<f32>` at this position.
    let old = textureSampleLevel(u_prev, u_samp,
            clamp(prev, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0)), 0.0).rgb
            * fade.x * fade.y * u_feedback();

    // The new frame's shape: a ring of energy whose brightness and thickness
    // follow the spectrum, so this is still a music visualisation and not a
    // screensaver.
    var energy = 0.0;
    for (var i = 0u; i < VIZ_BANDS; i = i + 1u) {
        energy = energy + level(band_at(i));
    }
    energy = energy / f32(VIZ_BANDS);

    // The band at this pixel's angle, so the ring varies with the spectrum's
    // *shape* rather than pulsing on its level alone.
    //
    // The harness's projection, not an `atan` indexed into the band array: `atan`
    // branches along `centred.x < 0` at `centred.y == 0`, which is the pane's
    // left edge, and `fresh` is *added* to the buffer every frame — so the step
    // was deposited as a permanent vertical line rather than merely drawn once.
    // See `gpu::BEARING`.
    let band = spectrum_at_bearing(centred);

    let r = length(centred);
    let ring = exp(-pow((r - 0.34) * 7.0, 2.0));
    // **Emission, in the accent token, never tinted toward the background.** The
    // buffer is cleared to black and this is added to it, so `fresh` is light
    // being deposited. Multiplying by `mix(u_bg.rgb, u_accent.rgb, band)` — which
    // is what this did — means that with a flat spectrum, where every band
    // normalises to 0 and the mix returns the *background* colour, the view
    // deposits background-coloured light and is therefore invisible against the
    // background it is drawn on. The spectrum drives how *bright* the ring is,
    // not what colour it is: a silent passage still shows a dim ring, which is
    // the idle state every other view here has.
    let fresh = u_accent().rgb * ring * (0.22 + 1.7 * band) * (0.45 + 0.9 * energy);

    frag_color = vec4<f32>(old + fresh, 1.0);
"#;

/// Pass two: the accumulated target, presented.
///
/// A straight read-back with a gentle tone curve. Without it the accumulation's
/// repeated `+` drives every channel to 1 within a few seconds and the trails
/// stop being trails — which is the one failure mode a feedback view has that no
/// other view in this set can have.
pub const PRESENT: &str = r#"
    var c = textureSampleLevel(u_prev, u_samp, v_uv, 0.0).rgb;
    // Reinhard on the accumulated value: keeps a long trail inside the palette's
    // range instead of clipping to white.
    c = c / (c + 1.0);
    frag_color = vec4<f32>(mix(u_bg().rgb, c, 0.92), 1.0);
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([DB_FLOOR; VIZ_BANDS])
    });

    // Snappy in both directions: an accumulation smears whatever it is given, so
    // a slow release would read as the trail lagging the music by a visible margin.
    const ATTACK_MS: f32 = 24.1;
    const RELEASE_MS: f32 = 7.2;
    compute_bands(viz, &mut prev, frame_dt(painter), ATTACK_MS, RELEASE_MS);
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
    feedback.draw(
        painter,
        rect,
        HELPERS,
        FRAG,
        PRESENT,
        uniforms,
        viz.take_arrival(),
    );
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
