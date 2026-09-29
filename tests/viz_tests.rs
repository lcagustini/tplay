//! Visualizer DSP — ring buffer, FFT, and the two band-mapping functions.
//!
//! Lives here rather than in a `#[cfg(test)] mod tests` inside
//! `src/audio/viz.rs` because the project keeps every test in `tests/` (each
//! file is its own auto-discovered binary, run exactly once). It needs an audio
//! device for nothing: the tap source is covered at Source level in
//! `eq_live.rs`-style suites, and everything here is pure.

use std::f32::consts::PI;
use tplay::audio::viz::{
    compute_bands, compute_wave, fft_magnitude, mode_now, VizBuf, DB_FLOOR, FFT_SIZE, MAX_MODE,
    VIZ_BANDS,
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

// --- the Chladni mode read -------------------------------------------------
//
// `mode_now` replaced a hysteretic argmax over the two loudest band indices,
// with a 3 dB margin and a 1.5 s hold. That pick was the *only* channel the
// music had into the figure, and a **discrete** pick has discrete failure
// modes: two near-equal bands repaint the whole plate, so it needed the margin,
// and the hold then capped the figure at one change per 1.5 s by construction.
// The image was bit-identical between switches — reported as "the chladni viz is
// rendering at 5fps" and then, once that turned out to be a still image rather
// than a slow one, as "kinda still and unrelated to the music".
//
// None of that was a drawing problem. The field is continuous in both mode
// numbers, so any real `(n, m)` is a valid cymatic figure, and a *continuous*
// read of the spectrum needs no margin, no hold and no anti-strobe machinery at
// all. These are the properties that make it safe to have deleted them.

// --- the Chladni mode read -------------------------------------------------
//
// `mode_now` replaced a hysteretic argmax over the two loudest band indices,
// with a 3 dB margin and a 1.5 s hold. That pick was the *only* channel the
// music had into the figure, and a **discrete** pick has discrete failure
// modes: two near-equal bands repaint the whole plate, so it needed the margin,
// and the hold then capped the figure at one change per 1.5 s by construction.
// The image was bit-identical between switches — reported as "the chladni viz is
// rendering at 5fps" and then, once that turned out to be a still image rather
// than a slow one, as "kinda still and unrelated to the music".
//
// None of that was a drawing problem. The field is continuous in both mode
// numbers, so any real `(n, m)` is a valid cymatic figure, and a *continuous*
// read of the spectrum needs no margin, no hold and no anti-strobe machinery at
// all. These are the properties that make it safe to have deleted them.
//
// Which left a third defect, found by the same report: the read was capped at
// `MAX_MODE` **bands**, and the bands are log-spaced over 20 Hz .. 22 kHz, so
// band 10 is 222 Hz. The figure could see **25 Hz .. 222 Hz — 1% of the
// spectrum** — and every vocal, melody, snare and cymbal did nothing to it. One
// band of bass and one band of vocal produced *identical* mode pairs, which is
// what "unrelated to the song" turned out to mean.

/// The frame the pane runs at, for driving the read in these tests. Spelled out
/// rather than taken from egui so the numbers below mean something on their own.
const FPS: f32 = 60.0;

/// The band that divides the two groups, low from high.
const SPLIT: usize = 16;
/// A band well above it, in the group that drives the high mode number.
const HIGH: usize = 28;

/// Run the read at [`FPS`] until it has settled, and report where it landed.
///
/// The read follows its target over a time constant, so a single frame's value is
/// a few percent of the way there — which is correct and is what keeps the plate
/// calm, but it means "where does this spectrum put the figure" is a question
/// about the settled value. Two seconds is 8 time constants.
fn settled(bands: &[f32; VIZ_BANDS], from: (f32, f32)) -> (f32, f32) {
    let mut now = from;
    for _ in 0..(2.0 * FPS) as usize {
        now = mode_now(bands, now, 1.0 / FPS);
    }
    now
}

/// The whole spectrum reaches the plate, and this is the regression test for the
/// bug that made the view bass-only.
///
/// `MAX_MODE` bounds the **mode** number — how many nodal lines fit on the plate
/// — and it was also being used as the highest readable **band**. The bands are
/// log-spaced over 20 Hz .. 22 kHz, so band 10 is 222 Hz: capping the band index
/// at 10 left the figure seeing 25 Hz .. 222 Hz and nothing else. Every vocal,
/// melody, snare and cymbal in the music did literally nothing to it.
///
/// The assertion is the strongest form of this that exists, because a bug of this
/// shape cannot be caught by "the read is continuous" or "the pair is in range" —
/// both were true while the top two thirds of the spectrum was being thrown away.
/// It has to be a tone *up there* moving the figure.
#[test]
fn the_whole_spectrum_reaches_the_plate() {
    let from = (1.0, 6.0);
    let peak_at = |band: usize| {
        let mut bands = [DB_FLOOR; VIZ_BANDS];
        bands[band] = 0.0;
        settled(&bands, from)
    };
    // A tone in each quarter of the spectrum, so nothing is inferred from one end.
    let bass = peak_at(2);
    let low_mid = peak_at(12);
    let mid = peak_at(20);
    let air = peak_at(30);

    assert!(
        low_mid.0 > bass.0 + 1.0,
        "moving a tone from band 2 to band 12 must move the low mode: \
         {bass:?} -> {low_mid:?}"
    );
    assert!(
        mid.1 > bass.1 + 0.5,
        "a tone at band 20 (1.6-2 kHz) must move the high mode — it did nothing \
         at all while the read stopped at band 10: {bass:?} -> {mid:?}"
    );
    assert!(
        air.1 > mid.1 + 1.0,
        "and a tone at band 30 (14-17 kHz) further still: {mid:?} -> {air:?}"
    );
}

/// A small change in the spectrum moves the figure a lot, and that is the whole
/// claim.
///
/// Not "moves" — moves *visibly*, because a read can be continuous in form and
/// still visually frozen: a linear barycentre sat near the middle of its range
/// and wandered a third of a mode over 15 s, which is the same complaint in a
/// different form. A mode number moves the nodal lines, so a shift of 0.01 is
/// continuous and useless.
#[test]
fn the_plate_moves_with_the_music() {
    let from = (1.0, 6.0);
    // Two bands *within* each half, with the energy traded from the low one to
    // the high one. Within a half is the whole point: `n` reads the low half and
    // `m` the high one, so moving energy across the split would leave each
    // reading untouched and prove nothing about either.
    let low = bands_with(&[(2, -6.0), (14, -20.0), (SPLIT + 4, -6.0), (HIGH, -20.0)]);
    let high = bands_with(&[(2, -20.0), (14, -6.0), (SPLIT + 4, -20.0), (HIGH, -6.0)]);
    let (low_n, low_m) = settled(&low, from);
    let (high_n, high_m) = settled(&high, from);

    assert!(
        high_n > low_n && high_m > low_m,
        "trading energy up each group must push both modes up — low \
         ({low_n:.2}, {low_m:.2}), high ({high_n:.2}, {high_m:.2})"
    );
    // And by a visible amount, because a mode number moves the nodal lines: a
    // read that shifted by 0.01 would be continuous and useless, and a centroid
    // of a bass-heavy spectrum did exactly that.
    assert!(
        high_n - low_n > 0.5 && high_m - low_m > 0.5,
        "the plate must move by a visible amount — low ({low_n:.2}, {low_m:.2}), \
         high ({high_n:.2}, {high_m:.2})"
    );
}

/// One half of the spectrum can be silent while the other is not.
///
/// A bass note with nothing above it is ordinary music, and a read that insists
/// on both halves having energy holds the whole figure still for it — which is
/// the same stillness the pick had, arrived at by a different route.
#[test]
fn one_silent_half_still_moves_the_other() {
    let from = (3.0, 7.0);
    let (n, m) = settled(&bands_with(&[(2, -6.0)]), from);
    assert!(
        (n - from.0).abs() > 0.5,
        "a loud low band with a silent high group must still move n: \
         {from:?} -> ({n:.2}, {m:.2})"
    );
    assert_eq!(
        m, from.1,
        "the silent group has no opinion, so it keeps its figure"
    );

    let (n, m) = settled(&bands_with(&[(HIGH, -6.0)]), from);
    assert_eq!(n, from.0);
    assert!(
        (m - from.1).abs() > 0.5,
        "and symmetrically for the high group: {from:?} -> ({n:.2}, {m:.2})"
    );
}

/// The read is continuous in the *levels*, including where the dominant band
/// changes hands.
///
/// This is the property the pick could not have had and no test could have
/// checked: a discrete argmax is discontinuous *by construction* at exactly this
/// crossover, and the pick's own tests could only count switches after the fact.
/// A barycentre has no such point — the two bands trade and the read slides
/// between them.
///
/// The ramp is one band's *level* rising past a fixed one rather than a band
/// index stepping, because a spectrum is continuous in level and not in index:
/// an input that moved a whole band at a time is a step function, and a
/// continuous function of a step function is allowed to step.
#[test]
fn the_read_is_continuous_through_a_handover() {
    let steps = 200;
    // Settle on the *start* of the ramp first. The read is followed, not jumped
    // to, so a first frame launched from an arbitrary seed would report the
    // settling transient rather than the handover this is about.
    let start = bands_with(&[(14, -6.0), (2, DB_FLOOR)]);
    let mut now = settled(&start, (1.0, 6.0));
    let mut last = f32::NEG_INFINITY;
    let mut biggest_step = 0.0f32;
    for step in 0..=steps {
        // Band 2 rises from the floor past a fixed band 14, so the loudest band
        // in the low group hands over partway up the sweep.
        let rising = DB_FLOOR + (step as f32 / steps as f32) * 60.0;
        let mut bands = bands_with(&[(14, -6.0), (2, rising)]);
        bands[SPLIT + 4] = -6.0;
        let (n, _) = mode_now(&bands, now, 1.0 / FPS);
        now = (n, now.1);
        if last.is_finite() {
            biggest_step = biggest_step.max((n - last).abs());
        }
        last = n;
    }
    // The whole sweep travels about four modes in 200 steps, so a followed read
    // moves ~0.02 per step. A handover that jumped would be a whole band at once.
    assert!(
        biggest_step < 0.1,
        "the read stepped by {biggest_step:.3} of a mode in one frame of a 200-step \
         level ramp — a handover is exactly where a discrete pick would jump"
    );
}

/// The read is calmer than the noise it is fed.
///
/// A barycentre is a *ratio*, so a hundredth of a dB of FFT noise in one bin
/// moves it several times more than it moves that bin, and the squared weights
/// that make it track peaks widen that further. Unsmoothed it moved the plate
/// about as much on a held sample as on real music, which reads as a broken
/// plate rather than a calm one — and it is why there is a time constant here at
/// all, in a view that deliberately has no hold.
///
/// The premise is the point: a *constant* input cannot legitimately move the
/// figure far, so what is measured below is the floor that real music has to beat.
#[test]
fn the_read_is_calmer_than_the_noise_it_is_fed() {
    /// Largest single-frame movement over a second of the same input repeated.
    fn worst_step(bands: &[f32; VIZ_BANDS]) -> f32 {
        let mut now = settled(bands, (1.0, 6.0));
        let mut last = (f32::NAN, f32::NAN);
        let mut worst = 0.0f32;
        for _ in 0..FPS as usize {
            let (n, m) = mode_now(bands, now, 1.0 / FPS);
            now = (n, m);
            if last.0.is_finite() {
                worst = worst.max((n - last.0).abs() + (m - last.1).abs());
            }
            last = (n, m);
        }
        worst
    }
    // The same level on every band, so the *barycentre* is fixed and nothing
    // about the music changes from frame to frame.
    let flat: Vec<(usize, f32)> = (0..VIZ_BANDS).map(|i| (i, -18.0)).collect();
    let flat = bands_with(&flat);
    let jitter = worst_step(&flat);
    // A tenth of a mode per frame: still, or drifting once per second at most.
    assert!(
        jitter < 0.1,
        "a completely unchanging spectrum moved the plate by {jitter:.3} of a mode in \
         one frame — the read is following FFT noise rather than the music"
    );
}

/// Silence holds the figure instead of snapping it to a range edge.
///
/// The read has no opinion when there is no energy anywhere, and a read that
/// guesses one parks the plate against a clamp for as long as the music is quiet
/// — which is most of the time between tracks.
#[test]
fn silence_holds_the_figure() {
    let held = (3.5, 7.25);
    assert_eq!(mode_now(&bands_with(&[]), held, 1.0 / FPS), held);
    // And a degenerate frame time cannot unfreeze it either, which is the one
    // direction a clock must not fail in: egui's predicted frame time is
    // legitimately zero on a session's first frame.
    let loud = bands_with(&[(2, -6.0)]);
    assert_eq!(mode_now(&loud, held, 0.0), held);
    assert_eq!(mode_now(&loud, held, f32::NAN), held);
    assert_eq!(mode_now(&loud, held, -1.0), held);
}

/// The degenerate `n == m` figure is unreachable, not merely unlikely.
///
/// `n == m` cancels the field to nothing, so the plate goes blank. The old guard
/// against it was a comparison in the view, which could not fire (it compared
/// two band *indices* from `enumerate`, distinct by construction); the pick's
/// own test is where the invariant became observable. Now it is structural —
/// two disjoint halves of the eligible range — so there is no check left to get
/// wrong, and the sweep is over every reachable pair of peaks and level spread,
/// because the property is about the ranges and not about any one input.
///
/// The upper bound is written out rather than read from `MAX_MODE`, so raising
/// the cap is a *visible* change here. The reason the cap exists moved with the
/// view: it used to be that a 48-cell grid cannot resolve a mode much above 10,
/// and the grid is gone. It is now that a nodal figure at mode 30 packs ~3 cells
/// per oscillation on a plate's half-width — the same arithmetic, the same
/// answer, and a different sentence because the drawing is a different one.
#[test]
fn the_pair_is_never_degenerate_and_stays_in_the_drawn_range() {
    const DRAWN_MAX: usize = 10;
    assert_eq!(
        DRAWN_MAX,
        MAX_MODE,
        "MAX_MODE moved to {MAX_MODE}: a nodal figure at that mode packs \
         {} oscillations per half-plate, so the lines land in the wrong place.",
        DRAWN_MAX as f32 / 3.0
    );
    // Every pair of peaks across the *whole* readable range, not just the low
    // end — the bug this replaced made everything above band 10 unreadable, and a
    // sweep bounded by `MAX_MODE` would have quietly agreed with it.
    for i in 1..VIZ_BANDS {
        for j in 1..VIZ_BANDS {
            for spread in [0.0, 0.5, 3.0, 12.0, 40.0, 59.9] {
                let bands = bands_with(&[(i, -20.0), (j, -20.0 - spread)]);
                let (n, m) = settled(&bands, (1.0, 6.0));
                assert!(
                    n < m,
                    "peaks ({i}, {j}) at spread {spread} gave ({n:.3}, {m:.3}) — a \
                     degenerate pair is a blank plate and a reversed one a mirrored one"
                );
                assert!(
                    (1.0..=DRAWN_MAX as f32).contains(&n) && (1.0..=DRAWN_MAX as f32).contains(&m),
                    "peaks ({i}, {j}) at spread {spread} gave ({n:.3}, {m:.3}), outside \
                     the range the plate can resolve"
                );
            }
        }
    }
}

/// Band 0 is not a mode number, and the reason is worth a test of its own: a
/// mode of 0 makes the whole field term zero, so it draws a blank figure rather
/// than a second spelling of a pair the view can already draw. The input is the
/// one that would produce it — band 0 the loudest thing in the array, which is
/// what a DC-heavy or clipped frame looks like.
#[test]
fn band_zero_is_never_a_mode_number() {
    let bands = bands_with(&[(0, 0.0), (1, -6.0), (7, -6.0)]);
    let (n, m) = settled(&bands, (1.0, 6.0));
    assert!(n >= 1.0, "band 0 draws a blank figure, got n = {n:.3}");
    assert!(m >= 1.0, "band 0 draws a blank figure, got m = {m:.3}");
}
