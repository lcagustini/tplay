//! Equalizer logic tests — RBJ coefficients, biquad processing, EqSource behavior.

use tplay::audio::eq::{peaking_eq_coeffs, Biquad, EQ_FREQUENCIES, EqShared};
use std::f32::consts::PI;

#[test]
fn eq_frequencies_match_reference_ui() {
    assert_eq!(EQ_FREQUENCIES, [
        20.0, 100.0, 300.0, 600.0, 1000.0, 3000.0, 5000.0, 8000.0, 12000.0, 16000.0,
    ]);
}

#[test]
fn peaking_eq_zero_gain_is_allpass() {
    // At 0 dB gain, a peaking EQ should be an all-pass filter (unity magnitude at all frequencies).
    // The coefficients are NOT identity (b1 != 0), but the frequency response is flat.
    // We verify by checking that the filter passes a signal unchanged at steady state.
    let mut bq = Biquad::new(44100, 1000.0, 0.0);
    
    // Feed a sine wave and verify output matches input at steady state
    let freq = 1000.0;
    let sr = 44100.0;
    let mut max_diff = 0.0f32;
    for i in 0..1000 {
        let t = i as f32 / sr;
        let input = (2.0 * PI * freq * t).sin();
        let out = bq.process(input);
        max_diff = max_diff.max((out - input).abs());
    }
    // After transient, output should match input (all-pass)
    assert!(max_diff < 1e-4, "max diff {}", max_diff);
}

#[test]
fn peaking_eq_positive_gain_boosts() {
    // Positive gain should have b0 > 1 (boost at center frequency)
    let (b0, _, _, _, _) = peaking_eq_coeffs(44100.0, 1000.0, 1.0, 6.0);
    assert!(b0 > 1.0);
}

#[test]
fn peaking_eq_negative_gain_cuts() {
    // Negative gain should have b0 < 1 (cut at center frequency)
    let (b0, _, _, _, _) = peaking_eq_coeffs(44100.0, 1000.0, 1.0, -6.0);
    assert!(b0 < 1.0);
}

#[test]
fn peaking_eq_symmetry() {
    // +6dB and -6dB should be reciprocal at center frequency
    let (b0_pos, _, _, _, _) = peaking_eq_coeffs(44100.0, 1000.0, 1.0, 6.0);
    let (b0_neg, _, _, _, _) = peaking_eq_coeffs(44100.0, 1000.0, 1.0, -6.0);
    assert!((b0_pos * b0_neg - 1.0).abs() < 1e-4);
}

#[test]
fn biquad_zero_gain_passes_through() {
    let mut bq = Biquad::new(44100, 1000.0, 0.0);
    let input = 0.5f32;
    let output = bq.process(input);
    assert!((output - input).abs() < 1e-6);
}

#[test]
fn biquad_state_persists_across_samples() {
    let mut bq = Biquad::new(44100, 1000.0, 6.0);
    let out1 = bq.process(1.0);
    let out2 = bq.process(0.0);
    let out3 = bq.process(0.0);
    // With boost and zero input, state should decay (ringing)
    assert!(out2 != 0.0 || out3 != 0.0);
}

#[test]
fn biquad_different_frequencies_different_coeffs() {
    let bq1 = Biquad::new(44100, 100.0, 6.0);
    let bq2 = Biquad::new(44100, 1000.0, 6.0);
    assert_ne!(bq1.b0, bq2.b0);
    assert_ne!(bq1.a1, bq2.a1);
}

#[test]
fn biquad_sample_rate_affects_coeffs() {
    let bq1 = Biquad::new(44100, 1000.0, 6.0);
    let bq2 = Biquad::new(48000, 1000.0, 6.0);
    assert_ne!(bq1.b0, bq2.b0);
}

// Frequency response spot checks (magnitude at center frequency)
#[test]
fn biquad_boost_magnitude_at_center() {
    // At center frequency, magnitude should be 10^(gain/20)
    let gain_db = 6.0;
    let expected_mag = 10f32.powf(gain_db / 20.0);
    let mut bq = Biquad::new(44100, 1000.0, gain_db);

    // Feed a sine at center frequency for a few cycles to reach steady state
    let freq = 1000.0;
    let sr = 44100.0;
    let mut max_out = 0.0f32;
    for i in 0..5000 {
        let t = i as f32 / sr;
        let input = (2.0 * PI * freq * t).sin();
        let out = bq.process(input);
        if i >= 1000 {
            max_out = max_out.max(out.abs());
        }
    }
    // Steady-state magnitude should match expected (within tolerance)
    assert!((max_out - expected_mag).abs() < 0.05, "magnitude {} vs expected {}", max_out, expected_mag);
}

#[test]
fn biquad_cut_magnitude_at_center() {
    let gain_db = -6.0;
    let expected_mag = 10f32.powf(gain_db / 20.0);
    let mut bq = Biquad::new(44100, 1000.0, gain_db);

    let freq = 1000.0;
    let sr = 44100.0;
    let mut max_out = 0.0f32;
    // Run more cycles to reach steady state (cut filters can have longer settling)
    for i in 0..5000 {
        let t = i as f32 / sr;
        let input = (2.0 * PI * freq * t).sin();
        let out = bq.process(input);
        // Only measure after first 1000 samples (transient)
        if i >= 1000 {
            max_out = max_out.max(out.abs());
        }
    }
    assert!((max_out - expected_mag).abs() < 0.05, "magnitude {} vs expected {}", max_out, expected_mag);
}

// Coefficient formula verification against known reference (w3.org audio-eq-cookbook)
#[test]
fn peaking_eq_coeffs_match_cookbook() {
    // Reference values from cookbook for: fs=44100, f0=1000, Q=1, gain=6dB
    // w0 = 2*pi*1000/44100 = 0.1427
    // alpha = sin(w0)/(2*Q) = 0.0712
    // A = 10^(6/40) = 1.4125
    // b0 = 1 + alpha*A = 1.1006
    // b1 = -2*cos(w0) = -1.9796
    // b2 = 1 - alpha*A = 0.8994
    // a0 = 1 + alpha/A = 1.0504
    // a1 = -2*cos(w0) = -1.9796
    // a2 = 1 - alpha/A = 0.9496
    // Normalized: b0/a0=1.0478, b1/a0=-1.8847, b2/a0=0.8562, a1/a0=-1.8847, a2/a0=0.9040

    let (b0, b1, b2, a1, a2) = peaking_eq_coeffs(44100.0, 1000.0, 1.0, 6.0);
    assert!((b0 - 1.0478).abs() < 0.001);
    assert!((b1 - (-1.8847)).abs() < 0.001);
    assert!((b2 - 0.8562).abs() < 0.001);
    assert!((a1 - (-1.8847)).abs() < 0.001);
    assert!((a2 - 0.9040).abs() < 0.001);
}