//! Live EQ behavior — `EqSource` refreshes gains from the shared
//! `Arc<RwLock<EqShared>>` on every sample, so a slider drag mid-playback
//! swaps coefficients without a sink rebuild. These tests drive an EqSource
//! over a synthetic source and mutate the shared state while iterating.
//!
//! (eq_tests.rs pins the biquad/coefficient math itself; this file pins the
//! Source-level contract: identity at 0 dB, boost in the time domain, and
//! live reconfiguration + disable.)

use rodio::buffer::SamplesBuffer;
use rodio::Source;
use std::f32::consts::PI;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tplay::audio::eq::{Biquad, EqShared, EqSource, EQ_FREQUENCIES};
use tplay::audio::viz::{TapSource, VizBuf};

/// Minimal infinite-ish f32 source: finite sample buffer the EqSource drains.
struct TestSrc {
    sr: u32,
    samples: Vec<f32>,
    pos: usize,
}

impl Iterator for TestSrc {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for TestSrc {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.samples.len() - self.pos)
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        self.sr
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

fn sine(sr: u32, freq: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (2.0 * PI * freq * i as f32 / sr as f32).sin())
        .collect()
}

fn boost_band_for(freq: f32) -> usize {
    EQ_FREQUENCIES
        .iter()
        .position(|&f| f == freq)
        .expect("known band")
}

#[test]
fn flat_gains_are_exact_identity() {
    // 0 dB on every band → all-pass: the Source passes its input unchanged.
    let shared = Arc::new(RwLock::new(EqShared {
        gains: [0.0; 10],
        enabled: true,
    }));
    let input = sine(44100, 997.0, 2000);
    let src = TestSrc {
        sr: 44100,
        samples: input.clone(),
        pos: 0,
    };
    let out: Vec<f32> = EqSource::new(src, shared).collect();

    let max_diff = input
        .iter()
        .zip(&out)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_diff < 1e-5,
        "EQ must not alter audio at 0 dB (max diff {max_diff})"
    );
}

#[test]
fn boost_band_amplifies_at_center_frequency() {
    let sr = 44100;
    let freq = 1000.0;
    let band = boost_band_for(freq);
    let mut gains = [0.0; 10];
    gains[band] = 12.0; // +12 dB ≈ 3.98× at center
    let shared = Arc::new(RwLock::new(EqShared {
        gains,
        enabled: true,
    }));

    let src = TestSrc {
        sr,
        samples: sine(sr, freq, 20_000),
        pos: 0,
    };
    let out: Vec<f32> = EqSource::new(src, shared).collect();

    // Measure steady state (skip the filter transient).
    let steady = &out[5000..];
    let max = steady.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        max > 2.5,
        "12 dB boost at center must exceed 2.5 amplitude, got {max}"
    );
}

#[test]
fn gain_change_mid_stream_takes_effect_live() {
    let sr = 44100;
    let freq = 1000.0;
    let band = boost_band_for(freq);
    let shared = Arc::new(RwLock::new(EqShared {
        gains: [0.0; 10],
        enabled: true,
    }));

    let src = TestSrc {
        sr,
        samples: sine(sr, freq, 30_000),
        pos: 0,
    };
    let mut eq = EqSource::new(src, Arc::clone(&shared));

    let before: Vec<f32> = (0..2000).map(|_| eq.next().unwrap()).collect();
    // "Drag" the slider: GUI holds the write lock.
    shared.write().unwrap().gains[band] = 12.0;
    let after: Vec<f32> = (0..28_000).map(|_| eq.next().unwrap()).collect();

    // Before the change it was an all-pass; after, steady state is boosted.
    let before_max = before.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        before_max < 1.1,
        "unboosted phase must stay near unity, got {before_max}"
    );

    let steady = &after[20_000..];
    let after_max = steady.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        after_max > 2.5,
        "live gain change must apply without restart ({after_max})"
    );
}

#[test]
fn disabled_eq_passes_audio_through_unfiltered() {
    // enabled=false skips processing entirely, regardless of the gains.
    let mut gains = [0.0; 10];
    gains[boost_band_for(1000.0)] = 50.0; // absurd boost, must be ignored
    let shared = Arc::new(RwLock::new(EqShared {
        gains,
        enabled: false,
    }));

    let input = sine(44100, 1000.0, 5000);
    let src = TestSrc {
        sr: 44100,
        samples: input.clone(),
        pos: 0,
    };
    let out: Vec<f32> = EqSource::new(src, shared).collect();

    let max_diff = input
        .iter()
        .zip(&out)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_diff < 1e-7,
        "disabled EQ must be bit-transparent (max diff {max_diff})"
    );
}

#[test]
fn try_seek_forwards_to_inner_and_resets_filter_state() {
    // The fast seek path (Sink::try_seek) reaches the decoder only if
    // EqSource and TapSource forward try_seek. Wrap an in-memory seekable
    // buffer whose sample values encode their index, jump to 0.5 s, and
    // assert the stream resumes at the mid-file sample.
    let sr = 44100u32;
    let samples: Vec<f32> = (0..sr as usize * 2).map(|i| i as f32).collect();
    let buf = SamplesBuffer::new(2, sr, samples);

    // EQ enabled with a boost so the seek's filter-history reset is observable.
    let shared = Arc::new(RwLock::new(EqShared {
        gains: [12.0; 10],
        enabled: true,
    }));
    let eq = EqSource::new(buf, shared);
    let mut tap = TapSource::new(eq, VizBuf::new());

    // Build filter history over the opening samples, then jump to 0.5 s
    // (sample offset 44100 in interleaved stereo).
    for _ in 0..100 {
        tap.next().unwrap();
    }
    tap.try_seek(Duration::from_millis(500)).unwrap();

    // Expected: the 10-band chain applied to 44100.0 from zeroed state.
    let mut expected = 44100.0f32;
    for freq in EQ_FREQUENCIES {
        let mut fresh = Biquad::new(sr, freq, 12.0);
        expected = fresh.process(expected);
    }
    let first = tap.next().unwrap();
    assert!(
        (first - expected).abs() < 1e-3,
        "seek must land on the mid-file sample with fresh filters: got {first}, expected {expected}"
    );
}

#[test]
fn reenable_after_disabled_applies_gains() {
    let sr = 44100;
    let freq = 1000.0;
    let band = boost_band_for(freq);
    let shared = Arc::new(RwLock::new(EqShared {
        gains: [0.0; 10],
        enabled: false,
    }));

    let src = TestSrc {
        sr,
        samples: sine(sr, freq, 30_000),
        pos: 0,
    };
    let mut eq = EqSource::new(src, Arc::clone(&shared));

    let disabled_phase: Vec<f32> = (0..5000).map(|_| eq.next().unwrap()).collect();
    // Toggle ON (toggle_eq in the app does exactly this write on the shared).
    shared.write().unwrap().gains[band] = 12.0;
    shared.write().unwrap().enabled = true;
    let enabled_phase: Vec<f32> = (0..25_000).map(|_| eq.next().unwrap()).collect();

    let disabled_max = disabled_phase
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    assert!(
        disabled_max <= 1.001,
        "disabled: passthrough, got {disabled_max}"
    );

    let steady = &enabled_phase[20_000..];
    let enabled_max = steady.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        enabled_max > 2.5,
        "reenabling must engage the filters ({enabled_max})"
    );
}
