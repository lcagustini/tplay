//! Equalizer Source — wraps a rodio Source (f32 samples) and applies 10-band graphic EQ.
//! Gains live in `Arc<Mutex<EqShared>>`, shared with the GUI thread: dragging a slider
//! updates the shared gains and the running source swaps filter coefficients in `next()`
//! — no sink rebuild, no audio restart.

use rodio::Source;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 10 equalizer band frequencies, matching the reference UI labels (20..16K).
pub const EQ_FREQUENCIES: [f32; 10] = [
    20.0, 100.0, 300.0, 600.0, 1000.0, 3000.0, 5000.0, 8000.0, 12000.0, 16000.0,
];

/// Live-controllable EQ state, shared between `TPlayApp` (writer) and `EqSource` (reader).
/// Gains are in dB (-12..12); 0 dB is an exact identity filter.
pub struct EqShared {
    pub gains: [f32; 10],
    pub enabled: bool,
}

/// RBJ peaking EQ filter coefficients.
/// Based on: https://www.w3.org/TR/audio-eq-cookbook/
fn peaking_eq_coeffs(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> (f32, f32, f32, f32, f32) {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;

    // Normalize by a0
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

/// Biquad filter state for one band (Direct Form II transposed)
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(sample_rate: u32, freq: f32, gain_db: f32) -> Self {
        let (b0, b1, b2, a1, a2) = peaking_eq_coeffs(sample_rate as f32, freq, 1.0, gain_db);
        Self { b0, b1, b2, a1, a2, z1: 0.0, z2: 0.0 }
    }

    fn process(&mut self, input: f32) -> f32 {
        let out = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * out + self.z2;
        self.z2 = self.b2 * input - self.a2 * out;
        out
    }
}

/// 10-band graphic equalizer Source wrapper for f32 samples.
/// Chains 10 biquad filters in series, one per band.
pub struct EqSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    shared: Arc<Mutex<EqShared>>,
    sample_rate: u32,
    bands: [Biquad; 10],
    cached_gains: [f32; 10],
    cached_enabled: bool,
}

impl<S> EqSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, shared: Arc<Mutex<EqShared>>) -> Self {
        let sample_rate = inner.sample_rate();
        let (gains, enabled) = {
            let state = shared.lock().unwrap();
            (state.gains, state.enabled)
        };
        let bands = std::array::from_fn(|i| {
            Biquad::new(sample_rate, EQ_FREQUENCIES[i], gains[i])
        });
        Self {
            inner,
            shared,
            sample_rate,
            bands,
            cached_gains: gains,
            cached_enabled: enabled,
        }
    }

    /// Pull the latest gains from the GUI thread. Bands whose gain changed get
    /// fresh coefficients *and* fresh filter state (no click from stale history).
    /// Returns whether the EQ is currently enabled.
    fn refresh(&mut self) -> bool {
        let state = self.shared.lock().unwrap();
        if state.enabled != self.cached_enabled {
            self.cached_enabled = state.enabled;
            self.bands = std::array::from_fn(|i| {
                Biquad::new(self.sample_rate, EQ_FREQUENCIES[i], state.gains[i])
            });
            self.cached_gains = state.gains;
        } else {
            for i in 0..10 {
                if state.gains[i] != self.cached_gains[i] {
                    self.cached_gains[i] = state.gains[i];
                    self.bands[i] = Biquad::new(self.sample_rate, EQ_FREQUENCIES[i], state.gains[i]);
                }
            }
        }
        state.enabled
    }
}

impl<S> Iterator for EqSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if !self.refresh() {
            return Some(sample);
        }
        let mut s = sample;
        for band in &mut self.bands {
            s = band.process(s);
        }
        Some(s)
    }
}

impl<S> Source for EqSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}