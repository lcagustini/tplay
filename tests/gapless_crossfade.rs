//! Crossfade/gapless tests — `fade_gains` curve properties (pure) and the
//! shared xf source builder on a real WAV file (both sinks play the same
//! full-track source; the two-sink playback logic itself is headless-boundary,
//! mirrored only where pure).

mod common;

use tplay::audio::transition::{build_gapless_next, fade_gains};
use tplay::audio::eq::{EqShared, EqSource, EQ_FREQUENCIES};
use tplay::audio::viz::{TapSource, VizBuf};
use tplay::audio::balance::{BalanceSource, balance_gains};
use rodio::buffer::SamplesBuffer;
use rodio::Source;
use std::sync::{Arc, RwLock};
use std::time::Duration;

fn dummy_shared() -> (Arc<RwLock<EqShared>>, Arc<RwLock<f32>>, VizBuf) {
    let eq_shared = Arc::new(RwLock::new(EqShared {
        gains: [0.0; 10],
        enabled: true,
    }));
    let balance = Arc::new(RwLock::new(0.0f32));
    let viz = VizBuf::new();
    (eq_shared, balance, viz)
}

/// Constant-power invariant: out² + in² = 1 for every progress point.
#[test]
fn fade_gains_constant_power() {
    for i in 0..=100 {
        let p = i as f32 / 100.0;
        let (out, inc) = fade_gains(p);
        let sum = out * out + inc * inc;
        assert!((sum - 1.0).abs() < 1e-5, "p={p}: out²+in²={sum}");
    }
}

#[test]
fn fade_gains_endpoints() {
    let (o0, i0) = fade_gains(0.0);
    assert!((o0 - 1.0).abs() < 1e-6 && i0.abs() < 1e-6, "p=0 should be (1, 0)");
    let (o1, i1) = fade_gains(1.0);
    assert!(o1.abs() < 1e-6 && (i1 - 1.0).abs() < 1e-6, "p=1 should be (0, 1)");
}

#[test]
fn fade_gains_midpoint_equal_power() {
    let (out, inc) = fade_gains(0.5);
    // cos(π/4) = sin(π/4) ≈ 0.7071
    assert!((out - inc).abs() < 1e-6, "midpoint gains must be equal");
    assert!((out - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn fade_gains_monotonic() {
    let mut prev_out = 1.0f32;
    let mut prev_in = 0.0f32;
    for i in 0..=100 {
        let p = i as f32 / 100.0;
        let (out, inc) = fade_gains(p);
        assert!(out <= prev_out + 1e-6, "out gain must not increase (p={p})");
        assert!(inc >= prev_in - 1e-6, "in gain must not decrease (p={p})");
        prev_out = out;
        prev_in = inc;
    }
}

#[test]
fn fade_gains_clamps_out_of_range() {
    let (o_lo, i_lo) = fade_gains(-3.0);
    assert!((o_lo - 1.0).abs() < 1e-6 && i_lo.abs() < 1e-6);
    let (o_hi, i_hi) = fade_gains(7.0);
    assert!(o_hi.abs() < 1e-6 && (i_hi - 1.0).abs() < 1e-6);
}

/// The xf/gapless builder is a plain buffered full-track source (EQ → Tap →
/// Balance) — decode a real WAV and verify it reports the file's shape.
#[test]
fn build_gapless_next_produces_full_track_source() {
    let dir = common::test_dir("gapless_xf_source");
    let wav = dir.join("a.wav");
    common::write_wav(&wav);
    let (eq_shared, balance, viz) = dummy_shared();

    let mut src = build_gapless_next(wav, eq_shared, balance, viz);
    assert_eq!(src.channels(), 2); // BalanceSource upmixes mono → stereo
    assert_eq!(src.sample_rate(), 8000);
    common::assert_duration_approx(src.total_duration(), Duration::from_secs(1), "wav duration");

    // Not truncated, not a mix — a full track flows through (stereo frames).
    let mut count = 0usize;
    while src.next().is_some() && count < 100_000 {
        count += 1;
    }
    assert!(count >= 15_998, "full track must drain ~16000 stereo samples, got {count}");
}

/// 0 dB EQ + centered balance = bit-transparent passthrough of the decoded file.
#[test]
fn build_gapless_next_identity_when_flags_nominal() {
    let dir = common::test_dir("gapless_xf_identity");
    let wav = dir.join("a.wav");
    common::write_wav(&wav);
    let (eq_shared, balance, viz) = dummy_shared();

    let mut src = build_gapless_next(wav, eq_shared, balance, viz);
    let first = src.next().expect("sample");
    // write_wav's first sample is i16 value 0.
    assert!(first.abs() < 1e-4, "first sample should be silence, got {first}");
    // Pull 4000 stereo frames (8000 samples) — the 4000th frame's mono sample
    // is i16 4000, duplicated to L and R by the balance upmix.
    for _ in 0..4000 * 2 {
        let _ = src.next();
    }
    let s4k = src.next().expect("sample");
    assert!((s4k - 4000.0 / 32768.0).abs() < 5e-3, "frame 4000 should be 4000/32768, got {s4k}");
}

/// The wrapper chain preserves stereo (balance outputs stereo from mono) —
/// exercises the same source stack the xf sink uses.
#[test]
fn source_chain_stereo_output() {
    let (eq_shared, balance, viz) = dummy_shared();
    let interleaved: Vec<f32> = (0..100).flat_map(|i| [i as f32 * 0.01, i as f32 * 0.01 + 0.5]).collect();
    let buf = SamplesBuffer::new(2, 44100, interleaved);
    let eq = EqSource::new(buf, eq_shared);
    let tap = TapSource::new(eq, viz);
    let bal = BalanceSource::new(tap, balance);
    assert_eq!(bal.channels(), 2);
}

#[test]
fn balance_gains_curve() {
    // Center = exact passthrough; hard left/right silence the opposite side.
    let (l, r) = balance_gains(0.0);
    assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    let (l, r) = balance_gains(-1.0);
    assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    let (l, r) = balance_gains(1.0);
    assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
}

/// EQ band frequencies still drive both filters and labels — 10 bands, ascending.
#[test]
fn eq_frequencies_ten_ascending() {
    assert_eq!(EQ_FREQUENCIES.len(), 10);
    assert!(EQ_FREQUENCIES.windows(2).all(|w| w[0] < w[1]));
}