//! Balance tests — `BalanceSource` refreshes balance from the shared
//! `Arc<RwLock<f32>>` on every sample, so a slider drag mid-playback
//! swaps gains without a sink rebuild. These tests drive a BalanceSource
//! over a synthetic source and mutate the shared state while iterating.

use rodio::buffer::SamplesBuffer;
use rodio::Source;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tplay::audio::balance::{balance_gains, BalanceSource};

/// Minimal infinite-ish f32 source: finite sample buffer the BalanceSource drains.
struct TestSrc {
    sr: u32,
    channels: u16,
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
    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(self.channels).expect("channel count is never zero")
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(self.sr).expect("sample rate is never zero")
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[test]
fn balance_gains_curve_is_correct() {
    // Center = (1.0, 1.0) exact passthrough
    assert_eq!(balance_gains(0.0), (1.0, 1.0));
    // Full left
    assert_eq!(balance_gains(-1.0), (1.0, 0.0));
    // Full right
    assert_eq!(balance_gains(1.0), (0.0, 1.0));
    // Midpoints
    assert_eq!(balance_gains(-0.5), (1.0, 0.5));
    assert_eq!(balance_gains(0.5), (0.5, 1.0));
    // Clamped outside range
    assert_eq!(balance_gains(-2.0), (1.0, -1.0)); // right gain negative (will silence)
    assert_eq!(balance_gains(2.0), (-1.0, 1.0)); // left gain negative
}

#[test]
fn mono_input_becomes_stereo_with_balance_applied() {
    let shared = Arc::new(RwLock::new(0.0f32));
    let src = TestSrc {
        sr: 44100,
        channels: 1,
        samples: vec![1.0; 100],
        pos: 0,
    };
    let out: Vec<f32> = BalanceSource::new(src, Arc::clone(&shared)).collect();

    // At center balance: mono 1.0 → L=1.0, R=1.0 (100 input → 200 output samples)
    assert_eq!(out.len(), 200);
    // Even indices (L) and odd indices (R) should both be 1.0
    for (i, &s) in out.iter().enumerate() {
        assert!((s - 1.0).abs() < 1e-5, "index {i}: got {s}, expected 1.0");
    }
}

#[test]
fn mono_full_left_routes_to_left_only() {
    let shared = Arc::new(RwLock::new(-1.0f32));
    let src = TestSrc {
        sr: 44100,
        channels: 1,
        samples: vec![1.0; 100],
        pos: 0,
    };
    let out: Vec<f32> = BalanceSource::new(src, Arc::clone(&shared)).collect();

    // Full left: L=1.0, R=0.0 (100 input → 200 output samples)
    assert_eq!(out.len(), 200);
    for (i, &s) in out.iter().enumerate() {
        let expected = if i % 2 == 0 { 1.0 } else { 0.0 };
        assert!(
            (s - expected).abs() < 1e-5,
            "index {i}: got {s}, expected {expected}"
        );
    }
}

#[test]
fn mono_full_right_routes_to_right_only() {
    let shared = Arc::new(RwLock::new(1.0f32));
    let src = TestSrc {
        sr: 44100,
        channels: 1,
        samples: vec![1.0; 100],
        pos: 0,
    };
    let out: Vec<f32> = BalanceSource::new(src, Arc::clone(&shared)).collect();

    // Full right: L=0.0, R=1.0 (100 input → 200 output samples)
    assert_eq!(out.len(), 200);
    for (i, &s) in out.iter().enumerate() {
        let expected = if i % 2 == 0 { 0.0 } else { 1.0 };
        assert!(
            (s - expected).abs() < 1e-5,
            "index {i}: got {s}, expected {expected}"
        );
    }
}

#[test]
fn stereo_balance_zero_is_bit_identical_passthrough() {
    // This guards the rodio ChannelVolume +6 dB sum bug:
    // stereo input at balance 0 must emit L,R unchanged (no sum/average).
    let shared = Arc::new(RwLock::new(0.0f32));
    let input_l: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
    let input_r: Vec<f32> = (0..100).map(|i| (i as f32 * 0.01) + 0.5).collect();
    let interleaved: Vec<f32> = input_l
        .iter()
        .zip(&input_r)
        .flat_map(|(&l, &r)| [l, r])
        .collect();

    let buf = SamplesBuffer::new(
        rodio::ChannelCount::new(2).expect("channel count is not zero"),
        rodio::SampleRate::new(44_100).expect("sample rate is not zero"),
        interleaved.clone(),
    );
    let out: Vec<f32> = BalanceSource::new(buf, Arc::clone(&shared)).collect();

    // Output must be bit-identical to input (interleaved L,R)
    assert_eq!(out.len(), interleaved.len());
    for (i, (&a, &b)) in out.iter().zip(interleaved.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-7,
            "bit-transparent at balance 0 failed at {i}: {a} vs {b}"
        );
    }
}

#[test]
fn stereo_balance_scales_channels_independently() {
    let shared = Arc::new(RwLock::new(-0.5f32)); // L=1.0, R=0.5
    let input_l: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
    let input_r: Vec<f32> = (0..100).map(|i| (i as f32 * 0.01) + 0.5).collect();
    let interleaved: Vec<f32> = input_l
        .iter()
        .zip(&input_r)
        .flat_map(|(&l, &r)| [l, r])
        .collect();

    let buf = SamplesBuffer::new(
        rodio::ChannelCount::new(2).expect("channel count is not zero"),
        rodio::SampleRate::new(44_100).expect("sample rate is not zero"),
        interleaved,
    );
    let out: Vec<f32> = BalanceSource::new(buf, Arc::clone(&shared)).collect();

    assert_eq!(out.len(), 200);
    for (i, &s) in out.iter().enumerate() {
        if i % 2 == 0 {
            // Left channel: full gain
            let expected = (i / 2) as f32 * 0.01;
            assert!(
                (s - expected).abs() < 1e-5,
                "L[{}] got {s}, expected {expected}",
                i / 2
            );
        } else {
            // Right channel: 0.5 gain
            let expected = ((i / 2) as f32 * 0.01 + 0.5) * 0.5;
            assert!(
                (s - expected).abs() < 1e-5,
                "R[{}] got {s}, expected {expected}",
                i / 2
            );
        }
    }
}

#[test]
fn balance_change_mid_stream_takes_effect_live() {
    let shared = Arc::new(RwLock::new(0.0f32));
    let src = TestSrc {
        sr: 44100,
        channels: 1,
        samples: vec![1.0; 200],
        pos: 0,
    };
    let mut bal = BalanceSource::new(src, Arc::clone(&shared));

    // First half at center
    let first: Vec<f32> = (0..100).map(|_| bal.next().unwrap()).collect();
    assert!(first.iter().all(|&s| (s - 1.0).abs() < 1e-5));

    // "Drag" the slider: GUI holds the write lock
    *shared.write().unwrap() = -1.0; // full left

    // Second half at full left
    let second: Vec<f32> = (0..100).map(|_| bal.next().unwrap()).collect();
    // Even indices (L) = 1.0, odd indices (R) = 0.0
    for (i, &s) in second.iter().enumerate() {
        let expected = if i % 2 == 0 { 1.0 } else { 0.0 };
        assert!(
            (s - expected).abs() < 1e-5,
            "live change failed at {i}: {s} vs {expected}"
        );
    }
}

#[test]
fn try_seek_forwards_and_clears_half_frame() {
    // Use a stereo SamplesBuffer with index-encoded samples to verify seek landing.
    let interleaved: Vec<f32> = (0..44100 * 2 * 2) // 2 seconds stereo
        .map(|i| i as f32)
        .collect();
    let buf = SamplesBuffer::new(
        rodio::ChannelCount::new(2).expect("channel count is not zero"),
        rodio::SampleRate::new(44_100).expect("sample rate is not zero"),
        interleaved,
    );
    let shared = Arc::new(RwLock::new(0.0f32));
    let mut bal = BalanceSource::new(buf, shared);

    // Consume a bit to build half-frame state
    for _ in 0..10 {
        bal.next().unwrap();
    }

    // Seek to 1 second (sample 44100 in stereo = frame 88200 interleaved)
    bal.try_seek(Duration::from_secs(1)).unwrap();

    // Next output should be the sample at 1s (L=88200, R=88201) scaled by balance 0 = (1,1)
    let l = bal.next().unwrap();
    let r = bal.next().unwrap();
    assert!(
        (l - 88200.0).abs() < 1e-3,
        "seek L landing: got {l}, expected 88200"
    );
    assert!(
        (r - 88201.0).abs() < 1e-3,
        "seek R landing: got {r}, expected 88201"
    );
}
