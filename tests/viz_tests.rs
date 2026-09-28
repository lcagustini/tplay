//! Visualizer DSP — ring buffer, FFT, and the two band-mapping functions.
//!
//! Lives here rather than in a `#[cfg(test)] mod tests` inside
//! `src/audio/viz.rs` because the project keeps every test in `tests/` (each
//! file is its own auto-discovered binary, run exactly once). It needs an audio
//! device for nothing: the tap source is covered at Source level in
//! `eq_live.rs`-style suites, and everything here is pure.

use eframe::egui;
use std::f32::consts::PI;
use tplay::audio::viz::{
    chladni_field, compute_bands, compute_level, compute_wave, fft_magnitude, pick_mode, VizBuf,
    FFT_SIZE, MARGIN_DB, MAX_MODE, VIZ_BANDS,
};
use tplay::gui::panes::visualizer::views::chladni::{segments, GRID};

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

/// Silence must report zero rather than NaN: the VU view scales by these without
/// a guard, and NaN through a scale propagates into every painted coordinate.
#[test]
fn compute_level_on_silence_is_zero_not_nan() {
    let (rms, peak) = compute_level(&VizBuf::new());
    assert_eq!((rms, peak), (0.0, 0.0));
}

#[test]
fn compute_level_of_a_full_scale_constant() {
    let buf = VizBuf::new();
    for _ in 0..512 {
        buf.push(1.0);
    }
    let (rms, peak) = compute_level(&buf);
    assert!((rms - 1.0).abs() < 1e-6, "rms {rms}");
    assert!((peak - 1.0).abs() < 1e-6, "peak {peak}");
}

/// A half-duty square is the cheapest signal that separates the two numbers: a
/// meter that reported `rms == peak` would be measuring the wrong one, and a
/// half-square RMS is exactly `sqrt(0.5)` with no tolerance to argue about.
#[test]
fn compute_level_separates_rms_from_peak() {
    let buf = VizBuf::new();
    for i in 0..512 {
        buf.push(if i % 2 == 0 { 1.0 } else { 0.0 });
    }
    let (rms, peak) = compute_level(&buf);
    assert!(
        (rms - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
        "rms {rms}"
    );
    assert_eq!(peak, 1.0);
    assert!(
        rms < peak,
        "rms must sit below the peak or the meter's two needles agree"
    );
}

/// The diagonal is the figure's mirror line, and the view relies on the field
/// vanishing there to read as a symmetric pattern.
#[test]
fn chladni_field_vanishes_on_the_diagonal() {
    for &t in &[0.1f32, 0.25, 0.5, 0.75, 0.9] {
        assert!(
            chladni_field(2, 1, t, t) < 1e-6,
            "not zero on the diagonal at {t}"
        );
    }
}

/// `n == m` is a degenerate mode: the two antisymmetric terms are identical and
/// cancel everywhere, so the figure would be blank. The view has to keep the two
/// mode numbers distinct, and this is what makes that necessary.
#[test]
fn chladni_modes_must_differ() {
    for &t in &[0.1f32, 0.25, 0.5, 0.75] {
        let (u, _) = (t, 1.0 - t);
        assert!(
            chladni_field(3, 3, t, u) < 1e-6,
            "(3,3) is blank at ({t}, {u})"
        );
    }
}

/// `abs()` of the difference makes the field mirror-symmetric across `gx == gy`,
/// which is what gives a Chladni figure its four-fold look.
#[test]
fn chladni_field_mirrors_across_the_diagonal() {
    let a = chladni_field(2, 1, 0.25, 0.75);
    let b = chladni_field(2, 1, 0.75, 0.25);
    assert!(a > 0.5, "the (2,1) figure must have structure, got {a}");
    assert!((a - b).abs() < 1e-6, "{a} vs {b} across the diagonal");
}

/// A mode number of 0 would make the whole term zero, so a view deriving its
/// modes from a band index must not be handed one un-clamped.
#[test]
fn chladni_field_clamps_a_zero_mode() {
    assert!(
        chladni_field(0, 2, 0.25, 0.75) > 0.5,
        "mode 0 must be treated as 1, not as a blank figure"
    );
}

// --- the mode pick ---------------------------------------------------------
//
// The view's figure is a *discrete* function of the two loudest bands, so every
// change of that pair is a redraw of the whole plate. A bare top-two picked a
// new pair 60 times in 15 s over a drifting bass line. These are the guard
// against that coming back.

/// The regression itself: two pairs within a hair of each other, and the figure
/// must not move. A challenger has to be `MARGIN_DB` louder on its *weaker*
/// band, not on its louder one — a pair is only as loud as its quieter member.
#[test]
fn a_near_tie_keeps_the_current_figure() {
    // The incumbent (1, 2) is weaker at -20; the challenger is (1, 3) at -19.
    // 1 dB is well inside the margin.
    let bands = bands_with(&[(1, -18.0), (2, -20.0), (3, -19.0)]);
    assert_eq!(
        pick_mode(&bands, Some((1, 2)), 0),
        (1, 2),
        "a 1 dB lead must not repaint the plate"
    );
}

/// The other half: a real change of the music's character still moves the
/// figure, or the fix would just be a figure that never changes.
#[test]
fn a_clear_winner_does_move_the_figure() {
    let bands = bands_with(&[(1, -50.0), (2, -50.0), (3, -10.0), (4, -12.0)]);
    assert_eq!(pick_mode(&bands, Some((1, 2)), 0), (3, 4));
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
    assert_eq!(pick_mode(&tie, Some((2, 3)), 0), (2, 3));

    // The same challenger with its weak band past the margin.
    let clear = bands_with(&[(1, 0.0), (2, incumbent + MARGIN_DB + 0.1), (3, incumbent)]);
    assert_eq!(pick_mode(&clear, Some((2, 3)), 0), (1, 2));
}

/// The hold is the insurance against a genuine two-cycle: hysteresis always
/// permits one, and it was reported as a strobe.
#[test]
fn a_held_figure_cannot_be_moved() {
    let bands = bands_with(&[(3, -10.0), (4, -12.0)]);
    assert_eq!(pick_mode(&bands, Some((1, 2)), 1), (1, 2));
    // The same bands, one frame after the hold expires.
    assert_eq!(pick_mode(&bands, Some((1, 2)), 0), (3, 4));
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
        let (n, m) = pick_mode(&frame_bands(frame), held, 0);
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
        pick_mode(&bands, Some((VIZ_BANDS - 1, VIZ_BANDS - 2)), 0),
        fresh
    );
    assert_eq!(pick_mode(&bands, Some((0, 4)), 0), fresh);
    assert_eq!(pick_mode(&bands, Some((2, 2)), 0), fresh);
    assert_eq!(
        pick_mode(&bands, Some((VIZ_BANDS - 1, VIZ_BANDS - 2)), 5),
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
/// raising the cap is a *visible* change here: `GRID` cannot resolve a mode much
/// above 10, and a test that reads the constant it is checking cannot say so.
#[test]
fn the_picked_pair_is_distinct_and_inside_the_drawn_range() {
    const DRAWN_MAX: usize = 10;
    assert_eq!(
        DRAWN_MAX,
        MAX_MODE,
        "MAX_MODE moved: {MAX_MODE} mode numbers over a {GRID}-cell grid is \
         {} cells per oscillation, so the interpolated lines land in the wrong \
         place. Widen the grid or leave the mode range alone.",
        2.0 * GRID as f32 / DRAWN_MAX as f32
    );
    for spread in [0.0, 0.5, 3.0, 12.0, 40.0, 59.9] {
        let bands = bands_with(&[(1, -20.0), (2, -20.0 - spread)]);
        for current in [None, Some((1, 2)), Some((9, 10))] {
            let (n, m) = pick_mode(&bands, current, 0);
            assert_ne!(n, m, "spread {spread}: a degenerate pair is a blank figure");
            let in_range = |i: usize| (1..=DRAWN_MAX).contains(&i);
            assert!(
                in_range(n) && in_range(m),
                "spread {spread}: ({n}, {m}) is outside the range the grid resolves"
            );
        }
    }
}

/// The range excludes band 0 for a reason, so pin it: mode 0 is clamped to 1 by
/// `chladni_field`, so a pair using band 0 is a second spelling of a pair the
/// view can already draw — and repainting between the two is the strobe again.
#[test]
fn band_zero_is_never_a_mode_number() {
    let bands = bands_with(&[(0, 0.0), (1, -10.0), (2, -12.0)]);
    let (n, m) = pick_mode(&bands, None, 0);
    assert_ne!(n, 0, "band 0 redraws mode 1's figure");
    assert_ne!(m, 0, "band 0 redraws mode 1's figure");
    assert_eq!((n, m), (1, 2));
}

/// The count is the frame cost — the whole figure is one mesh, two triangles per
/// segment — so this is a tripwire, not a proof. Mode, grid and ceiling are all
/// written out, because a budget that reads the constants it is guarding cannot
/// report that they moved.
#[test]
fn the_worst_figure_stays_within_its_segment_budget() {
    const WORST_MODE: usize = 10;
    const CELLS: usize = 48;
    const BUDGET: usize = 1200;
    assert_eq!(WORST_MODE, MAX_MODE, "the worst mode moved");
    assert_eq!(CELLS, GRID, "the grid moved");

    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
    let segs = segments(WORST_MODE, WORST_MODE - 1, rect);
    assert!(
        segs.len() <= BUDGET,
        "{} segments at mode {WORST_MODE} over a {CELLS}-cell grid, budget {BUDGET} — \
         GRID, MAX_MODE or NODAL_CUTOFF moved, and so did the per-frame cost",
        segs.len()
    );
}

// --- the contour -----------------------------------------------------------
//
// The figure is drawn as one batched mesh, so the segments are the drawing. A
// dangling end is a mark the eye reads as a speck rather than as a line, which
// is what the per-cell dot version was: every crossing emitted twice, once by
// each of the two cells sharing the edge.

fn plate_rect() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))
}

/// The claim the lattice is there for. Two cells sharing an edge must read the
/// *same* stored field value, so their interpolated join points are bit
/// identical — which is the whole difference between a contour and a spray of
/// dots that happens to look joined. A nodal set is a set of closed curves, so
/// no segment end may touch only one other.
#[test]
fn every_segment_end_meets_another_segment() {
    for &(n, m) in &[(1, 2), (3, 5), (5, 8), (8, 10), (10, 9)] {
        let segs = segments(n, m, plate_rect());
        assert!(!segs.is_empty(), "mode ({n}, {m}) drew nothing");
        let ends: Vec<egui::Pos2> = segs.iter().flat_map(|[a, b]| [*a, *b]).collect();
        for e in &ends {
            let meets = ends.iter().filter(|o| o.distance(*e) < 1e-3).count();
            assert!(
                meets >= 2,
                "mode ({n}, {m}): a dangling end at {e:?} — {meets} segment(s) meet there"
            );
        }
    }
}

/// The figure is centred and inscribed, so a point outside the pane's short
/// side is a coordinate mix-up rather than a shape. Cheap, and it catches the
/// kind of bug a convexity or continuity check reads as fine.
#[test]
fn the_figure_stays_inside_the_pane() {
    let rect = plate_rect();
    let limit = rect.width().min(rect.height());
    for &(n, m) in &[(1, 2), (4, 7), (10, 9)] {
        for [a, b] in segments(n, m, rect) {
            for p in [a, b] {
                assert!(
                    p.distance(rect.center()) <= limit,
                    "mode ({n}, {m}): {p:?} is outside the pane"
                );
            }
        }
    }
}

/// A segment joins two crossings on *different* edges of one cell, so it spans
/// at most the cell's diagonal. Longer than that means a crossing was placed off
/// the edge it belongs to, which is the one way the interpolation can lie.
#[test]
fn every_segment_spans_at_most_one_cell() {
    let rect = plate_rect();
    let cell = rect.width().min(rect.height()) / GRID as f32;
    for &(n, m) in &[(1, 2), (3, 5), (8, 10)] {
        for [a, b] in segments(n, m, rect) {
            let len = b.distance(a);
            assert!(
                len <= cell * std::f32::consts::SQRT_2,
                "mode ({n}, {m}): a {len:.2}pt segment over a {cell:.2}pt cell \
                 means a crossing was placed off the edge it belongs to"
            );
        }
    }
}
