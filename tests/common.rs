//! Shared test utilities for tplay.
//!
//! Each `tests/<suite>.rs` is its own binary and compiles this whole file, but
//! only uses a subset of helpers — so dead_code fires per-binary even though
//! every helper is used somewhere. Suppressed here on purpose.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Creates a unique temporary directory path for tests (does not create it).
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("tplay-test-{}", std::process::id()))
}

/// Creates a unique test subdirectory and returns its path.
pub fn test_dir(name: &str) -> PathBuf {
    let dir = temp_dir().join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Creates a minimal valid WAV file at the given path (1 second, 8kHz mono PCM).
pub fn write_wav(path: &Path) {
    let rate = 8000u32;
    let n = 8000usize;
    let mut data = Vec::with_capacity(44 + n * 2);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36u32 + (n as u32) * 2).to_le_bytes());
    data.extend_from_slice(b"WAVE");
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&(rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&((n as u32) * 2).to_le_bytes());
    for i in 0..n {
        data.extend_from_slice(&(i as i16).to_le_bytes());
    }
    fs::write(path, data).unwrap();
}

/// Creates a minimal FLAC file (just headers, no audio frames) for seektable testing.
/// This is a valid FLAC with STREAMINFO and PADDING but no audio data.
pub fn write_minimal_flac(path: &Path) {
    let mut data = Vec::new();
    data.extend_from_slice(b"fLaC");
    // STREAMINFO block (type 0, last=0, length=34)
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x22]);
    // STREAMINFO payload (34 bytes)
    data.extend_from_slice(&[0x00; 10]); // min/max block size, min frame size
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // sample rate (0), channels, bits per sample
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // total samples (0)
    data.extend_from_slice(&[0x00; 16]); // MD5
    // PADDING block (type 1, last=1, length=100)
    data.extend_from_slice(&[0x81, 0x00, 0x00, 0x64]);
    data.extend_from_slice(&vec![0u8; 100]);
    fs::write(path, data).unwrap();
}

/// Creates a FLAC file with sample rate set in STREAMINFO for seektable builder.
pub fn write_flac_with_sample_rate(path: &Path, sample_rate: u32, padding_bytes: usize) {
    let mut data = Vec::new();
    data.extend_from_slice(b"fLaC");
    // STREAMINFO block
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x22]);
    data.extend_from_slice(&[0x00; 10]);
    // Sample rate at bytes 10-12 (20 bits)
    let sr_bytes = [
        (sample_rate >> 12) as u8,
        ((sample_rate >> 4) & 0xFF) as u8,
        ((sample_rate & 0xF) << 4) as u8,
    ];
    data[10..13].copy_from_slice(&sr_bytes);
    data.extend_from_slice(&[0x00; 24]); // rest of STREAMINFO
    // PADDING block
    let len = padding_bytes as u32;
    data.extend_from_slice(&[0x81, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
    data.extend_from_slice(&vec![0u8; padding_bytes]);
    fs::write(path, data).unwrap()
}

/// Asserts two durations are approximately equal (within 1 second).
pub fn assert_duration_approx(actual: Option<Duration>, expected: Duration, msg: &str) {
    match actual {
        Some(d) => {
            let diff = d.abs_diff(expected);
            assert!(diff <= Duration::from_secs(1), "{} (got {:?}, expected {:?})", msg, d, expected);
        }
        None => panic!("{} (got None, expected {:?})", msg, expected),
    }
}

/// Gives every node in a DockState a finite layout rect, like a rendered frame does.
/// Never-painted leaves have Rect::NOTHING viewports that serialize as `null`,
/// which serde_json can't parse back. This mirrors what a real session does.
pub fn lay_out<T>(tree: &mut egui_dock::DockState<T>) {
    use eframe::egui::{pos2, vec2, Rect};
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
    for i in 0..tree.surfaces_count() {
        let surface = egui_dock::SurfaceIndex(i);
        for node in tree[surface].iter_mut() {
            node.set_rect(rect);
        }
    }
}