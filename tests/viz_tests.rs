//! Visualizer DSP — ring buffer, FFT, and the two band-mapping functions.
//!
//! Lives here rather than in a `#[cfg(test)] mod tests` inside
//! `src/audio/viz.rs` because the project keeps every test in `tests/` (each
//! file is its own auto-discovered binary, run exactly once). It needs an audio
//! device for nothing: the tap source is covered at Source level in
//! `eq_live.rs`-style suites, and everything here is pure.

use std::f32::consts::PI;
use tplay::audio::viz::{
    compute_bands, compute_wave, fft_magnitude, hold_tick, pick_mode, VizBuf, FFT_SIZE, HOLD_SECS,
    MARGIN_DB, MAX_MODE, VIZ_BANDS,
};

/// A band array with the given `(index, dB)` entries and silence everywhere
/// else, which is what the floored bottom of the range looks like.
fn bands_with(peaks: &[(usize, f32)]) -> [f32; VIZ_BANDS] {
    let mut bands = [-60.0; VIZ_BANDS];
    for &(i, db) in peaks {
        bands[i] = db;
    }
    bands
}

#[test]
fn viz_buf_push_and_snapshot() {
    let buf = VizBuf::new();
    buf.push(0.5);
    buf.push(-0.25);
    let tail = buf.snapshot_tail(10);
    assert_eq!(tail, vec![0.5, -0.25]);
}

#[test]
fn viz_buf_clear() {
    let buf = VizBuf::new();
    buf.push(1.0);
    buf.clear();
    assert!(buf.snapshot_tail(10).is_empty());
}

/// A source at a chosen rate, so the tap can report one that is not the default.
struct RateSource {
    rate: u32,
    left: usize,
}

impl Iterator for RateSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        Some(0.0)
    }
}

impl tplay::rodio::Source for RateSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
    fn try_seek(&mut self, _: std::time::Duration) -> Result<(), tplay::rodio::source::SeekError> {
        Err(tplay::rodio::source::SeekError::NotSupported {
            underlying_source: "rate probe",
        })
    }
}

/// The band→bin mapping needs the source's real rate, and a source is the only
/// thing that knows it — `compute_bands` used to assume 44.1 kHz outright, so a
/// 48 kHz file had every band edge ~9% off.
#[test]
fn the_tap_reports_the_sources_sample_rate() {
    use tplay::audio::viz::TapSource;

    let buf = VizBuf::new();
    assert_eq!(
        buf.sample_rate(),
        44100,
        "nothing playing: the default stands"
    );

    let mut tap = TapSource::new(
        RateSource {
            rate: 48000,
            left: 4,
        },
        buf.clone(),
    );
    assert_eq!(buf.sample_rate(), 48000);
    while tap.next().is_some() {}

    // Explicitly: a non-44.1k rate survives, and a 96k one is not clamped.
    TapSource::new(
        RateSource {
            rate: 96000,
            left: 1,
        },
        buf.clone(),
    );
    assert_eq!(buf.sample_rate(), 96000);

    // `Default` must not hand out a zero rate, which would divide the bins to inf.
    assert_eq!(VizBuf::default().sample_rate(), 44100);
}

#[test]
fn fft_constant_dc() {
    let mut input = [0.0f32; FFT_SIZE * 2];
    for i in 0..FFT_SIZE {
        input[i * 2] = 1.0; // real = 1, imag = 0
    }
    let mag = fft_magnitude(&mut input);
    // DC bin (index 0) should be FFT_SIZE, others ~0
    assert!((mag[0] - FFT_SIZE as f32).abs() < 1.0);
    for m in &mag[1..10] {
        assert!(*m < 1.0);
    }
}

#[test]
fn fft_sine_1khz() {
    // 1 kHz sine at 44.1 kHz, 1024 samples → bin ≈ 1000 * 1024 / 44100 ≈ 23
    let mut input = [0.0f32; FFT_SIZE * 2];
    for i in 0..FFT_SIZE {
        input[i * 2] = (2.0 * PI * 1000.0 * i as f32 / 44100.0).sin();
    }
    let mag = fft_magnitude(&mut input);
    let peak_bin = mag
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap()
        .0;
    assert!(
        (peak_bin as i32 - 23).abs() <= 1,
        "peak at bin {}, expected ~23",
        peak_bin
    );
}

#[test]
fn compute_bands_decays_when_empty() {
    let buf = VizBuf::new();
    let mut prev = [0.0f32; VIZ_BANDS];
    compute_bands(&buf, &mut prev, 0.3, 0.95);
    for v in prev {
        assert!(v < 0.0); // decayed from 0 toward -60
    }
}

#[test]
fn compute_wave_empty() {
    let buf = VizBuf::new();
    let wave = compute_wave(&buf, VIZ_BANDS);
    assert_eq!(wave.len(), VIZ_BANDS);
    assert!(wave.iter().all(|&v| v == 0.0));
}

#[test]
fn compute_wave_peak_envelope() {
    let buf = VizBuf::new();
    for &v in &[0.2, -0.8, 0.1, -0.3] {
        buf.push(v);
    }
    // 4 samples, 2 buckets: bucket0 = max(|0.2|, |-0.8|) = 0.8, bucket1 = 0.3
    let wave = compute_wave(&buf, 2);
    assert_eq!(wave.len(), 2);
    assert!((wave[0] - 0.8).abs() < 1e-6);
    assert!((wave[1] - 0.3).abs() < 1e-6);
}

#[test]
fn compute_wave_reaches_last_bucket() {
    // 1024 samples, 350 buckets (a ~700px pane): div_ceil grouping used to
    // leave the rightmost buckets empty, collapsing the wave before the
    // pane edge. Every bucket must now get its share and fill to 1.0.
    let buf = VizBuf::new();
    for _ in 0..1024 {
        buf.push(1.0);
    }
    let wave = compute_wave(&buf, 350);
    assert_eq!(wave.len(), 350);
    assert_eq!(wave[349], 1.0);
    assert!(wave.iter().all(|&v| v == 1.0));
}

// --- the hold --------------------------------------------------------------
//
// The Chladni figure's anti-strobe mechanism has two parts: a margin on how much
// louder a challenger has to be, and a *duration* for which the incumbent is
// pinned. The duration was a frame count, which is a duration only at one frame
// rate, and on the 120 Hz display this was reported on it came out at half its
// intended length — so the figure changed at 1.3 Hz against a hold written to
// mean 0.67 Hz, and the symptom was "the visualizer is running at 2 fps" when the
// app was in fact at 120.

/// The same wall-clock time is the same hold, at any frame rate.
///
/// The property, stated as a simulation rather than a comparison, because a
/// comparison can only say "these two numbers match" — the thing that has to be
/// true is that ninety frames at 60 Hz and at 120 Hz spend the same *seconds*.
#[test]
fn the_hold_is_a_duration_not_a_frame_count() {
    /// Seconds a full hold lasts, spent one frame at a time.
    fn seconds_to_spend(rate: f32) -> f32 {
        let mut hold = HOLD_SECS;
        let mut elapsed = 0.0f32;
        // A minute is far longer than any hold, and a frame count at any rate
        // the pane has ever run at empties well inside it.
        while hold > 0.0 && elapsed < 60.0 {
            hold = hold_tick(hold, 1.0 / rate);
            elapsed += 1.0 / rate;
        }
        elapsed
    }
    let at_30 = seconds_to_spend(30.0);
    let at_60 = seconds_to_spend(60.0);
    let at_120 = seconds_to_spend(120.0);
    let at_144 = seconds_to_spend(144.0);
    // One frame of slack, so this is about the shape and not about rounding.
    let slack = 1.0 / 20.0;
    for (rate, spent) in [
        (30.0, at_30),
        (60.0, at_60),
        (120.0, at_120),
        (144.0, at_144),
    ] {
        assert!(
            (spent - HOLD_SECS).abs() < slack,
            "a {HOLD_SECS}s hold spent at {rate} fps took {spent:.3}s — the hold is \
             being counted in frames, so it is twice as short on a 120 Hz display as \
             it is on a 60 Hz one"
        );
    }
}

/// A degenerate frame time cannot release a hold early.
#[test]
fn a_degenerate_frame_time_does_not_release_the_hold() {
    for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let held = hold_tick(1.0, dt);
        // `INFINITY` is the sharp end, and the reason this is not a one-liner:
        // `dt.max(0.0)` passes it through, and `1.0 - inf` clamps to zero — so
        // the guard itself is what released the figure, rather than what stopped
        // it.
        assert!(
            held >= 1.0,
            "dt {dt} must not shorten a hold, got {held} — releasing the figure early \
             is the strobe this exists to prevent, and egui's predicted frame time is \
             legitimately zero on a session's first frame"
        );
    }
    // And it does run out, which is the other half: a hold that never expires is
    // a figure that never changes again.
    assert_eq!(hold_tick(0.01, 0.5), 0.0);
}

// --- the mode pick ---------------------------------------------------------
//
// The view's figure is a *discrete* function of the two loudest bands, so every
// change of that pair is a redraw of the whole plate. A bare top-two picked a
// new pair 60 times in 15 s over a drifting bass line. These are the guard
// against that coming back.
//
// The *field* these mode numbers are drawn with is no longer here. It was
// `chladni_field`, and the four tests that pinned it went with the function when
// the view became a fragment shader: the nodal set is now read per fragment
// rather than searched across a lattice, so a cell cannot straddle a nodal line
// and the properties those tests were defending cannot be violated by the
// drawing. `pick_mode` stayed, because a *discrete pick* is a different problem
// from a field evaluation and the hysteresis is all that is between a figure
// that tracks the music and one that strobes.

/// The regression itself: two pairs within a hair of each other, and the figure
/// must not move. A challenger has to be `MARGIN_DB` louder on its *weaker*
/// band, not on its louder one — a pair is only as loud as its quieter member.
#[test]
fn a_near_tie_keeps_the_current_figure() {
    // The incumbent (1, 2) is weaker at -20; the challenger is (1, 3) at -19.
    // 1 dB is well inside the margin.
    let bands = bands_with(&[(1, -18.0), (2, -20.0), (3, -19.0)]);
    assert_eq!(
        pick_mode(&bands, Some((1, 2)), 0.0),
        (1, 2),
        "a 1 dB lead must not repaint the plate"
    );
}

/// The other half: a real change of the music's character still moves the
/// figure, or the fix would just be a figure that never changes.
#[test]
fn a_clear_winner_does_move_the_figure() {
    let bands = bands_with(&[(1, -50.0), (2, -50.0), (3, -10.0), (4, -12.0)]);
    assert_eq!(pick_mode(&bands, Some((1, 2)), 0.0), (3, 4));
}

/// The margin is measured against the weaker band, which is the half that
/// catches a failure a louder-band comparison would miss. Both halves, because
/// the assertion is only about the band the comparison reads, and both built
/// from `MARGIN_DB` so the test tracks the constant.
#[test]
fn the_margin_is_measured_on_the_weaker_band() {
    let incumbent = -25.0;
    // (1, 2) beats the incumbent (2, 3) by 25 dB on its *loudest* band and not
    // at all on its weakest, which is the band the margin is measured on.
    let tie = bands_with(&[(1, 0.0), (2, incumbent), (3, incumbent)]);
    assert_eq!(pick_mode(&tie, Some((2, 3)), 0.0), (2, 3));

    // The same challenger with its weak band past the margin.
    let clear = bands_with(&[(1, 0.0), (2, incumbent + MARGIN_DB + 0.1), (3, incumbent)]);
    assert_eq!(pick_mode(&clear, Some((2, 3)), 0.0), (1, 2));
}

/// The hold is the insurance against a genuine two-cycle: hysteresis always
/// permits one, and it was reported as a strobe.
#[test]
fn a_held_figure_cannot_be_moved() {
    let bands = bands_with(&[(3, -10.0), (4, -12.0)]);
    assert_eq!(pick_mode(&bands, Some((1, 2)), 1.0), (1, 2));
    // The same bands, one frame after the hold expires.
    assert_eq!(pick_mode(&bands, Some((1, 2)), 0.0), (3, 4));
}

/// Silence is where the strobe was worst: every band sits on the `-60` floor
/// and the top-two is decided by hundredths of a dB of FFT noise. The premise
/// guard is the point — it proves the input really is unordered noise, so a
/// green `switches == 0` is the fix and not a static input that never varied.
#[test]
fn the_quiet_floor_keeps_the_figure_rather_than_tracking_noise() {
    const FRAMES: usize = 600;
    let frame_bands = |frame: usize| -> [f32; VIZ_BANDS] {
        std::array::from_fn(|i| -60.0 + ((i * 7 + frame * 13) % 11) as f32 * 1e-3)
    };

    let winners: std::collections::HashSet<usize> = (0..FRAMES)
        .map(|frame| {
            frame_bands(frame)
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0
        })
        .collect();
    assert!(
        winners.len() > 1,
        "premise: the noise floor must not have a stable loudest band, or this \
         case proves nothing"
    );

    let mut switches = 0;
    let mut held = None;
    for frame in 0..FRAMES {
        let (n, m) = pick_mode(&frame_bands(frame), held, 0.0);
        if held.is_some_and(|p| p != (n, m)) {
            switches += 1;
        }
        held = Some((n, m));
    }
    assert_eq!(
        switches, 0,
        "the figure must not change on the noise floor ({switches} changes over \
         {FRAMES} frames, {winners:?} took the lead)"
    );
}

/// egui memory outlives a `MAX_MODE` edit and a rebuild, so a stored pair can be
/// out of the range the view can draw — or degenerate, which is a blank plate.
/// Re-picking is better than drawing either.
#[test]
fn a_stale_stored_pair_is_re_picked() {
    let bands = bands_with(&[(3, -10.0), (4, -12.0)]);
    let fresh = (3, 4);
    assert_eq!(
        pick_mode(&bands, Some((VIZ_BANDS - 1, VIZ_BANDS - 2)), 0.0),
        fresh
    );
    assert_eq!(pick_mode(&bands, Some((0, 4)), 0.0), fresh);
    assert_eq!(pick_mode(&bands, Some((2, 2)), 0.0), fresh);
    assert_eq!(
        pick_mode(&bands, Some((VIZ_BANDS - 1, VIZ_BANDS - 2)), 5.0),
        fresh,
        "a hold must not pin a pair the view cannot draw"
    );
}

/// `n == m` is a blank figure, and the old guard against it in the view could
/// never fire: it compared two *band indices* from `enumerate`, which are
/// distinct by construction. This is where the invariant is observable — a pair
/// that ties or reverses in level, and a range it has to stay inside.
///
/// The upper bound is written out rather than read from `MAX_MODE`, so that
/// raising the cap is a *visible* change here. The reason the cap exists moved
/// with the view: it used to be that a 48-cell grid cannot resolve a mode much
/// above 10, and the grid is gone. It is now that a nodal figure at mode 30
/// packs ~3 cells per oscillation on a 48-cell plate — the same arithmetic, the
/// same answer, and a different sentence because the drawing is a different one.
#[test]
fn the_picked_pair_is_distinct_and_inside_the_drawn_range() {
    const DRAWN_MAX: usize = 10;
    assert_eq!(
        DRAWN_MAX,
        MAX_MODE,
        "MAX_MODE moved to {MAX_MODE}: a nodal figure at that mode packs \
         {} oscillations per half-plate, so the lines land in the wrong place.",
        DRAWN_MAX as f32 / 3.0
    );
    for spread in [0.0, 0.5, 3.0, 12.0, 40.0, 59.9] {
        let bands = bands_with(&[(1, -20.0), (2, -20.0 - spread)]);
        for current in [None, Some((1, 2)), Some((9, 10))] {
            let (n, m) = pick_mode(&bands, current, 0.0);
            assert_ne!(n, m, "spread {spread}: a degenerate pair is a blank figure");
            let in_range = |i: usize| (1..=DRAWN_MAX).contains(&i);
            assert!(
                in_range(n) && in_range(m),
                "spread {spread}: ({n}, {m}) is outside the range the grid resolves"
            );
        }
    }
}

/// The range excludes band 0 for a reason, so pin it: a mode of 0 makes the
/// whole field term zero, so a pair using band 0 is a blank figure rather than a
/// second spelling of a pair the view can already draw — and repainting between
/// the two is the strobe again.
#[test]
fn band_zero_is_never_a_mode_number() {
    let bands = bands_with(&[(0, 0.0), (1, -10.0), (2, -12.0)]);
    let (n, m) = pick_mode(&bands, None, 0.0);
    assert_ne!(n, 0, "band 0 draws a blank figure");
    assert_ne!(m, 0, "band 0 draws a blank figure");
    assert_eq!((n, m), (1, 2));
}
