//! Visualizer DSP — ring buffer, FFT, and the two band-mapping functions.
//!
//! Lives here rather than in a `#[cfg(test)] mod tests` inside
//! `src/audio/viz.rs` because the project keeps every test in `tests/` (each
//! file is its own auto-discovered binary, run exactly once). It needs an audio
//! device for nothing: the tap source is covered at Source level in
//! `eq_live.rs`-style suites, and everything here is pure.

use std::f32::consts::PI;
use tplay::audio::viz::{compute_bands, compute_wave, fft_magnitude, VizBuf, FFT_SIZE, VIZ_BANDS};

#[test]
fn viz_buf_push_and_snapshot() {
    let buf = VizBuf::new();
    buf.push(0.5);
    buf.push(-0.25);
    let tail = buf.snapshot_tail(10);
    assert_eq!(tail, vec![0.5, -0.25]);
}

#[test]
fn viz_buf_clear() {
    let buf = VizBuf::new();
    buf.push(1.0);
    buf.clear();
    assert!(buf.snapshot_tail(10).is_empty());
}

#[test]
fn fft_constant_dc() {
    let mut input = [0.0f32; FFT_SIZE * 2];
    for i in 0..FFT_SIZE {
        input[i * 2] = 1.0; // real = 1, imag = 0
    }
    let mag = fft_magnitude(&mut input);
    // DC bin (index 0) should be FFT_SIZE, others ~0
    assert!((mag[0] - FFT_SIZE as f32).abs() < 1.0);
    for m in &mag[1..10] {
        assert!(*m < 1.0);
    }
}

#[test]
fn fft_sine_1khz() {
    // 1 kHz sine at 44.1 kHz, 1024 samples → bin ≈ 1000 * 1024 / 44100 ≈ 23
    let mut input = [0.0f32; FFT_SIZE * 2];
    for i in 0..FFT_SIZE {
        input[i * 2] = (2.0 * PI * 1000.0 * i as f32 / 44100.0).sin();
    }
    let mag = fft_magnitude(&mut input);
    let peak_bin = mag.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
    assert!((peak_bin as i32 - 23).abs() <= 1, "peak at bin {}, expected ~23", peak_bin);
}

#[test]
fn compute_bands_decays_when_empty() {
    let buf = VizBuf::new();
    let mut prev = [0.0f32; VIZ_BANDS];
    compute_bands(&buf, &mut prev, 0.3, 0.95);
    for v in prev {
        assert!(v < 0.0); // decayed from 0 toward -60
    }
}

#[test]
fn compute_wave_empty() {
    let buf = VizBuf::new();
    let wave = compute_wave(&buf, VIZ_BANDS);
    assert_eq!(wave.len(), VIZ_BANDS);
    assert!(wave.iter().all(|&v| v == 0.0));
}

#[test]
fn compute_wave_peak_envelope() {
    let buf = VizBuf::new();
    for &v in &[0.2, -0.8, 0.1, -0.3] {
        buf.push(v);
    }
    // 4 samples, 2 buckets: bucket0 = max(|0.2|, |-0.8|) = 0.8, bucket1 = 0.3
    let wave = compute_wave(&buf, 2);
    assert_eq!(wave.len(), 2);
    assert!((wave[0] - 0.8).abs() < 1e-6);
    assert!((wave[1] - 0.3).abs() < 1e-6);
}

#[test]
fn compute_wave_reaches_last_bucket() {
    // 1024 samples, 350 buckets (a ~700px pane): div_ceil grouping used to
    // leave the rightmost buckets empty, collapsing the wave before the
    // pane edge. Every bucket must now get its share and fill to 1.0.
    let buf = VizBuf::new();
    for _ in 0..1024 {
        buf.push(1.0);
    }
    let wave = compute_wave(&buf, 350);
    assert_eq!(wave.len(), 350);
    assert_eq!(wave[349], 1.0);
    assert!(wave.iter().all(|&v| v == 1.0));
}
