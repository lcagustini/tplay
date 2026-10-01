//! Visualizer DSP — ring buffer, FFT, and the two band-mapping functions.
//!
//! Lives here rather than in a `#[cfg(test)] mod tests` inside
//! `src/audio/viz.rs` because the project keeps every test in `tests/` (each
//! file is its own auto-discovered binary, run exactly once). It needs an audio
//! device for nothing: the tap source is covered at Source level in
//! `eq_live.rs`-style suites, and everything here is pure.

use std::f32::consts::PI;
use tplay::audio::viz::{
    compute_bands, compute_wave, fft_magnitude, mode_now, smoothing_for_a_dt, VizBuf, DB_FLOOR,
    FFT_SIZE, MAX_MODE, VIZ_BANDS, VIZ_BUFFER_CAP,
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
    // With no clock the cursor re-anchors to the newest sample, so the first
    // read is the whole tail. `n` above the ring's length yields what exists,
    // which is what the FFT caller reads as "not enough data yet".
    let tail = buf.read_window(2, 0.0);
    assert_eq!(tail, vec![0.5, -0.25]);
    assert!(buf.read_window(10, 0.0).len() <= 2);
}

/// **The read cursor is the stutter fix, and this is the measurement that found
/// it.** Audio does not reach the tap one sample at a time: `TapSource::next`
/// runs inside rodio's mixer pull, and cpal asks for a whole `buffer_size` per
/// output callback — 8192 samples, one call every ~186 ms at the shipped
/// default. A reader that takes "the newest N samples" therefore sees the same
/// window for ~22 frames at 120 fps and then a window of unrelated audio.
/// Measured on the real fixture, that was **0 distinct spectra across 22 idle
/// frames**: the picture froze and snapped, and no frame rate could fix it,
/// because the discontinuity was in the data.
///
/// The cursor walks the backlog out at real-time rate instead, so every read is a
/// slightly later slice of the same burst.
#[test]
fn the_read_cursor_turns_an_audio_burst_into_many_small_steps() {
    let buf = VizBuf::new();
    buf.set_rate(44100);
    let burst = 8192usize;
    let frame = 1.0 / 120.0;
    let per_frame = 44100.0 * frame;
    // The very first read re-anchors to the newest sample, so establish that
    // before pushing, or the burst has nothing to walk.
    for i in 0..1024 {
        buf.push(i as f32);
    }
    buf.read_window(1024, frame);
    for i in 0..burst {
        buf.push((burst + i) as f32);
    }

    let mut first = buf.read_window(1024, frame);

    // Every read while the backlog lasts must move, and must move by about one
    // frame's worth of samples rather than the whole burst. `frames` is however
    // many reads the burst can feed, minus one: the last read legitimately finds
    // an empty backlog, and asserting on it would be asserting that audio
    // appeared from nothing.
    let frames = burst / per_frame as usize - 1;
    let mut moved = 0usize;
    for _ in 0..frames {
        let next = buf.read_window(1024, frame);
        assert_ne!(
            next, first,
            "the cursor did not advance on read {moved}, so the window is frozen"
        );
        // The window holds a ramp, so its newest sample is the cursor position.
        let step = next[next.len() - 1] - first[first.len() - 1];
        assert!(
            (step - per_frame).abs() < per_frame * 0.5,
            "the cursor advanced {step} samples on read {moved}; it should advance about \
             {per_frame}, which is what splits one burst into many small steps"
        );
        moved += 1;
        first = next;
    }
    assert_eq!(moved, frames);
    assert!(
        frames > 15,
        "one 8192-sample burst must feed many frames of reads, not two or three — got \
         {frames}, which is the stutter back"
    );

    // The backlog drains over many reads rather than being spent in one jump: the
    // window has not yet reached the newest sample the burst pushed.
    assert!(
        first[first.len() - 1] < (burst + 1024 + burst) as f32,
        "after {frames} reads the cursor is still short of the newest sample, so the burst \
         was walked out gradually rather than consumed whole"
    );
}

/// **Two reads in one frame, and the arrival must survive the second.** This is
/// the bug that blanked the spectrogram.
///
/// Every view calls `compute_bands` *and* `compute_wave` (the latter inside
/// `Uniforms::pack`), so there are two reads per frame. When the second one
/// advanced the cursor too, it consumed the arrival flag that
/// `take_arrival` reports — and the two feedback views are the only ones that
/// call `take_arrival`. Trails kept its previous frame; the spectrogram, which
/// has no history to fall back on, showed nothing at all.
///
/// So `compute_wave` peeks (`read_window_peek`) and an advancing read only ever
/// **ORs** into the flag. Both halves are here.
#[test]
fn a_second_read_in_a_frame_does_not_consume_the_arrival() {
    let buf = VizBuf::new();
    buf.set_rate(44100);
    let frame = 1.0 / 120.0;
    buf.read_window(1024, frame); // establish, re-anchors

    for i in 0..2048 {
        buf.push(i as f32);
    }
    // Frame N: the advancing read, then the peeking read, then the take. That is
    // the exact order `spectrogram::draw` performs.
    let first = buf.read_window(1024, frame);
    let second = buf.read_window_peek(1024);
    assert!(
        buf.take_arrival(),
        "the spectrogram reads twice per frame and then asks whether to accumulate; a \
         peeking read must not leave that question answered 'no', or the view never draws \
         a column"
    );
    // The flag is consumed, so a *second* take with no new audio must be false —
    // otherwise one burst would be spent once per reader.
    assert!(
        !buf.take_arrival(),
        "the arrival is consumed by the take, so one push cannot advance a history twice"
    );

    // The peek must also not have moved the cursor, or it spends the backlog twice
    // as fast as the audio arrives and the next frame jumps.
    let third = buf.read_window(1024, frame);
    let step = third[third.len() - 1] - first[first.len() - 1];
    let per_frame = 44100.0 * frame;
    assert!(
        (step - per_frame).abs() < per_frame * 0.5,
        "one frame advanced the cursor by {step} samples; with a peek in between it must still \
         be about {per_frame}, because the backlog is walked out at the audio's own rate"
    );
    // And the peek returns the window as it stood, which is the same slice the
    // advancing read just returned — a peek is a read, not a move.
    assert_eq!(
        second, first,
        "a peeking read must return the same window the advancing read just returned"
    );
    // **The invariant itself, stated so a peek cannot become an advancing read.**
    // A peek that took a `dt` would move the cursor and spend the backlog twice
    // per frame, which is the bug: the second read consumed the arrival, so the
    // spectrogram drew no column at all. `compute_wave` reads 1024 samples every
    // frame for the wave view, so this runs 120 times a second.
    buf.push(999.0);
    let peeked = buf.read_window_peek(1024);
    let after_peek = buf.read_window(1024, frame);
    let step = after_peek[after_peek.len() - 1] - peeked[peeked.len() - 1];
    assert!(
        (step - per_frame).abs() < per_frame * 0.5,
        "one advancing read after a peek moved the cursor {step} samples; the peek must not \
         have moved it at all, so a frame's two reads spend the backlog once. A peek that \
         advances spends it twice, which is what blanked the spectrogram."
    );

    // **The whole frame, twice, over and over** — which is the shape that blanked
    // it. Audio arrives in bursts, and every frame after a burst draws a column
    // while the cursor walks the backlog out. Measured on the committed fixture
    // with the real `spectrogram::draw` order (advancing read, peek, take): 240
    // columns over 240 frames, one per frame. With the second read advancing as
    // well, the arrival was consumed by it and the count was 0.
    let mut columns = 0usize;
    for f in 0..240 {
        // A burst, then 21 quiet frames — the cpal callback's period at 120 fps.
        if f % 22 == 0 {
            for i in 0..8192 {
                buf.push((f * 8192 + i) as f32);
            }
        }
        buf.read_window(1024, frame);
        let _ = buf.read_window_peek(1024);
        if buf.take_arrival() {
            columns += 1;
        }
    }
    // **Every frame, not every burst.** A burst is 22 frames of backlog, and the
    // cursor walks it out one frame at a time, so all 22 must draw. Asserting
    // "one column per burst" instead would pass with the stutter back.
    assert_eq!(
        columns, 240,
        "the spectrogram must draw a column on every frame it has audio for — all 240, not one \
         per burst. Got {columns}. Zero means the view shows nothing at all, which is what the \
         second advancing read caused."
    );
}

/// **`wave` is the only view with no advancing read of its own, and that has to
/// be fixed somewhere or its picture freezes.**
///
/// The symptom is a frozen envelope rather than a broken one: the shader reads
/// whatever window it is handed correctly, so nothing in the WGSL is wrong, and a
/// still waveform looks like a paused one. The cause is that every other view
/// advances the cursor as a side effect of `compute_bands` — which `wave`, being
/// a function of time rather than of frequency, does not call. Its cursor therefore
/// stood still for a whole ring's worth of audio between reads.
#[test]
fn a_standalone_advance_moves_the_cursor_that_a_peek_then_reads() {
    const FRAME: f32 = 1.0 / 60.0;
    let buf = VizBuf::new();
    buf.set_rate(44100);

    // Audio arriving *while* the frames run, which is the arrangement that has a
    // backlog to walk out. Pushing three rings first and advancing afterwards
    // would not: the first `advance` re-anchors onto `written`, and a cursor at
    // the newest sample has nothing left to consume.
    //
    // **A burst every 20 frames, and the number is load-bearing.** One burst is
    // 8192 samples and 20 frames consume about 14 700, so the backlog *drains*
    // between bursts and stays under the 16 384-sample ring. A burst every tenth
    // frame outruns the cursor, the trail eventually exceeds the ring, and the
    // re-anchor fires — a jump of the whole backlog, which is correct behaviour
    // and would fail the step assertion below for a reason that has nothing to
    // do with what is being pinned.
    let mut pushed = 0.0f32;
    let mut last = None;
    let mut steps = Vec::new();
    for frame in 0..60 {
        let burst = frame % 20 == 0;
        let n = if burst { 8192 } else { 735 };
        for _ in 0..n {
            buf.push(pushed);
            pushed += 1.0;
        }
        buf.advance(FRAME);
        let window = buf.read_window_peek(1024);
        assert_eq!(window.len(), 1024, "a peek must serve a full window");
        if let Some(prev) = last {
            steps.push(window.last().unwrap() - prev);
        }
        last = window.last().copied();
    }

    assert!(
        steps.iter().all(|s| *s > 0.0),
        "the window must end at a newer sample every frame, or the envelope is a frozen burst \
         rather than a moving waveform. Steps: {steps:?}"
    );
    // The point of the cursor: on a burst frame it advances by one frame, not by
    // the whole burst. A burst is 8192 samples, so a step near that would mean
    // the walk is a jump.
    let biggest = steps.iter().cloned().fold(0.0f32, f32::max);
    assert!(
        biggest < 1200.0,
        "no frame may advance the cursor by more than about one frame's worth (735 samples at \
         44.1 kHz). Biggest step was {biggest}."
    );
}

/// The cursor must not walk off the end of the ring, and a `clear` must reset it
/// — otherwise the window is served from samples that have already been
/// overwritten, which is the same stutter by another route.
#[test]
fn the_read_cursor_stays_inside_the_ring_and_resets() {
    let buf = VizBuf::new();
    buf.set_rate(44100);
    // Far more than the ring holds, so the cursor is forced off the back.
    for i in 0..(VIZ_BUFFER_CAP * 4) {
        buf.push(i as f32);
    }
    let window = buf.read_window(1024, 1.0 / 60.0);
    assert_eq!(window.len(), 1024, "the window must always be full");
    // Every sample must be one we actually pushed, i.e. contiguous ascending
    // ramp values with no wrap or duplicate.
    for pair in window.windows(2) {
        assert_eq!(
            pair[1] - pair[0],
            1.0,
            "the window is not a contiguous slice of the ramp"
        );
    }

    buf.clear();
    assert!(
        buf.read_window(1024, 1.0 / 60.0).is_empty(),
        "a clear resets the cursor along with the ring, so a cleared buffer reads as empty \
         rather than serving samples that were never pushed"
    );
}

/// **A peek-only reader is a real reader, and it must not fall off the ring.**
///
/// `wave` is the only view with no `compute_bands` call, so it reaches the buffer
/// through `Uniforms::pack` -> `compute_wave` -> `read_window_peek` and *nothing*
/// else. Its cursor therefore never advances, so it only ever read the ring if the
/// re-anchor in `read_locked` were reachable without `advance` — which it was not.
/// The ring turned over underneath a cursor stuck at wherever `clear` left it, and
/// after one ring's worth of audio (~372 ms) `cursor < oldest`. Two wrong answers
/// fell out of that one line, and which one you saw depended on the build:
///
/// - `state.cursor - oldest` wrapped to about 2^64, so `.min(len)` capped the
///   window at the whole ring — an envelope of stale audio rather than a panic;
/// - with a floor instead, `end` became 0 and the window was **empty**, so the
///   wave view drew nothing at all.
///
/// Neither is the current behaviour, and neither is observable from the outside —
/// a wave that draws nothing and a wave that draws the wrong thing are the same
/// picture to a user. So the assertion is on the samples, not the length: the
/// envelope must end at the newest sample pushed.
#[test]
fn a_peek_only_reader_never_falls_off_the_ring() {
    let buf = VizBuf::new();
    buf.set_rate(44100);
    for i in 0..(VIZ_BUFFER_CAP * 3) {
        buf.push(i as f32);
    }

    // No advancing read has ever run, so this is the wave view's exact position.
    let window = buf.read_window_peek(1024);
    assert_eq!(
        window.len(),
        1024,
        "a peek must serve a full window even when its cursor has been left behind"
    );
    // And it must be the newest `n` samples, which is what a peak envelope of the
    // present moment means — not the oldest thing still in the ring.
    let last = (VIZ_BUFFER_CAP * 3 - 1) as f32;
    assert_eq!(
        *window.last().unwrap(),
        last,
        "the peek must end at the newest sample pushed, not at the stale cursor"
    );

    // A cleared buffer is the state a new song starts from.
    buf.clear();
    assert!(
        buf.read_window_peek(1024).is_empty(),
        "a cleared buffer reads as empty through the peek path too"
    );
}

#[test]
fn viz_buf_clear() {
    let buf = VizBuf::new();
    buf.push(1.0);
    buf.clear();
    assert!(buf.read_window(10, 1.0 / 60.0).is_empty());
}

/// An arrival is reported **once**, and only once.
///
/// This is the whole of "the visualizer pauses with the song". The two feedback
/// views hold a history, and a history advances by one frame per call — so
/// without this the spectrogram kept scrolling a picture of silence, and it
/// paused because nobody was pushing samples rather than because anything asked
/// it to. Both halves are here: `take` is what makes one arrival one frame rather
/// than one frame per viewer, and `push` being the only writer is what makes it
/// true while a sink is paused (the mixer is not calling `TapSource::next`, so
/// nothing is pushed, so there is nothing to report).
#[test]
fn an_arrival_is_reported_once_and_only_while_audio_is_pushed() {
    // The order matters and mirrors the app: a view **reads** (which folds in
    // whatever arrived) and then **takes** the flag. A push on its own is not
    // reported, because the read is what turns pending audio into a position the
    // view can act on — and that is what keeps a burst from being spent twice.
    let buf = VizBuf::new();
    let frame = 1.0 / 120.0;
    buf.read_window(1024, frame);
    assert!(
        !buf.take_arrival(),
        "a fresh buffer has had no audio, and a view must not record a frame of it"
    );

    buf.push(0.5);
    buf.read_window(1024, frame);
    assert!(buf.take_arrival(), "a pushed sample is an arrival");
    assert!(
        !buf.take_arrival(),
        "one arrival must not be counted twice: the next frame has no new audio, so a view \
         that reads this twice in a frame would advance its history twice for one push"
    );

    buf.push(0.25);
    buf.read_window(1024, frame);
    assert!(buf.take_arrival(), "a second push is a second arrival");

    // Stopped playback clears the buffer, and a clear is not an arrival either —
    // `stop()` is the only caller, and the frame after it must hold, not advance.
    buf.push(1.0);
    buf.read_window(1024, frame);
    assert!(buf.take_arrival());
    buf.clear();
    assert!(
        !buf.take_arrival(),
        "clearing is what a stop does, and a stopped view holds its last frame"
    );
}

/// **Exactly one read per frame may advance the cursor, and any read may report an
/// arrival.** Both rules are about *which branch runs*, and no runtime measurement
/// can see either: a peeking read is handed `dt = 0.0`, so making it advance
/// moves nothing and every behavioural test still passes. Three mutations of
/// `read_window_peek` and `read_locked` survived the behavioural suite — this is
/// the source-reading pin for them, and it exists for the same reason
/// `the_wgpu_harness_never_names_a_view` does: a shader string or a hidden branch
/// is invisible from outside.
///
/// The failure it guards is the spectrogram rendering nothing. Two reads per frame
/// (`compute_bands`, then `compute_wave` inside `Uniforms::pack`) with the second
/// advancing consumed the arrival, and the two feedback views are the only ones
/// that call `take_arrival`.
#[test]
fn the_read_cursors_advance_and_report_rules_are_stated_in_one_place() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/audio/viz.rs");
    let text = std::fs::read_to_string(&path).unwrap();
    let code: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    // A peek passes `advance = false`, and `compute_wave` — the only caller — is
    // the read that used to consume the arrival.
    let peek = code
        .split("pub fn read_window_peek")
        .nth(1)
        .and_then(|s| s.split("fn read_locked").next())
        .expect("read_window_peek must exist");
    assert!(
        peek.contains("false"),
        "read_window_peek must not advance the cursor. It is the second read of every frame \
         (compute_wave, inside Uniforms::pack), and an advancing read there consumes the \
         arrival flag that take_arrival reports — so the spectrogram and trails draw no \
         column and the spectrogram shows nothing at all."
    );
    let advancing = code
        .split("pub fn read_window(")
        .nth(1)
        .and_then(|s| s.split("fn read_locked").next())
        .expect("read_window must exist");
    assert!(
        advancing.contains("true"),
        "read_window is the frame's one advancing read — it is what walks the audio backlog \
         out at real-time rate, and it is the whole stutter fix."
    );

    // The flag folds a pending arrival on *any* read, and only ever ORs. Both
    // halves were bugs: gating the fold on `advance` loses an arrival when a
    // frame's reads all peek, and assigning instead of OR-ing lets a second read
    // overwrite a true with a false.
    let folded = code
        .split("let oldest = state.written.saturating_sub")
        .nth(1)
        .expect("read_locked must exist");
    let flag_line = folded
        .lines()
        .find(|l| l.contains("state.advanced ="))
        .expect("read_locked must set state.advanced")
        .trim();
    assert!(
        flag_line.contains("state.advanced ||"),
        "the arrival fold must OR into `advanced`, never assign. Assigning lets the frame's \
         second read overwrite a true with a false, which is the spectrogram's blank. Got: \
         `{flag_line}`"
    );
    assert!(
        !folded.contains("if advance {\n            state.advanced")
            && !folded.contains("if advance && state.advanced")
            && !flag_line.contains("if advance"),
        "the arrival fold must not be gated on `advance`. A push is an arrival whether or not \
         this read moves the cursor, so a frame whose reads all peek must still report one — \
         and the gate is as easy to write on the line above the fold as around it, so the \
         whole body is checked."
    );
    let clears = folded
        .lines()
        .find(|l| l.contains("arrived_since_read = false"))
        .expect("read_locked must clear arrived_since_read");
    assert!(
        !clears.trim_start().starts_with("if"),
        "arrived_since_read is consumed by the fold above it, not conditionally — got \
         `{}`",
        clears.trim()
    );
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
    fn channels(&self) -> tplay::rodio::ChannelCount {
        tplay::rodio::ChannelCount::new(1).expect("1 is not zero")
    }
    fn sample_rate(&self) -> tplay::rodio::SampleRate {
        tplay::rodio::SampleRate::new(self.rate).expect("rate is not zero")
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

/// The band smoother's time constants must reproduce the old per-frame fractions
/// at 60 Hz, and must **halve the strength at 120 Hz** — which is the whole point.
///
/// The first half is what makes the change safe to make without being able to look
/// at it: a 60 Hz display sees exactly the numbers that shipped, so only the
/// high-refresh case moves. The second half is the defect being fixed, stated as a
/// number: the same music on the same machine smooths twice as slowly per frame,
/// because the frame is half as long, and a percussive spectrum moves a band by
/// ~10 dB between consecutive frames.
///
/// `dt = 0` must hold rather than snap, for the reason `feedback_for_a_dt` gives:
/// egui's predicted `dt` is legitimately zero on a session's first frame, and
/// `exp(0.0)` being exactly 1.0 is the failure that would freeze a view for good.
#[test]
fn the_band_smoother_is_a_time_constant_and_not_a_frame_fraction() {
    // (per-frame fraction as it shipped, its time constant in ms)
    for (fraction, tau_ms) in [
        (0.3_f32, 46.7_f32), // bars / radial / chladni attack
        (0.92, 6.6),         // bars / radial release — deliberately fast
        (0.6, 18.2),         // chladni + spectrogram
        (0.5, 24.1),         // flame + trails attack
        (0.9, 7.2),          // trails release
    ] {
        let at_60 = smoothing_for_a_dt(1.0 / 60.0, tau_ms);
        assert!(
            (at_60 - fraction).abs() < 0.005,
            "at 60 Hz a {tau_ms} ms time constant must reproduce the {fraction} it replaced, \
             got {at_60} — a 60 Hz display must be unchanged by this"
        );
        // **The claim that matters is not the ratio, it is that the time constant
        // survives.** `a` is nonlinear in `dt`, so halving the frame does not halve
        // the step (0.92 -> 0.72 for a fast release), and asserting a ratio would
        // be asserting the shape of `exp`. Inverting it — the time constant a
        // coefficient *implies* — must come back as the number the view wrote down,
        // at every frame rate. That is frame-rate independence stated as an
        // equation, and it is what "the same music, the same smoothing" means.
        let mut previous = 0.0_f32;
        for hz in [144.0_f32, 120.0, 60.0, 30.0] {
            let dt = 1.0 / hz;
            let a = smoothing_for_a_dt(dt, tau_ms);
            assert!(
                a > previous,
                "a longer frame must take a *bigger* step toward the target: \
                 {hz} Hz -> {a} after {previous}"
            );
            previous = a;
            let implied_ms = dt / -(1.0 - a).ln() * 1000.0;
            assert!(
                (implied_ms - tau_ms).abs() < tau_ms * 0.02,
                "at {hz} Hz a {tau_ms} ms time constant must still be {tau_ms} ms of \
                 smoothing; this frame rate implies {implied_ms} ms"
            );
        }
    }
    // A clock that cannot be trusted must not move anything, and a zero time
    // constant is no smoothing rather than a division by zero.
    assert_eq!(smoothing_for_a_dt(0.0, 46.7), 0.0);
    assert_eq!(smoothing_for_a_dt(-1.0, 46.7), 0.0);
    assert_eq!(smoothing_for_a_dt(1.0 / 60.0, 0.0), 1.0);
    // Monotone in the time constant, so a view's number still means what it says.
    let mut last = f32::INFINITY;
    for tau in [1.0_f32, 6.6, 18.2, 46.7, 200.0, 1e6] {
        let a = smoothing_for_a_dt(1.0 / 60.0, tau);
        assert!(
            a < last,
            "a longer time constant must smooth less per frame: {tau} ms -> {a}"
        );
        last = a;
    }
}

#[test]
fn compute_bands_decays_when_empty() {
    let buf = VizBuf::new();
    let mut prev = [0.0f32; VIZ_BANDS];
    compute_bands(&buf, &mut prev, 1.0 / 60.0, 46.7, 6.6);
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
    // The first advancing read anchors the cursor at the newest sample, which is
    // what a standalone test needs — in the app `compute_bands` does it every
    // frame, and `compute_wave` then peeks at the same place.
    buf.read_window(1024, 1.0 / 60.0);
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
    buf.read_window(1024, 1.0 / 60.0);
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
