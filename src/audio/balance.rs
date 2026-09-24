//! Balance (L/R) Source — wraps a rodio Source (f32 samples) and applies
//! per-channel gain for stereo balance. The balance value lives in an
//! `Arc<RwLock<f32>>` shared with the GUI thread: dragging the slider updates
//! the shared value and the running source applies it in `next()` — no sink
//! rebuild, no audio restart.

use rodio::Source;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Convert a balance value (-1.0 = full left, 0.0 = center, 1.0 = full right)
/// into per-channel gains for a stereo output.
/// Linear balance law: b ≤ 0 → (left=1.0, right=1.0+b), b ≥ 0 → (left=1.0-b, right=1.0).
/// Center (0) = (1.0, 1.0) exact passthrough.
pub fn balance_gains(balance: f32) -> (f32, f32) {
    if balance <= 0.0 {
        (1.0, 1.0 + balance) // left full, right fades
    } else {
        (1.0 - balance, 1.0) // right full, left fades
    }
}

/// Live-controllable balance source wrapper for f32 samples.
/// Reads balance from `Arc<RwLock<f32>>` per audio frame and scales channels.
/// Preserves channel count: mono → stereo (both channels scaled), stereo → first two channels scaled.
pub struct BalanceSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    balance: Arc<RwLock<f32>>,
    cached_balance: f32,
    half_frame: Option<f32>,
}

impl<S> BalanceSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, balance: Arc<RwLock<f32>>) -> Self {
        let cached_balance = *balance.read().unwrap();
        Self {
            inner,
            balance,
            cached_balance,
            half_frame: None,
        }
    }

    /// Pull the latest balance from the GUI thread.
    fn refresh(&mut self) {
        let b = *self.balance.read().unwrap();
        if b != self.cached_balance {
            self.cached_balance = b;
            // Clear any buffered half-frame on balance change to avoid a momentary
            // channel mismatch (mono→stereo or L/R swap mid-frame).
            self.half_frame = None;
        }
    }
}

impl<S> Iterator for BalanceSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        // Read balance periodically (per sample is cheap, RwLock read is fast).
        self.refresh();

        let (left_gain, right_gain) = balance_gains(self.cached_balance);

        // Handle stereo (2 channels) or mono (1 channel) input.
        // Output is always stereo: mono input → both channels get the same sample
        // scaled by the respective gain; stereo input → first two channels scaled.
        match self.inner.channels() {
            1 => {
                // Mono input: emit left then right.
                if let Some(sample) = self.half_frame.take() {
                    return Some(sample * right_gain);
                }
                let sample = self.inner.next()?;
                self.half_frame = Some(sample);
                Some(sample * left_gain)
            }
            2 => {
                // Stereo input: emit L then R.
                if let Some(sample) = self.half_frame.take() {
                    return Some(sample * right_gain);
                }
                let left = self.inner.next()?;
                let right = self.inner.next().unwrap_or(0.0);
                self.half_frame = Some(right);
                Some(left * left_gain)
            }
            _ => {
                // Fallback: treat as mono (first channel only).
                if let Some(sample) = self.half_frame.take() {
                    return Some(sample * right_gain);
                }
                let sample = self.inner.next()?;
                self.half_frame = Some(sample);
                Some(sample * left_gain)
            }
        }
    }
}

impl<S> Source for BalanceSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> u16 {
        2 // BalanceSource always outputs stereo
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// Forward the seek to the decoder; on success drop the half-frame —
    /// stale buffered sample from before the jump point would click at the seek.
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let res = self.inner.try_seek(pos);
        if res.is_ok() {
            self.half_frame = None;
        }
        res
    }
}