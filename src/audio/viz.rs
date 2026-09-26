//! Audio visualization — tap source, ring buffer, FFT, and smoothing.
//!
//! Pure logic, no UI, no audio device. The tap wraps the EQ source so it
//! visualizes exactly what the user hears (post-EQ). The ring buffer is
//! capped at ~100 ms (4096 samples at 44.1 kHz). FFT is a hand-rolled
//! radix-2 1024-point transform with Hann window. Smoothing applies classic
//! attack/release per bin for the "WMP bars" feel.

use rodio::Source;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use std::f32::consts::PI;

/// Cap for the ring buffer (mono samples). 4096 @ 44.1 kHz ≈ 93 ms.
pub const VIZ_BUFFER_CAP: usize = 4096;
/// FFT window size — power of two. 1024 gives ~43 Hz bins at 44.1 kHz.
pub const FFT_SIZE: usize = 1024;
/// Number of log-spaced output bands for drawing.
pub const VIZ_BANDS: usize = 32;

/// Shared ring buffer for the tap source → GUI.
/// Lock-free on the audio thread would be ideal; the mutex is uncontended
/// at 44.1 kHz pushes and a single snapshot per frame on the GUI thread.
/// ponytail: if contention ever shows up in profiling, swap for a crossbeam
/// lock-free ring or a double-buffered copy.
#[derive(Debug, Clone, Default)]
pub struct VizBuf {
    buf: Arc<Mutex<VecDeque<f32>>>,
}

impl VizBuf {
    pub fn new() -> Self {
        Self {
            buf: Arc::new(Mutex::new(VecDeque::with_capacity(VIZ_BUFFER_CAP))),
        }
    }

    /// Push one mono sample (audio thread).
    pub fn push(&self, sample: f32) {
        let mut guard = self.buf.lock().unwrap();
        if guard.len() == VIZ_BUFFER_CAP {
            guard.pop_front();
        }
        guard.push_back(sample);
    }

    /// Snapshot the most recent `n` samples (GUI thread).
    /// Returns `Vec<f32>` of length up to `n`, oldest first.
    pub fn snapshot_tail(&self, n: usize) -> Vec<f32> {
        let guard = self.buf.lock().unwrap();
        let len = guard.len().min(n);
        guard.iter().rev().take(len).rev().copied().collect()
    }

    /// Clear the buffer (e.g. on track load/stop).
    pub fn clear(&self) {
        self.buf.lock().unwrap().clear();
    }
}

/// Tap source — wraps an f32 source, mono-downmixes, pushes into the buffer,
/// forwards every sample untouched. Used after `EqSource` so the viz reflects
/// exactly what is heard (post-EQ, post-volume if volume is applied upstream).
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

/// Hann window for the FFT.
fn hann_window() -> [f32; FFT_SIZE] {
    let mut w = [0.0f32; FFT_SIZE];
    for (i, win) in w.iter_mut().enumerate() {
        *win = 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_SIZE - 1) as f32).cos());
    }
    w
}

/// Radix-2 Cooley-Tukey FFT, in-place, decimation-in-time.
/// Input is interleaved real/imaginary (len must be 2 * power_of_two).
/// Returns magnitude spectrum (first n/2 bins, DC included).
pub fn fft_magnitude(input: &mut [f32]) -> Vec<f32> {
    let n_complex = input.len() / 2;
    assert!(n_complex.is_power_of_two());
    let n = n_complex;

    // Bit-reversal permutation (swaps pairs for interleaved storage)
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            // Swap real and imaginary parts together
            input.swap(2 * i, 2 * j);
            input.swap(2 * i + 1, 2 * j + 1);
        }
    }

    // Cooley-Tukey (decimation-in-time, interleaved real/imag storage)
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let wlen = ang.cos();
        let wlen_im = ang.sin();
        for i in (0..n).step_by(len) {
            let mut w = 1.0;
            let mut w_im = 0.0;
            for j in 0..len / 2 {
                // Index calculations for interleaved storage
                let idx_u = 2 * (i + j);
                let idx_v = 2 * (i + j + len / 2);

                let u_re = input[idx_u];
                let u_im = input[idx_u + 1];
                let v_re_in = input[idx_v];
                let v_im_in = input[idx_v + 1];

                // v = v * w (complex multiplication)
                let v_re = v_re_in * w - v_im_in * w_im;
                let v_im = v_re_in * w_im + v_im_in * w;

                // Butterfly
                input[idx_u] = u_re + v_re;
                input[idx_u + 1] = u_im + v_im;
                input[idx_v] = u_re - v_re;
                input[idx_v + 1] = u_im - v_im;

                // w = w * wlen (complex multiplication)
                let nw = w * wlen - w_im * wlen_im;
                w_im = w * wlen_im + w_im * wlen;
                w = nw;
            }
        }
        len <<= 1;
    }

    // Magnitudes (only positive frequencies)
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

    // Hann window
    let window = hann_window();
    for i in 0..FFT_SIZE {
        samples[i] *= window[i];
    }

    // FFT
    let mut fft_input = vec![0.0f32; FFT_SIZE * 2];
    for i in 0..FFT_SIZE {
        fft_input[i * 2] = samples[i];
        fft_input[i * 2 + 1] = 0.0;
    }
    let mag = fft_magnitude(&mut fft_input);

    // Hann window coherent gain = 0.5, so full-scale sine peak magnitude = FFT_SIZE/4
    // Normalize so that 1.0 input amplitude → 1.0 magnitude (0 dB)
    let norm = 4.0 / FFT_SIZE as f32;

    // Log-spaced band averaging (skip DC, start at bin 1)
    let max_bin = mag.len() - 1; // Nyquist
    for (b, prev_b) in prev.iter_mut().enumerate() {
        // Log spacing: 20 Hz .. sample_rate/2
        let f_min: f32 = 20.0;
        let f_max: f32 = 22050.0; // 44.1k/2
        let log_min = f_min.ln();
        let log_max = f_max.ln();
        let frac_lo = b as f32 / VIZ_BANDS as f32;
        let frac_hi = (b + 1) as f32 / VIZ_BANDS as f32;
        let f_lo = (log_min + frac_lo * (log_max - log_min)).exp();
        let f_hi = (log_min + frac_hi * (log_max - log_min)).exp();

        let bin_lo = (f_lo * FFT_SIZE as f32 / 44100.0).round() as usize;
        let bin_hi = (f_hi * FFT_SIZE as f32 / 44100.0).round() as usize;
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
