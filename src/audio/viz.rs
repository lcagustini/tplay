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

/// Cap for the ring buffer (mono samples). 4096 @ 44.1 kHz ≈ 93 ms.
pub const VIZ_BUFFER_CAP: usize = 4096;
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
    }

    /// The most recent `n` samples (GUI thread), oldest first, up to `n` long.
    pub fn snapshot_tail(&self, n: usize) -> Vec<f32> {
        let state = self.lock();
        let len = state.buf.len().min(n);
        state.buf.iter().rev().take(len).rev().copied().collect()
    }

    /// Clear the buffer (e.g. on track load/stop).
    pub fn clear(&self) {
        self.lock().buf.clear();
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
        let channels = inner.channels();
        // The band→bin mapping needs the real rate; a source is the only thing
        // that knows it.
        buf.set_rate(inner.sample_rate());
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

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
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
pub fn compute_bands(viz: &VizBuf, prev: &mut [f32; VIZ_BANDS], attack: f32, release: f32) {
    // Snapshot enough for one FFT window
    let mut samples = viz.snapshot_tail(FFT_SIZE);
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

/// The highest band index eligible to be a mode number, and so the highest mode
/// number drawn. Not a taste call: a nodal figure at mode 30 packs ~3 cells per
/// oscillation on a 48-cell plate, so the lines land in the wrong place, and the
/// high bands are single FFT bins of noise with no music in them — so they are
/// the last thing that should be choosing a figure.
pub const MAX_MODE: usize = 10;
/// `pick_mode` seeds two slots from one band, so it needs a second eligible
/// band, and it indexes `bands` directly, so it needs a band that exists.
const _: () = assert!(MAX_MODE >= 2 && MAX_MODE < VIZ_BANDS);

/// How much louder a challenger has to be before the figure changes, in dB.
pub const MARGIN_DB: f32 = 3.0;
/// How long the figure stays pinned after a switch, in **seconds**. The
/// insurance against a genuine two-cycle, which hysteresis always permits, and
/// with the spectrum tracked briskly it is the *only* thing bounding how often
/// the figure can change. It costs no latency: a switch happens the moment the
/// margin is cleared, and only the switch *after* it waits.
///
/// **Seconds, and it used to be a frame count** — `HOLD_FRAMES = 90`, written down
/// as "1.5 s at 60 fps". That is a duration only at one frame rate: on the 120 Hz
/// display this was reported on, it was 0.75 s, and the figure changed at 1.3 Hz
/// against a hold written to mean 0.67 Hz. The measured pass rate came from
/// `gpu::probe`, and it is the reason this is a `f32`.
///
/// The same trap twice already in this repo: `gpu::feedback_for_a_dt` exists
/// because a per-frame multiplier is a fixed fraction per frame, and the VU
/// meter's hold was a clock for the same reason. Both were `dt`-injected and
/// pure; this one was a bare integer in a view, and nothing could see it.
pub const HOLD_SECS: f32 = 1.5;

/// Spend one frame of the hold: `hold` seconds remaining after `dt` has passed.
///
/// The whole fix in one pure function, and pure so it can be tested.
///
/// **A degenerate `dt` spends nothing, and that is the safe direction.** A
/// negative frame time is not a quantity a caller should have to reason about,
/// and an *infinite* one — which `dt.max(0.0)` lets straight through, since
/// infinity is not less than zero — would take `hold - inf` to zero and release
/// the figure immediately. That is the strobe this whole mechanism exists to
/// prevent, reached through the guard rather than around it, so the test for it
/// is not decoration. The shape is deliberately the same as
/// [`crate::gui::panes::visualizer::gpu::feedback_for_a_dt`], which drops its
/// frame for the same reason.
pub fn hold_tick(hold: f32, dt: f32) -> f32 {
    if !dt.is_finite() || dt <= 0.0 {
        return hold.max(0.0);
    }
    (hold - dt).max(0.0)
}

/// Pick the plate's mode pair from the smoothed band levels, sticking to the
/// current one.
///
/// The mode numbers are a **discrete** pick, so a bare top-two repaints the
/// whole plate as a different figure whenever two bands are near-equal at the
/// top — most frames of most music, and every frame of a quiet passage, where
/// all 32 bands sit on the `-60` floor and the order is decided by hundredths of
/// a dB of FFT noise. Measured against the real smoothing constants over a
/// drifting bass line, a bare top-two switched 60 times in 15 s: four
/// whole-figure redraws a second.
///
/// Smoothing cannot fix that, and the cost of it here is not a taste call
/// either — a discrete pick has nothing for a smoother to average. So the figure
/// sticks: keep the current pair unless a challenger is louder by
/// `MARGIN_DB`, and pin it for `hold` seconds after a switch. `hold` is a
/// duration in seconds and the caller owns the decrement — see [`hold_tick`].
///
/// Candidates are band indices `1..=MAX_MODE`, and **the band index is the mode
/// number**. Band 0 is excluded because a mode of 0 makes the whole field term
/// zero, so it can only ever draw a blank figure — a free source of the exact
/// strobing this exists to stop.
pub fn pick_mode(
    bands: &[f32; VIZ_BANDS],
    current: Option<(usize, usize)>,
    hold: f32,
) -> (usize, usize) {
    // The top two eligible bands, in one pass and without allocating: this runs
    // every frame. Seeding both slots with band 1 costs nothing because the
    // second slot is necessarily overwritten by band 2 (MAX_MODE >= 2).
    let mut top: [(usize, f32); 2] = [(1, f32::MIN); 2];
    for (i, &level) in bands.iter().enumerate().skip(1).take(MAX_MODE) {
        if level > top[0].1 {
            top[1] = top[0];
            top[0] = (i, level);
        } else if level > top[1].1 {
            top[1] = (i, level);
        }
    }
    let cand = if top[0].0 > top[1].0 {
        (top[1].0, top[0].0)
    } else {
        (top[0].0, top[1].0)
    };

    let Some((n, m)) = current else { return cand };
    let eligible = |i: usize| (1..=MAX_MODE).contains(&i);
    // A stored pair outside the range, or degenerate, is stale — an edited
    // MAX_MODE, or egui memory that outlived the build that wrote it — and
    // re-picking beats drawing a blank or an unresolvable plate.
    if !eligible(n) || !eligible(m) || n == m {
        return cand;
    }
    if hold > 0.0 || cand == (n, m) {
        return (n, m);
    }
    // The *weaker* band of each pair, not the louder one: a pair is only as
    // loud as its quieter member, so that is the honest comparison to make.
    let weak = |p: (usize, usize)| f32::min(bands[p.0], bands[p.1]);
    if weak(cand) > weak((n, m)) + MARGIN_DB {
        cand
    } else {
        (n, m)
    }
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
    let samples = viz.snapshot_tail(WAVE_WINDOW);
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
