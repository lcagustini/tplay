//! VU meter view — loudness, not spectrum: an RMS bar with a peak-hold marker
//! on a dB scale.
//!
//! The only view here driven by `compute_level` rather than `compute_bands`.
//! Loudness is one of the two properties a music visualization is defined to
//! follow, and the rest of this set reads only the spectrum — so without this
//! the pane cannot answer "how loud is it".
//!
//! The scale is dB, not linear amplitude, because a linear meter pins almost all
//! real music to the left tenth of its travel.

use crate::audio::viz::{compute_level, VizBuf};
use crate::gui::theme::Palette;
use eframe::egui;

/// A peak holds this long before it begins to fall, so a transient is not
/// invisible between frames.
const HOLD_SECS: f64 = 1.0;
/// How fast the held peak falls once the hold expires, in scale units/second.
const FALL_RATE: f32 = 0.5;
/// Meter height as a fraction of the pane's short axis.
const TRACK_FRAC: f32 = 0.22;

fn state_id() -> egui::Id {
    egui::Id::new("tplay.viz.vu.hold")
}

/// Amplitude 0..1 → 0..1 on a −60…0 dB scale.
///
/// The clamp is load-bearing, not tidiness: silence floors to 1e-6 to dodge a
/// `log10(0)`, which is −120 dB — another 60 dB below the scale's own floor, so
/// the raw mapping returns **−1.0** there and only the caller's `> 0.0` checks
/// were keeping a negative position off the rail.
fn db_scale(v: f32) -> f32 {
    let db = 20.0 * v.max(1e-6).log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let (rms, peak) = compute_level(viz);
    let ctx = painter.ctx().clone();
    let now = ctx.input(|i| i.time);
    let dt = ctx.input(|i| i.predicted_dt);

    // Peak-hold ballistics: rise instantly, hold, then fall. The state has to
    // live somewhere across frames, and egui memory is where every other view
    // keeps what it needs.
    let (mut held, mut held_at) = ctx.memory_mut(|m| {
        m.data
            .get_temp::<(f32, f64)>(state_id())
            .unwrap_or((0.0, 0.0))
    });
    let target = db_scale(peak);
    if target >= held {
        held = target;
        held_at = now;
    } else if now - held_at > HOLD_SECS {
        held = (held - FALL_RATE * dt).max(target);
    }
    ctx.memory_mut(|m| m.data.insert_temp(state_id(), (held, held_at)));

    // Lay the meter along the pane's longer axis, so it is a usable size whether
    // the pane is docked wide-and-short or tall-and-narrow. Everything below is
    // then addressed in `(t, w)` — `t` along the meter, `w` across it — and one
    // mapper turns that into pixels. Doing it this way is what keeps every
    // `Rect::from_min_max` below correctly ordered in both orientations.
    let horizontal = rect.width() >= rect.height();
    let track = rect.width().min(rect.height()) * TRACK_FRAC;
    let pt = |t: f32, w: f32| {
        if horizontal {
            egui::pos2(
                rect.left() + t * rect.width(),
                rect.center().y - track * 0.5 + w * track,
            )
        } else {
            egui::pos2(
                rect.center().x - track * 0.5 + w * track,
                rect.bottom() - t * rect.height(),
            )
        }
    };

    // Rail, in the slider-track token so it matches the EQ pane's slider.
    painter.rect_filled(
        egui::Rect::from_min_max(pt(0.0, 0.0), pt(0.0, 1.0)),
        0.0,
        palette.slider_track,
    );

    // dB graticule. Fixed at −40/−20/−6 rather than derived: a scale is a fixed
    // reference, and deriving these from the pane would make the markings move
    // when the splitter is dragged.
    for db in [-40.0f32, -20.0, -6.0] {
        let t = (db + 60.0) / 60.0;
        painter.line_segment(
            [pt(t, 0.0), pt(t, 0.25)],
            egui::Stroke::new(0.5_f32, palette.border),
        );
    }

    // RMS body, full track height. At silence `db_scale` is 0.0, which makes this
    // a zero-width rect — a no-op, so it needs no guard.
    painter.rect_filled(
        egui::Rect::from_min_max(pt(0.0, 0.0), pt(db_scale(rms), 1.0)),
        0.0,
        palette.accent,
    );

    // Peak-hold marker, overshooting the track like a real needle, in the
    // brighter token — the same body/edge split bars.rs makes.
    painter.line_segment(
        [pt(held, -0.15), pt(held, 1.15)],
        egui::Stroke::new(2.0_f32, palette.progress_fill),
    );
}
