//! Equalizer Source — wraps a rodio Source (f32 samples) and applies 10-band graphic EQ.
//! Gains live in `Arc<RwLock<EqShared>>`, shared with the GUI thread: a slider drag
//! updates the shared gains and the running source swaps filter coefficients in
//! `next()` — no sink rebuild, no audio restart.

use rodio::Source;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

/// 10 equalizer band frequencies, matching the reference UI labels (20..16K).
pub const EQ_FREQUENCIES: [f32; 10] = [
    20.0, 100.0, 300.0, 600.0, 1000.0, 3000.0, 5000.0, 8000.0, 12000.0, 16000.0,
];

/// How many bands there are. Every array sized by the band count is written
/// `[f32; EQ_BANDS]` rather than `[f32; 10]`, so adding an eleventh frequency to
/// `EQ_FREQUENCIES` is a one-line change that the compiler checks everywhere
/// instead of a four-file edit that compiles until a band reads out of bounds.
pub const EQ_BANDS: usize = EQ_FREQUENCIES.len();

/// Equalizer presets — Flat is the reset. A manually tweaked slider
/// switches the selection to Custom (None).
/// Curves follow sfxengine.com/blog/best-equalizer-settings-for-music:
/// Flat ⇐ Flat, Rock ⇐ Rock/Metal, Pop ⇐ V-Shape, Jazz ⇐ Treble Boost,
/// Classical ⇐ gentle V-Shape, Electronic ⇐ Bass Boost, Vocal ⇐ Vocal Enhancement.
pub const EQ_PRESETS: [(&str, [f32; EQ_BANDS]); 7] = [
    ("Flat", [0.0; EQ_BANDS]),
    ("Rock", [2.0, 2.5, 3.0, -1.0, 0.0, 3.0, 2.0, 0.5, 0.5, 0.0]),
    ("Pop", [3.0, 2.5, 1.0, -0.5, -0.5, -1.5, 1.0, 2.0, 2.5, 2.0]),
    ("Jazz", [0.0, 0.5, 0.5, 0.0, 0.5, 1.0, 2.5, 2.0, 1.5, 1.0]),
    (
        "Classical",
        [2.5, 2.0, 0.5, 0.0, -0.5, -1.0, 0.5, 1.5, 2.0, 1.5],
    ),
    (
        "Electronic",
        [4.0, 5.0, -2.0, -1.0, 0.0, 0.0, 0.5, 1.0, 1.0, 0.5],
    ),
    (
        "Vocal",
        [0.0, -2.0, -1.0, 0.0, 0.5, 3.0, 1.5, -1.0, 0.0, 0.0],
    ),
];

/// The preset a gain set matches, or `None` for Custom. Derived, never stored:
/// `gains` is the single source of truth, so a hand-tweaked curve cannot drift
/// out of sync with the name shown beside it.
pub fn preset_for(gains: [f32; EQ_BANDS]) -> Option<&'static str> {
    EQ_PRESETS
        .iter()
        .find(|(_, g)| *g == gains)
        .map(|(n, _)| *n)
}

/// Gain limits in dB, applied by `EqSettings::set_band`.
pub const EQ_GAIN_MIN_DB: f32 = -12.0;
pub const EQ_GAIN_MAX_DB: f32 = 12.0;

/// The app's equalizer settings, owning the handle the audio source reads.
///
/// `EqSource` is built on the *GUI* thread during a track change and read on the
/// *audio* thread every frame while the GUI writes gains as sliders move — so the
/// state has to be an `Arc<RwLock<EqShared>>`. Holding the handle here rather
/// than in `TPlayApp` keeps the presets, clamping and name derivation beside the
/// filters they describe, and gives `TPlayApp`'s two accessors something to
/// delegate to instead of each reaching for the lock.
///
/// Setters report whether the value changed. No caller needs the answer —
/// `flush_config`'s content compare decides writes — but it is what the tests
/// pin, and it costs nothing to keep.
pub struct EqSettings {
    shared: Arc<RwLock<EqShared>>,
}

impl EqSettings {
    pub fn new(enabled: bool, gains: [f32; EQ_BANDS]) -> Self {
        Self {
            shared: Arc::new(RwLock::new(EqShared { gains, enabled })),
        }
    }

    /// A clone of the handle for `EqSource` to hold. Cloned, not lent: the
    /// source outlives any borrow of `self`.
    pub fn shared(&self) -> Arc<RwLock<EqShared>> {
        Arc::clone(&self.shared)
    }

    pub fn enabled(&self) -> bool {
        self.shared
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .enabled
    }

    pub fn gains(&self) -> [f32; EQ_BANDS] {
        self.shared
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .gains
    }

    /// Set one band's gain, clamped to the +/-12 dB the UI offers. An
    /// out-of-range band index is ignored and reports no change.
    pub fn set_band(&self, band: usize, gain_db: f32) -> bool {
        if band >= EQ_FREQUENCIES.len() {
            return false;
        }
        let clamped = gain_db.clamp(EQ_GAIN_MIN_DB, EQ_GAIN_MAX_DB);
        let mut shared = self.shared.write().unwrap_or_else(PoisonError::into_inner);
        if (shared.gains[band] - clamped).abs() < f32::EPSILON {
            return false;
        }
        shared.gains[band] = clamped;
        true
    }

    /// Apply a named preset's gains, or `None` to leave them alone (the
    /// ComboBox's "Custom" row, which is only a label for whatever the user
    /// has already dialled in).
    pub fn set_preset(&self, name: Option<&str>) -> bool {
        let Some(name) = name else { return false };
        let Some((_, gains)) = EQ_PRESETS.iter().find(|(n, _)| *n == name) else {
            return false;
        };
        let mut shared = self.shared.write().unwrap_or_else(PoisonError::into_inner);
        if shared.gains == *gains {
            return false;
        }
        shared.gains = *gains;
        true
    }

    pub fn toggle(&self) {
        let mut shared = self.shared.write().unwrap_or_else(PoisonError::into_inner);
        shared.enabled = !shared.enabled;
    }

    /// The preset the current gains match, or `None` for Custom. The ComboBox
    /// holds a `None` option, so it needs the Option, not the label — and the
    /// label is just this or "Custom", derived by the caller so there is one
    /// derivation of the name rather than two accessors over it.
    pub fn preset(&self) -> Option<&'static str> {
        preset_for(self.gains())
    }
}

/// Live-controllable EQ state, written by `EqSettings` and read by `EqSource`.
/// Gains are in dB (-12..12); 0 dB is an exact identity filter.
#[derive(Debug, Clone, Default)]
pub struct EqShared {
    pub gains: [f32; EQ_BANDS],
    pub enabled: bool,
}

/// RBJ peaking EQ filter coefficients.
/// Based on: https://www.w3.org/TR/audio-eq-cookbook/
pub fn peaking_eq_coeffs(
    sample_rate: f32,
    freq: f32,
    q: f32,
    gain_db: f32,
) -> (f32, f32, f32, f32, f32) {
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
pub struct Biquad {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
    pub z1: f32,
    pub z2: f32,
}

impl Biquad {
    pub fn new(sample_rate: u32, freq: f32, gain_db: f32) -> Self {
        let (b0, b1, b2, a1, a2) = peaking_eq_coeffs(sample_rate as f32, freq, 1.0, gain_db);
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let out = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * out + self.z2;
        self.z2 = self.b2 * input - self.a2 * out;
        out
    }
}

/// How many samples pass between two reads of the shared gains.
///
/// The read is the only reason `refresh` is not free, and it ran once per
/// *sample* — ~88k lock acquisitions a second per sink, against a 10-band
/// filter chain that is the actual work in `next`. 32 samples is ~0.7 ms at
/// 44.1 kHz, far below a frame, so a slider drag still lands within the frame
/// that moved it, and the coefficients swap between samples rather than inside
/// a frame, so the change is bit-identical to a per-sample poll — only later.
///
/// `BalanceSource` deliberately does *not* do this: it holds one channel of a
/// frame between two `next()` calls, so a value that changed in between would
/// scale the left with one gain and the right with the other. There the
/// per-sample read is load-bearing, not an optimisation.
const REFRESH_EVERY: u32 = 32;

/// 10-band graphic EQ Source wrapper for f32 samples: 10 biquad filters in
/// series, one per band.
pub struct EqSource<S>
where
    S: Source<Item = f32>,
{
    inner: S,
    shared: Arc<RwLock<EqShared>>,
    sample_rate: u32,
    bands: [Biquad; EQ_BANDS],
    cached_gains: [f32; EQ_BANDS],
    cached_enabled: bool,
    /// Samples since the last read of `shared`; see `REFRESH_EVERY`.
    since_refresh: u32,
}

impl<S> EqSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, shared: Arc<RwLock<EqShared>>) -> Self {
        // rodio 0.22 types the rate as `NonZero<u32>`; the filter coefficients
        // and this struct's own field are plain `u32`, so unwrap it once here
        // rather than carrying the newtype through the whole chain.
        let sample_rate = inner.sample_rate().get();
        let (gains, enabled) = {
            let state = shared.read().unwrap_or_else(PoisonError::into_inner);
            (state.gains, state.enabled)
        };
        let bands = std::array::from_fn(|i| Biquad::new(sample_rate, EQ_FREQUENCIES[i], gains[i]));
        Self {
            inner,
            shared,
            sample_rate,
            bands,
            cached_gains: gains,
            cached_enabled: enabled,
            since_refresh: REFRESH_EVERY,
        }
    }

    /// Pull the latest gains from the GUI thread. A band whose gain changed gets
    /// fresh coefficients *and* fresh filter state (no click from stale
    /// history). Leaves `cached_enabled` describing the current state, so the
    /// caller can branch on it.
    fn refresh(&mut self) {
        let state = self.shared.read().unwrap_or_else(PoisonError::into_inner);
        if state.enabled != self.cached_enabled {
            self.cached_enabled = state.enabled;
            self.bands = std::array::from_fn(|i| {
                Biquad::new(self.sample_rate, EQ_FREQUENCIES[i], state.gains[i])
            });
            self.cached_gains = state.gains;
        } else {
            // An index loop on purpose: this walks four parallel arrays at once
            // (`state.gains`, `cached_gains`, `bands`, `EQ_FREQUENCIES`), two of
            // them mutated. Clippy's `needless_range_loop` suggests
            // `EQ_FREQUENCIES.iter().enumerate()`, which still needs the index
            // for the other three, so it is no shorter — and a 4-way `zip` reads
            // worse than the parallel-array form it replaces.
            #[allow(clippy::needless_range_loop)]
            for i in 0..EQ_BANDS {
                if state.gains[i] != self.cached_gains[i] {
                    self.cached_gains[i] = state.gains[i];
                    self.bands[i] =
                        Biquad::new(self.sample_rate, EQ_FREQUENCIES[i], state.gains[i]);
                }
            }
        }
    }
}

impl<S> Iterator for EqSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.since_refresh >= REFRESH_EVERY {
            self.since_refresh = 0;
            self.refresh();
        } else {
            self.since_refresh += 1;
        }
        if !self.cached_enabled {
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
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// Forward the seek to the decoder; on success drop the filter history —
    /// stale biquad state from before the jump point would click at the seek.
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let res = self.inner.try_seek(pos);
        if res.is_ok() {
            for band in &mut self.bands {
                band.z1 = 0.0;
                band.z2 = 0.0;
            }
        }
        res
    }
}
