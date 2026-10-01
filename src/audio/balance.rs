//! Balance (L/R) Source — wraps a rodio Source (f32 samples) and applies
//! per-channel gain. The value lives in an `Arc<RwLock<f32>>` shared with the GUI
//! thread: dragging the slider updates it and the running source applies it in
//! `next()` — no sink rebuild, no audio restart.

use rodio::Source;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

/// Per-channel gains for a stereo output from a balance value (-1.0 = full
/// left, 0.0 = center, 1.0 = full right).
/// Linear law: b ≤ 0 → (left=1.0, right=1.0+b), b ≥ 0 → (left=1.0-b, right=1.0).
/// Center (0) = (1.0, 1.0) exact passthrough.
pub fn balance_gains(balance: f32) -> (f32, f32) {
    if balance <= 0.0 {
        (1.0, 1.0 + balance) // left full, right fades
    } else {
        (1.0 - balance, 1.0) // right full, left fades
    }
}

/// Live-controllable balance wrapper for f32 samples: reads the
/// `Arc<RwLock<f32>>` per audio frame and scales channels. Channel count is
/// preserved: mono → stereo (both scaled), stereo → first two channels scaled.
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
        let cached_balance = *balance.read().unwrap_or_else(PoisonError::into_inner);
        Self {
            inner,
            balance,
            cached_balance,
            half_frame: None,
        }
    }

    /// Pull the latest balance from the GUI thread.
    ///
    /// Per **sample**, and that is load-bearing rather than incidental: a frame's
    /// two channels are emitted by two `next()` calls, so a balance that changed
    /// between them would scale the left with one gain and the right with the
    /// other. Polling less often than this is what `EqSource` gets away with,
    /// because it has no such split.
    fn refresh(&mut self) {
        let b = *self.balance.read().unwrap_or_else(PoisonError::into_inner);
        if b != self.cached_balance {
            self.cached_balance = b;
            // Drop any buffered half-frame on a balance change, or it mismatches
            // the channel (mono→stereo, or an L/R swap) mid-frame.
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
        self.refresh();

        let (left_gain, right_gain) = balance_gains(self.cached_balance);

        // Stereo (2 channels) or mono (1) input; output is always stereo — mono
        // emits the same sample scaled by each gain, stereo scales its first two
        // channels. The two arms are one arm: a source claiming more than two
        // channels is treated as mono, since only the first two are scaled.
        match self.inner.channels().get() {
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
                // Mono (or unknown layout): emit left then right.
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

    fn channels(&self) -> rodio::ChannelCount {
        // BalanceSource always outputs stereo, and rodio 0.22 types a channel
        // count as `NonZero<u16>` — so the constant has to be wrapped once.
        rodio::ChannelCount::new(2).expect("2 is not zero")
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// Forward the seek to the decoder; on success drop the half-frame — a stale
    /// buffered sample from before the jump point would click at the seek.
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let res = self.inner.try_seek(pos);
        if res.is_ok() {
            self.half_frame = None;
        }
        res
    }
}
