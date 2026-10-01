//! Audio visualization — tap source, ring buffer, FFT, and smoothing.
//!
//! Pure logic: no UI, no audio device. The tap wraps the EQ source so it
//! visualizes exactly what is heard (post-EQ). The ring buffer is capped at
//! ~100 ms (4096 samples at 44.1 kHz). The FFT is a hand-rolled radix-2
//! 1024-point transform with a Hann window; smoothing is classic per-bin
//! attack/release for the "WMP bars" feel.

use rodio::Source;
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

/// Cap for the ring buffer (mono samples). 16384 @ 44.1 kHz ≈ 372 ms.
///
/// **It has to exceed the audio device's `buffer_size`, and that is the whole
/// reason it is this size.** cpal hands rodio a whole `buffer_size` per output
/// callback — 8192 at the shipped default — and `TapSource::next` runs once per
/// sample inside that one call. So the tap receives all 8192 samples at once,
/// and `read_window`'s cursor then walks them out over the frames that follow.
/// A ring smaller than one burst cannot hold the backlog: the cursor outruns the
/// ring, `read_window` re-anchors to the newest sample, and the picture jumps
/// again — measured at 8 distinct spectra over 22 frames with a 4096 ring, and a
/// 20.9 dB step in one frame. 16384 holds two default bursts, so the cursor
/// always has somewhere to walk.
///
/// `config.buffer_size` is clamped to 512..=65536 in `main.rs`, so a
/// hand-set 65536 would outrun this again. That is a ceiling worth naming rather
/// than a bug to design out: 65536 is 1.5 s of ring, and the visualizer
/// tolerating it buys nothing.
pub const VIZ_BUFFER_CAP: usize = 16384;
/// FFT window size — power of two. 1024 gives ~43 Hz bins at 44.1 kHz.
pub const FFT_SIZE: usize = 1024;
/// Number of log-spaced output bands for drawing.
pub const VIZ_BANDS: usize = 32;
/// Buckets in the wave envelope handed to a shader, in [`tplay::gui::panes::visualizer::gpu::Uniforms::wave`].
///
/// A **constant rather than a pane measurement**, which is the whole reason the
/// wave view became a shader's business: the uniform array's length is part of
/// its GLSL declaration, so it cannot move with the splitter.
///
/// **128, and the ceiling is why.** A `float` array's elements are allowed one
/// `vec4` slot each rather than four to one, so the array is up to 128 fragment
/// uniform *vectors* — and GL 3.3 core only guarantees
/// `MAX_FRAGMENT_UNIFORM_VECTORS >= 224`. With `u_bands` alongside it, 256
/// buckets would have put the block over a floor the spec actually promises, on a
/// limit no compiler checks: glslang links a 256-element array perfectly well and
/// a driver that cannot place it reports it at link time, which is a blank pane
/// with a log line on one machine and nowhere else. 128 plus 32 is 160, and
/// `the_uniform_block_fits_the_gl_33_floor` is the test that keeps it there.
///
/// The visual cost is small: 128 buckets over a ~780px pane is one bucket every
/// 6px, and the shader interpolates linearly between neighbours, so the envelope
/// is a coarser polyline rather than a coarser signal.
pub const WAVE_BUCKETS: usize = 128;

/// The dB floor [`compute_bands`] reports, and so the floor every view's `level()`
/// maps up from.
///
/// **One definition, because it is a boundary between two halves that cannot see
/// each other.** The DSP clamps into `-60.0..=0.0` and the shaders map that range
/// up to `0.0..=1.0` with `(d + 60.0) / 60.0` — and that expression was in six
/// view bodies plus two inlined loops, all hand-written and all currently right.
/// If the floor moved and only this side did, **nothing would fail**: every shader
/// still compiles, every value still in range, and the symptom is a view that
/// quietly compresses or clips its quiet end, differently in each one. So the
/// constant is interpolated into the GLSL prelude rather than typed into a shader,
/// and `the_db_floor_is_one_number_across_the_dsp_and_every_shader` is what keeps
/// the two ends honest.
pub const DB_FLOOR: f32 = -60.0;

/// The buffer plus the rate its samples were captured at, which the band→bin
/// mapping needs and only a source knows. Written once per track, so no
/// read-compare.
#[derive(Debug, Clone)]
struct VizState {
    /// Until a source reports its own: only observable with nothing playing.
    rate: u32,
    buf: VecDeque<f32>,
    /// How many samples have ever been pushed. An absolute index, so the
    /// reader can tell "no audio yet" from "audio, and here is how far through
    /// it we have read" without inspecting the ring.
    written: u64,
    /// The read cursor: an absolute index one past the newest sample a reader
    /// has looked at. This is what makes the picture slide instead of jump.
    /// See [`VizBuf::read_window`].
    cursor: u64,
    /// Set whenever the cursor moved, cleared by `take_arrival`.
    advanced: bool,
    /// Set by `push`, cleared by the next read. Separate from `advanced` because
    /// a push is news on its own: a burst that arrived but whose backlog the
    /// cursor has not reached yet is still audio that happened.
    arrived_since_read: bool,
    /// Has any read ever happened? The first read has no clock to spend and no
    /// backlog to walk, so it takes the tail outright; without this the cursor
    /// sits at `written = 0` and a cold buffer reads as empty.
    started: bool,
}

/// Shared ring buffer for the tap source → GUI.
/// Lock-free on the audio thread would be ideal; the mutex is uncontended
/// at 44.1 kHz pushes and a single snapshot per frame on the GUI thread.
/// ponytail: if contention ever shows up in profiling, swap for a crossbeam
/// lock-free ring or a double-buffered copy.
#[derive(Debug, Clone)]
pub struct VizBuf {
    inner: Arc<Mutex<VizState>>,
}

impl Default for VizBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl VizBuf {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VizState {
                rate: 44100,
                buf: VecDeque::with_capacity(VIZ_BUFFER_CAP),
                written: 0,
                cursor: 0,
                advanced: false,
                arrived_since_read: false,
                started: false,
            })),
        }
    }

    /// Called by `TapSource::new` — also at the crossfade arm, so a mixed-rate
    /// overlap briefly reports the incoming track's rate.
    pub fn set_rate(&self, rate: u32) {
        self.lock().rate = rate;
    }

    pub fn sample_rate(&self) -> u32 {
        self.lock().rate
    }

    /// Push one mono sample (audio thread).
    pub fn push(&self, sample: f32) {
        let mut state = self.lock();
        if state.buf.len() == VIZ_BUFFER_CAP {
            state.buf.pop_front();
        }
        state.buf.push_back(sample);
        state.written += 1;
        state.arrived_since_read = true;
    }

    /// The newest `n` samples, ending at a cursor that **advances with the
    /// clock** (GUI thread), oldest first.
    ///
    /// **This is the stutter fix, and the ring alone cannot do it.** Audio does
    /// not reach the tap one sample at a time. `TapSource::next` is called from
    /// inside rodio's mixer pull, and the mixer is pulled by cpal's output
    /// callback, which asks for a whole `buffer_size` at a time — 8192 samples,
    /// one call every ~186 ms at the shipped default. So `push` runs 8192 times
    /// in a burst and then not at all for the rest of the period.
    ///
    /// A reader that takes "the newest `n` samples" therefore sees the **same
    /// window** for ~22 frames at 120 fps and then a window of completely
    /// different audio. Measured, that is **0 distinct spectra across 22 idle
    /// frames**: the picture is frozen and then snaps, which is stutter, and no
    /// frame rate removes it because the discontinuity is in the data. (It is
    /// also why the judder got worse as the display got faster: a 186 ms hole
    /// is 11 frames at 60 Hz, where it hides under the eye's persistence, and
    /// 22 at 120 Hz, where it does not.)
    ///
    /// So the window ends at a **cursor** that advances by `rate * elapsed`
    /// rather than jumping to the newest sample. A burst then feeds the cursor
    /// over the frames that follow it, each frame reading a slightly later slice
    /// of the audio, and the spectrum moves continuously. `dt` is clamped for the
    /// same reason `feedback_for_a_dt` clamps it: a gap that is not a frame must
    /// not be spent as one, and a burst that arrives late must still be consumed.
    pub fn read_window(&self, n: usize, dt: f32) -> Vec<f32> {
        let mut state = self.lock();
        Self::read_locked(&mut state, n, dt, true).0
    }

    /// Read the window **without** advancing the cursor, and report whether the
    /// cursor has moved since the last advancing read.
    ///
    /// **This exists because `read_window` had a side effect the app could not
    /// see, and it cost the spectrogram its whole picture.** Every view calls
    /// `compute_bands` *and* `compute_wave` (inside `Uniforms::pack`), so there
    /// are two reads per frame. The second one consumed the arrival flag, and the
    /// two feedback views — the only two that call `take_arrival`, and so the only
    /// two whose picture depends on it — read nothing left. Trails kept its
    /// previous frame; the spectrogram, which has no history of its own to fall
    /// back on, showed nothing at all. A second read was never a second
    /// *arrival*, so it must not move the cursor.
    /// Move the cursor on without reading a window.
    ///
    /// **This exists for `wave`, and it is the view's whole reason for existing.**
    /// Every other view advances the cursor as a side effect of `compute_bands`,
    /// which it needs anyway. `wave` is the only view with no `compute_bands` call
    /// — it is a function of time, not of frequency — so its cursor stood still
    /// for 44 frames at a time (one ring's worth of audio), and the envelope it
    /// drew was a frozen picture of one burst rather than a moving waveform. The
    /// `wave` shader is correct for whatever window it is handed, which is exactly
    /// why nothing in it was wrong.
    ///
    /// Advancing here rather than in `compute_wave` keeps the one-advancing-read
    /// rule intact: `compute_wave` is called from `Uniforms::pack` for *every*
    /// view, so if it advanced, the ten views that already advanced through
    /// `compute_bands` would each spend the backlog twice per frame — the
    /// spectrogram's blank again, by another route.
    pub fn advance(&self, dt: f32) {
        let mut state = self.lock();
        Self::read_locked(&mut state, 0, dt, true);
    }

    pub fn read_window_peek(&self, n: usize) -> Vec<f32> {
        let mut state = self.lock();
        Self::read_locked(&mut state, n, 0.0, false).0
    }

    fn read_locked(state: &mut VizState, n: usize, dt: f32, advance: bool) -> (Vec<f32>, bool) {
        let before = state.cursor;

        // **The cursor is the newest sample the reader has looked at, and it is
        // allowed to trail `written` by more than a window** — that trail is the
        // backlog, and consuming it a little per frame is the entire fix. So the
        // only thing that forces a jump is the trail outgrowing the ring, which
        // is a seek or a track change rather than a burst.
        let oldest = state.written.saturating_sub(state.buf.len() as u64);
        // **The re-anchor is not gated on `advance`, and that is the fix for the
        // wave view.** A cursor behind the ring is not a lagging cursor, it is an
        // invalid index — the samples it names are gone — so repairing it is not
        // part of consuming the backlog. `wave` is the only view with no
        // `compute_bands` call, so it is the only one whose cursor never advances;
        // with the anchor inside `if advance` its cursor sat wherever `clear` left
        // it while the ring turned over under it, and after one ring's worth of
        // audio (~372 ms) every peek read an empty window. An empty wave envelope
        // and a subtraction wrap are the same bug, and the second one only showed
        // up in a debug build.
        //
        // An advancing read is unaffected: its own arm below reaches the same
        // place, and a read that has never run anchors at `written` either way.
        if state.cursor < oldest {
            state.cursor = state.written;
        }
        if advance {
            if !state.started {
                // The first read has no backlog to walk and possibly no clock to
                // spend, so start at the newest sample and mark the buffer read.
                state.started = true;
                state.cursor = state.written;
            }
            if dt > 0.0 {
                let want = (state.rate as f32 * dt.min(Self::MAX_CATCHUP_SECS)) as u64;
                state.cursor = state.cursor.saturating_add(want).min(state.written);
            }
        }
        // **Any read folds a pending arrival into the flag, and only ever ORs.**
        // Two rules, both learned the hard way:
        //
        // - *Any* read, not just an advancing one. A `push` between two reads in
        //   one frame is still an arrival, and only an advancing read folding it
        //   in meant a frame whose reads were all peeks reported nothing — which
        //   is the spectrogram's blank again, by a different route.
        // - *OR*, never assign. Assigning is what broke it outright: a second
        //   read in the same frame overwrote a true with a false.
        state.advanced = state.advanced || state.arrived_since_read || state.cursor != before;
        state.arrived_since_read = false;

        // The `n` samples ending at `cursor`. Ring offsets count back from the
        // newest sample, so the cursor maps to `cursor - oldest` and the window is
        // the `n` before it. A cursor nearer the oldest than `n` yields a short
        // window, which the FFT caller treats as "not enough data yet".
        //
        // **`saturating_sub`, and the peek-only view is why.** The re-anchor above
        // is inside `if advance`, so a view that only ever peeks never re-anchors.
        // `wave` is the one: it is the only view with no `compute_bands` call, so
        // its cursor sits at whatever `clear` left there and the ring turns over
        // underneath it — after one ring's worth of audio (~372 ms at 44.1 kHz)
        // `cursor < oldest`, and unsigned subtraction panics in a debug build. The
        // `.min(len)` is then what keeps a release build honest: a wrapping
        // subtraction lands near 2^64 and only this caps the window at the ring.
        let len = state.buf.len();
        let end = (state.cursor.saturating_sub(oldest) as usize).min(len);
        let start = end.saturating_sub(n);
        let window = state
            .buf
            .iter()
            .skip(start)
            .take(end - start)
            .copied()
            .collect();
        (window, state.advanced)
    }

    /// The longest span a single `read_window` will consume, so one late frame
    /// cannot spend the whole backlog in one step.
    const MAX_CATCHUP_SECS: f32 = 0.25;

    /// Did the read cursor move since the last caller asked? **Consumes the
    /// answer**, so one arrival is one frame's worth of history rather than one
    /// per viewer.
    ///
    /// This is the whole of "the visualizer pauses with the song", and it is here
    /// rather than in the pane because the pane cannot see the audio thread. A
    /// view holds a *history*, and a history of nothing is not a picture — while
    /// paused nothing is pushed, the cursor has nothing left to consume, and it
    /// stops moving.
    pub fn take_arrival(&self) -> bool {
        std::mem::replace(&mut self.lock().advanced, false)
    }

    /// Clear the buffer (e.g. on track load/stop).
    pub fn clear(&self) {
        let mut state = self.lock();
        state.buf.clear();
        state.written = 0;
        state.cursor = 0;
        state.advanced = false;
        state.arrived_since_read = false;
        state.started = false;
    }

    /// The shared state, absorbing a poisoned lock rather than unwrapping it:
    /// every holder here is a leaf that cannot panic while the guard is live, so
    /// a poisoned buffer would otherwise be the one thing that escalates a
    /// recovered panic into an abort — and `push` runs 44.1k times a second on
    /// the audio thread.
    fn lock(&self) -> std::sync::MutexGuard<'_, VizState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Tap source — wraps an f32 source, mono-downmixes, pushes into the buffer, and
/// forwards every sample untouched. Sits after `EqSource` so the viz reflects
/// exactly what is heard (post-EQ; volume and balance are applied at the Sink).
pub struct TapSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    buf: VizBuf,
    channels: u16,
    acc: f32,
    acc_count: u16,
}

impl<S> TapSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, buf: VizBuf) -> Self {
        // rodio 0.22 types both as `NonZero`; the ring buffer's own frame count
        // and rate are plain integers, so unwrap them at the boundary.
        let channels = inner.channels().get();
        // The band→bin mapping needs the real rate; a source is the only thing
        // that knows it.
        buf.set_rate(inner.sample_rate().get());
        Self {
            inner,
            buf,
            channels,
            acc: 0.0,
            acc_count: 0,
        }
    }
}

impl<S> Iterator for TapSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        // `inspect` for the downmix side effect; the sample itself still passes
        // through unchanged.
        self.inner.next().inspect(|&sample| {
            // Mono downmix: accumulate across channels, push when frame complete.
            self.acc += sample;
            self.acc_count += 1;
            if self.acc_count == self.channels {
                let mono = self.acc / self.channels as f32;
                self.buf.push(mono);
                self.acc = 0.0;
                self.acc_count = 0;
            }
        })
    }
}

impl<S> Source for TapSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        self.inner.total_duration()
    }

    /// Forward the seek to the decoder. The viz buffer is intentionally not
    /// cleared — same as the slow-path seek — the ~100 ms window just rolls on.
    fn try_seek(&mut self, pos: std::time::Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

/// The FFT's Hann window: a constant, so it is built once rather than 1024
/// cosines per frame (60k/second, for a function of two compile-time constants).
static HANN: LazyLock<[f32; FFT_SIZE]> = LazyLock::new(|| {
    let mut w = [0.0f32; FFT_SIZE];
    for (i, win) in w.iter_mut().enumerate() {
        *win = 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_SIZE - 1) as f32).cos());
    }
    w
});

/// Radix-2 Cooley-Tukey FFT, in-place, decimation-in-time.
/// Input is interleaved real/imaginary (len must be 2 * power_of_two).
/// Returns magnitude spectrum (first n/2 bins, DC included).
pub fn fft_magnitude(input: &mut [f32]) -> Vec<f32> {
    let n_complex = input.len() / 2;
    assert!(n_complex.is_power_of_two());
    let n = n_complex;

    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            input.swap(2 * i, 2 * j);
            input.swap(2 * i + 1, 2 * j + 1);
        }
    }

    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let wlen = ang.cos();
        let wlen_im = ang.sin();
        for i in (0..n).step_by(len) {
            let mut w = 1.0;
            let mut w_im = 0.0;
            for j in 0..len / 2 {
                let idx_u = 2 * (i + j);
                let idx_v = 2 * (i + j + len / 2);

                let u_re = input[idx_u];
                let u_im = input[idx_u + 1];
                let v_re_in = input[idx_v];
                let v_im_in = input[idx_v + 1];

                // v = v * w (complex multiplication)
                let v_re = v_re_in * w - v_im_in * w_im;
                let v_im = v_re_in * w_im + v_im_in * w;

                input[idx_u] = u_re + v_re;
                input[idx_u + 1] = u_im + v_im;
                input[idx_v] = u_re - v_re;
                input[idx_v + 1] = u_im - v_im;

                let nw = w * wlen - w_im * wlen_im;
                w_im = w * wlen_im + w_im * wlen;
                w = nw;
            }
        }
        len <<= 1;
    }

    let half = n / 2;
    let mut mag = Vec::with_capacity(half);
    for i in 0..half {
        let re = input[i * 2];
        let im = input[i * 2 + 1];
        mag.push((re * re + im * im).sqrt());
    }
    mag
}

/// Compute log-spaced band magnitudes from the most recent audio samples.
/// Returns `VIZ_BANDS` values in dB (0 dB = full scale), smoothed externally.
/// The per-frame fraction of a one-pole smoother, given its time constant.
///
/// **The band smoothing is the last per-frame coefficient in this file, and it is
/// wrong for the same reason `feedback_for_a_dt` and `PLATE_TAU` were.** A fixed
/// fraction per frame is a fixed fraction per frame *whatever the frame rate*: at
/// 60 fps an attack of `0.3` closes in about 47 ms, and on a 120 Hz display the
/// same `0.3` closes in about 28 ms — half the smoothing, on the same music. It
/// is not a small difference here: a percussive spectrum moves a band by ~10 dB
/// between consecutive frames, which at this pane's height is ~57 px of bar
/// travel, so the attack is the only thing standing between that and a juddering
/// picture. Measured on a 120 Hz output, the coefficients really were smoothing
/// at half their intended strength.
///
/// The time constant is what the view means ("a 47 ms attack"), so that is what a
/// view now writes down, and the fraction is derived per frame from `dt`. **At 60
/// Hz this reproduces the old numbers to within a rounding error**, so a 60 Hz
/// display is unchanged and only the higher-refresh case moves — the property that
/// makes this safe to do without being able to look at it.
///
/// Both degenerate inputs are guarded, for the reason
/// [`feedback_for_a_dt`](crate::audio::viz::feedback_for_a_dt) gives: a clock that
/// cannot be trusted must not move anything, and a zero time constant is not
/// smoothing at all rather than a division by zero.
pub fn smoothing_for_a_dt(dt: f32, tau_ms: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if tau_ms <= 0.0 {
        return 1.0;
    }
    1.0 - (-(dt * 1000.0) / tau_ms).exp()
}

/// `attack_ms` / `release_ms` are **time constants in milliseconds**, not per-frame
/// fractions — see [`smoothing_for_a_dt`] for why, and for the `dt` clock's own
/// caveat.
pub fn compute_bands(
    viz: &VizBuf,
    prev: &mut [f32; VIZ_BANDS],
    dt: f32,
    attack_ms: f32,
    release_ms: f32,
) {
    let attack = smoothing_for_a_dt(dt, attack_ms);
    let release = smoothing_for_a_dt(dt, release_ms);
    // One FFT window, ending at the read cursor so the window slides rather than
    // jumping a whole audio burst at a time.
    let mut samples = viz.read_window(FFT_SIZE, dt);
    if samples.len() < FFT_SIZE {
        // Not enough data yet — decay previous values toward -60 dB (noise floor)
        for v in prev.iter_mut() {
            *v = *v * release + DB_FLOOR * (1.0 - release);
        }
        return;
    }

    let window = &*HANN;
    for (s, w) in samples.iter_mut().zip(window.iter()) {
        *s *= *w;
    }

    let mut fft_input = vec![0.0f32; FFT_SIZE * 2];
    for i in 0..FFT_SIZE {
        fft_input[i * 2] = samples[i];
        fft_input[i * 2 + 1] = 0.0;
    }
    let mag = fft_magnitude(&mut fft_input);

    // The Hann window's coherent gain is 0.5, so a full-scale sine peaks at
    // FFT_SIZE/4: normalize so 1.0 input amplitude → 1.0 magnitude (0 dB).
    let norm = 4.0 / FFT_SIZE as f32;

    // Log-spaced band averaging (skip DC, start at bin 1)
    let max_bin = mag.len() - 1;
    // 20 Hz .. Nyquist of the *source's* rate, not an assumed 44.1 kHz — the
    // hardcoded version put every band edge ~9% off on a 48 kHz file.
    let rate = viz.sample_rate() as f32;
    let log_min = 20.0f32.ln();
    let log_max = (rate / 2.0).ln();
    for (b, prev_b) in prev.iter_mut().enumerate() {
        let frac_lo = b as f32 / VIZ_BANDS as f32;
        let frac_hi = (b + 1) as f32 / VIZ_BANDS as f32;
        let f_lo = (log_min + frac_lo * (log_max - log_min)).exp();
        let f_hi = (log_min + frac_hi * (log_max - log_min)).exp();

        let bin_lo = (f_lo * FFT_SIZE as f32 / rate).round() as usize;
        let bin_hi = (f_hi * FFT_SIZE as f32 / rate).round() as usize;
        let lo = bin_lo.max(1).min(max_bin);
        let hi = bin_hi.max(lo + 1).min(max_bin);

        let sum: f32 = mag[lo..hi].iter().sum();
        let count = (hi - lo) as f32;
        let avg = if count > 0.0 { sum / count } else { 0.0 };

        // Convert to dB (full scale reference = 1.0 after normalization)
        let db: f32 = 20.0 * (avg * norm + 1e-10).log10();
        let clamped = db.clamp(DB_FLOOR, 0.0);

        // Attack/release smoothing
        if clamped > *prev_b {
            *prev_b += (clamped - *prev_b) * attack;
        } else {
            *prev_b += (clamped - *prev_b) * release;
        }
    }
}

// The loudness measurement this module used to carry, gone with the VU meter.
//
// `(rms, peak)` over the ring buffer's last window was the one input nothing
// else in the pane reads: a spectrum describes *shape* and this describes
// *amount*, which is why the VU meter existed, and why removing it is a real
// loss of a capability rather than a tidying. What is left is the argument for
// bringing it back rather than for keeping it dormant: **a function no caller
// reaches is not a capability, it is a per-frame pass waiting for a mistake.** The
// only caller ran once a frame over 4 096 samples to fill a uniform that the
// deleted view was the sole reader of, so keeping it meant keeping the cost
// wired to nothing. A loudness view wants this back, and wants it back with its
// own tests — `compute_level_on_silence_is_zero_not_nan` and the two beside it
// were the specification, and they are in this file's history.

/// The highest mode number drawn, and so the densest nodal figure the plate can
/// resolve. Not a taste call: a nodal figure at mode 30 packs ~3 oscillations
/// into a plate's half-width, so the lines land in the wrong place.
///
/// **A bound on the drawing, and never on the hearing.** It is the mode *number*
/// that is capped — how many nodal lines fit on the plate — and not which bands
/// may drive it. Those were the same number for this view's whole life, and that
/// is the single worst bug it has had, because the bands are **log-spaced over
/// 20 Hz .. 22 kHz**: band 10 is 222 Hz. Capping the band index at 10 therefore
/// left the figure able to see **25 Hz .. 222 Hz — 1% of the spectrum** — and
/// every vocal, melody, snare and cymbal in the music did nothing to it whatsoever.
/// Reported as "the nodes move too little and are unrelated to the song", and it
/// was a bass-only meter wearing a cymatic figure. One band of bass and one band
/// of vocal produce *identical* mode pairs.
///
/// `mode_now` now reads all [`VIZ_BANDS`] and maps them into `1..=MAX_MODE`, so
/// the cap bounds the drawing and the whole spectrum reaches it.
pub const MAX_MODE: usize = 10;
/// Where the readable bands split between the two mode numbers, and the reason
/// `mode_now` cannot produce the degenerate figure.
///
/// **Two disjoint groups, not one group read twice.** `n == m` cancels the field
/// to nothing, so the pair has to stay apart; taking two barycentres over
/// overlapping bands would have to *check* that and would fail it whenever the
/// spectrum centred, and the check is one more thing to get wrong. Halving the
/// range makes `m - n` a difference of two quantities that cannot coincide, so
/// the separation is structural.
const MODE_SPLIT: usize = MAX_MODE / 2;
/// Lowest and highest band a mode may be read from, and the low/high divide.
///
/// **The whole spectrum, which is the fix.** Band 0 is excluded because a mode of
/// 0 makes the whole field term zero — a blank figure rather than a coarser one.
/// Everything above it is fair game now, so `m` is driven by the same cymbals
/// and air that make a track sound bright.
const MODE_BAND_LO: usize = 1;
const MODE_BAND_HI: usize = VIZ_BANDS - 1;
/// The last band of the low group; the high group starts at the next one.
const MODE_BAND_SPLIT: usize = MODE_BAND_LO + (MODE_BAND_HI - MODE_BAND_LO + 1).div_ceil(2);
const _: () = assert!(
    MODE_BAND_LO >= 1
        && MODE_SPLIT >= 1
        && MODE_BAND_SPLIT > MODE_BAND_LO
        && MODE_BAND_HI >= MODE_BAND_SPLIT
);

/// How long the plate takes to follow a change in the music, in **seconds**.
///
/// A *time constant*, not a per-frame fraction, and not a hold: the read is
/// continuous so there is nothing to strobe against, and what is left to reject
/// is frame-to-frame FFT noise. Quarter of a second is the smallest value that
/// clears the measured noise floor by an order of magnitude while passing a 1 Hz
/// change essentially unattenuated, and it is well clear of the ~50 ms a bar or a
/// beat takes to be worth reacting to.
///
/// **Seconds, and it would not be a duration in frames.** The same trap
/// `feedback_for_a_dt` and the deleted `HOLD_SECS` both existed for: a
/// per-frame coefficient is a different filter at every frame rate, so the same
/// build would settle differently on a 60 Hz and a 144 Hz display.
const PLATE_TAU: f32 = 0.25;

/// How sharply a band's level is raised to a power before it counts toward the
/// mode read. **The responsiveness dial, and the only one in this view.**
///
/// A barycentre weights every band by its energy, which is a *centroid* — and a
/// centroid of a bass-heavy spectrum is anchored. Measured on the committed
/// `lena.mp3`: **93% of its energy sits below 741 Hz**, so the low group's
/// barycentre wanders a fifth of its range no matter what the music does. That is
/// not a bug in the arithmetic, it is what a centroid *is*, and it is the second
/// half of "the nodes move too little".
///
/// Raising the power walks the read from centroid towards **soft argmax**: the
/// loudest band dominates and the figure follows the peak, which is what actually
/// moves. Combined travel of the two mode numbers over the 12 s fixture, swept:
///
/// ```text
/// power   1     2     3     4     6     8    12    16
/// modes  3.05  3.20  3.45  3.68  4.03  4.24  4.43  4.49
/// ```
///
/// **8, and the tail is why.** Everything past it is under 5% of the range for a
/// read that is by then very nearly an argmax — which is the *old* discrete pick,
/// and the reason this view was a slideshow is that a discrete pick has to be
/// rate-limited to be watchable. The read stays continuous in *level* at any
/// power (a trade between two bands slides the figure continuously, which
/// `the_read_is_continuous_through_a_handover` pins), but the further towards an
/// argmax it walks, the more of the figure's movement is a band switching places
/// and the less of it is the music's *shape*. 8 buys a third more travel than the
/// original 2 and stops short of that.
const MODE_POWER: f32 = 8.0;

/// The plate's mode numbers this frame, read straight off the band levels.
///
/// **Continuous in, continuous out — and that is the entire fix.** This replaced a
/// discrete hysteretic argmax (`pick_mode`) with a 1.5 s hold, and the pair it
/// returned was the *only* channel the music had into the picture. Because the
/// pick was discrete, a discrete pick's failure modes applied to it: two bands
/// near-equal at the top repainted the whole plate, so it needed a margin and a
/// hold, and the hold capped the figure at one change per 1.5 s by construction
/// (measured: a hard 0.67/s even with two bands deliberately duelling). The image
/// was therefore bit-identical between switches and the spectrum could not reach
/// it — reported as "the viz is rendering at 5fps" and as "kinda still and
/// unrelated to the music".
///
/// Neither symptom was about drawing. The field is continuous in both mode
/// numbers, so any real `(n, m)` is a valid cymatic figure, and a *continuous
/// read* of the spectrum needs no margin, no hold, and no anti-strobe machinery
/// at all: a figure that slides cannot strobe, and this deleted the slide, the
/// hold countdown and the stored state along with the pick. Staleness is
/// prevented by the only thing that can actually cause it — the band smoothing
/// above, which is per-bin attack/release.
///
/// Two barycentres, one per half of the eligible range, so `n < m` always and the
/// degenerate `n == m` figure is unreachable rather than merely unlikely (see
/// [`MODE_SPLIT`]). Band 0 is excluded for the same reason `pick_mode` excluded
/// it: a mode of 0 zeroes the whole field term, so it can only draw a blank.
///
/// `prev` is followed over `dt` rather than replaced, and **that is as load-bearing
/// as the continuity is.** A barycentre is a *ratio* of two sums over the band
/// levels, so a hundredth of a dB of FFT noise in one bin moves it several times
/// more than it moves that bin — and weighting by squared energy, which is what
/// makes the read track peaks, widens that a third time. Measured through the
/// real `draw` at 60 fps, the *unsmoothed* read moved the plate by 0.046 of a
/// mode per frame **on a single steady tone with no change in the music at all**,
/// against 0.064 for music that was actually changing: roughly 70% of the motion
/// was twinkle, not sound. That is a worse failure than the frozen plate it
/// replaced, because a plate that jitters reads as broken rather than calm.
///
/// One pole, one time constant, and the twinkle goes while the music gets
/// through. Measured through the real `draw` at 60 fps over the committed
/// `lena.mp3` fixture, as mean movement per frame: a **held sample** (no change
/// in the "music" at all) moves the plate 0.0027 per frame and takes 28 large
/// steps in 736; the **real recording** moves it 0.0253 and takes 334. So the
/// motion is the music at roughly 10:1 over the noise, and `m` travels 3.6 modes
/// over the 12 s where the noise floor does not move at all. Before the pole those
/// two figures were 0.046 and 0.064 — a third of a mode per frame of pure
/// twinkle, which reads as a broken plate rather than a calm one.
///
/// The clock is `dt`, not a per-frame fraction, for the reason
/// [`crate::gui::panes::visualizer::gpu::feedback_for_a_dt`] exists: a fixed
/// fraction per frame is a different filter at every frame rate, so the same
/// build would settle differently on a 60 Hz and a 144 Hz display.
///
/// Pure, and `Ui`-free, so the whole read is testable without a window — which
/// is the half that used to be untestable behind a `Shape::Callback`.
pub fn mode_now(bands: &[f32; VIZ_BANDS], prev: (f32, f32), dt: f32) -> (f32, f32) {
    // `level()` in GLSL, kept as the same expression because a band is a level in
    // dB and every read of one has to agree about that.
    let level = |d: f32| (d - DB_FLOOR).clamp(0.0, -DB_FLOOR) / -DB_FLOOR;
    // A band's share of its group's energy, raised to `MODE_POWER`. Squaring was
    // the original and it is a *centroid* — anchored by the bass, which holds
    // 93% of a real recording's energy, so the figure barely moved. See
    // `MODE_POWER` for the sweep that chose the number.
    let centre = |range: std::ops::RangeInclusive<usize>| {
        let mut weight = 0.0f32;
        let mut moment = 0.0f32;
        for (i, &db) in bands
            .iter()
            .enumerate()
            .take(*range.end() + 1)
            .skip(*range.start())
        {
            let w = level(db).powf(MODE_POWER);
            weight += w;
            moment += w * i as f32;
        }
        // No energy: no opinion, so the caller keeps what it had.
        (weight > 0.0).then(|| moment / weight)
    };
    // The barycentre comes back in **band** units and has to be mapped into
    // **mode** units, because the two are nothing alike: the bands are log-spaced
    // over 20 Hz .. 22 kHz, while a mode is a count of nodal lines and `MAX_MODE`
    // bounds that count. Affine over each group's own span, so the mapping is
    // continuous and monotonic — a curve in it would put figures at musically
    // meaningless band boundaries.
    //
    // Each group maps onto its own half of `1..=MAX_MODE`, and that is what keeps
    // `n < m` structural rather than checked: `n` cannot reach above `MODE_SPLIT`
    // and `m` cannot reach below it, whatever the spectrum does.
    let n = |band: f32| {
        let (first, last) = (MODE_BAND_LO as f32, MODE_BAND_SPLIT as f32);
        let along = ((band - first) / (last - first)).clamp(0.0, 1.0);
        1.0 + along * (MODE_SPLIT as f32 - 1.0)
    };
    let m = |band: f32| {
        let (first, last) = ((MODE_BAND_SPLIT + 1) as f32, MODE_BAND_HI as f32);
        let along = ((band - first) / (last - first)).clamp(0.0, 1.0);
        // `MAX_MODE - MODE_SPLIT - 1`, not `MAX_MODE - MODE_SPLIT`: the high
        // range is `SPLIT+1 ..= MAX_MODE`, and counting the steps *from* its
        // first value lands the last one on `MAX_MODE + 1`. The range sweep
        // caught it as an out-of-range mode number.
        (MODE_SPLIT + 1) as f32 + along * (MAX_MODE - MODE_SPLIT - 1) as f32
    };
    let target = match (
        centre(MODE_BAND_LO..=MODE_BAND_SPLIT),
        centre((MODE_BAND_SPLIT + 1)..=MODE_BAND_HI),
    ) {
        (Some(nb), Some(mb)) => (n(nb), m(mb)),
        // One half silent and the other not: the silent half keeps the figure it
        // had and the loud half moves on its own, because a bass note with no
        // treble above it still has to change the plate.
        (Some(nb), None) => (n(nb), prev.1),
        (None, Some(mb)) => (prev.0, m(mb)),
        (None, None) => return prev,
    };
    let settle = |now: f32, was: f32| {
        // A degenerate `dt` contributes **no elapsed time**, so the plate holds.
        // That is the same shape as `feedback_for_a_dt`, and the same reason: a
        // clock that cannot be trusted must not be allowed to move anything, and
        // egui's predicted frame time is legitimately zero on a session's first
        // frame. Returning the *target* instead would be a snap, which is the one
        // thing this view is not supposed to do to a figure.
        if !dt.is_finite() || dt <= 0.0 {
            return was;
        }
        let k = 1.0 - (-dt / PLATE_TAU).exp();
        was + (now - was) * k
    };
    (settle(target.0, prev.0), settle(target.1, prev.1))
}

/// Compute a mirrored wave envelope from the ring buffer for the "wave" view.
/// Returns `buckets` values in [0, 1] — the peak |sample| per bucket over a
/// wider time window (~23 ms at 44.1 kHz). One bucket per display column gives
/// the classic WMP mirrored silhouette; the envelope smooths the raw
/// sample-level noise that made point-sampled polygons render as garbage.
///
/// The only caller is the harness, at [`WAVE_BUCKETS`]. The bucket count is a
/// parameter rather than a constant because a uniform array's length is part of
/// its GLSL declaration and the harness interpolates that one.
pub fn compute_wave(viz: &VizBuf, buckets: usize) -> Vec<f32> {
    const WAVE_WINDOW: usize = 1024;
    // **A peek, not an advancing read.** `compute_bands` already advanced the
    // cursor this frame, and `Uniforms::pack` runs both, so advancing here too
    // would spend the backlog twice per frame — which is what took the
    // spectrogram's picture. See `read_window_peek`.
    let samples = viz.read_window_peek(WAVE_WINDOW);
    if samples.is_empty() {
        return vec![0.0; buckets.max(1)];
    }
    let buckets = buckets.max(1);
    let len = samples.len();
    let mut wave = vec![0.0f32; buckets];
    // Even distribution: sample i lands in bucket i*buckets/len, so the last
    // sample fills the last bucket. The old `i / div_ceil(len, buckets)`
    // rounding left the rightmost buckets empty (silent gap at pane edge).
    for (i, &s) in samples.iter().enumerate() {
        let v = s.abs();
        if v.is_finite() {
            let b = (i * buckets / len).min(buckets - 1);
            if v > wave[b] {
                wave[b] = v;
            }
        }
    }
    wave
}
