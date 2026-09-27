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
            *v = *v * release + (-60.0) * (1.0 - release);
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
        let clamped = db.clamp(-60.0, 0.0);

        // Attack/release smoothing
        if clamped > *prev_b {
            *prev_b += (clamped - *prev_b) * attack;
        } else {
            *prev_b += (clamped - *prev_b) * release;
        }
    }
}

/// Overall loudness of the window: `(rms, peak)` in 0..1. The spectrum answers
/// "what is it made of", this answers "how loud" — the pair is what a meter
/// needs, and the ring buffer already holds the window, so it is a second pass
/// over data the tap pushed once.
///
/// RMS is `sqrt(mean(x²))`; peak is the largest `|x|`. Silence is `(0.0, 0.0)`
/// rather than NaN, so a caller can scale by it without a guard.
pub fn compute_level(viz: &VizBuf) -> (f32, f32) {
    /// One ring-buffer window: long enough for the RMS to be steady rather than
    /// sample-locked, and exactly the buffer's own cap.
    const LEVEL_WINDOW: usize = VIZ_BUFFER_CAP;
    let samples = viz.snapshot_tail(LEVEL_WINDOW);
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_squares / samples.len() as f32).sqrt();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    (rms.clamp(0.0, 1.0), peak.clamp(0.0, 1.0))
}

/// The Chladni (cymatic) standing-wave field at `(n, m)` — the term every
/// Chladni figure is built from, for a point at `(gx, gy)` in 0..1.
///
/// Exposed per mode rather than as a whole-figure generator because the mode
/// numbers come from the spectrum at draw time, and because the antisymmetric
/// difference is the part a test can pin: it is zero wherever `gx == gy` and
/// antisymmetric under swapping the two axes.
pub fn chladni_field(n: usize, m: usize, gx: f32, gy: f32) -> f32 {
    let (n, m) = (n.max(1) as f32, m.max(1) as f32);
    let (pi_n_gx, pi_m_gx) = (PI * n * gx, PI * m * gx);
    let (pi_n_gy, pi_m_gy) = (PI * n * gy, PI * m * gy);
    (pi_n_gx.sin() * pi_m_gy.sin() - pi_m_gx.sin() * pi_n_gy.sin()).abs()
}

/// Compute a mirrored wave envelope from the ring buffer for the "wave" view.
/// Returns `buckets` values in [0, 1] — the peak |sample| per bucket over a
/// wider time window (~23 ms at 44.1 kHz). One bucket per display column gives
/// the classic WMP mirrored silhouette; the envelope smooths the raw
/// sample-level noise that made point-sampled polygons render as garbage.
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
