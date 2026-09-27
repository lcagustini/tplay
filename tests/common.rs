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

/// Creates a minimal valid AIFF file (1 second, 8kHz mono 16-bit PCM) carrying
/// `NAME`/`AUTH` text chunks, which lofty maps to title and artist.
///
/// The rate is encoded correctly — 8000 Hz as the 80-bit extended float every
/// AIFF carries — so this fixture is what proves symphonia misreads it, rather
/// than a mistake in the fixture. Hand-built for the same reason as `write_wav`:
/// nothing in this repo can *encode* audio.
pub fn write_aiff(path: &Path, title: &str, artist: &str) {
    let n = 8000usize;
    let samples: Vec<u8> = (0..n).flat_map(|i| (i as i16).to_be_bytes()).collect();

    // IFF chunk: 4-byte id, big-endian size, payload — padded to an even size.
    let mut text = Vec::new();
    for (id, value) in [("NAME", title), ("AUTH", artist)] {
        text.extend_from_slice(id.as_bytes());
        text.extend_from_slice(&(value.len() as u32).to_be_bytes());
        text.extend_from_slice(value.as_bytes());
        if value.len() % 2 == 1 {
            text.push(0);
        }
    }

    let mut body = b"AIFF".to_vec();
    body.extend_from_slice(&text);
    // COMM: channels, frame count, sample width, then the rate as an 80-bit
    // extended float — 8000 Hz = 1.953125 × 2^12.
    body.extend_from_slice(b"COMM");
    body.extend_from_slice(&18u32.to_be_bytes());
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&(n as u32).to_be_bytes());
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&[0x40, 0x0B, 0x7A, 0, 0, 0, 0, 0, 0, 0]);
    // SSND: no offset, no block size, then the samples.
    body.extend_from_slice(b"SSND");
    body.extend_from_slice(&((8 + samples.len()) as u32).to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&samples);

    let mut out = b"FORM".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    fs::write(path, out).unwrap();
}

/// Creates a minimal FLAC file (just headers, no audio frames) for header-parse tests.
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
    data.extend_from_slice(&[0u8; 100]);
    fs::write(path, data).unwrap();
}

/// A minimal ID3v2.3 tag block: `TIT2`/`TPE1`/`TALB` frames, latin1 text.
fn id3v2(title: &str, artist: &str, album: &str) -> Vec<u8> {
    let mut frames = Vec::new();
    for (id, text) in [("TIT2", title), ("TPE1", artist), ("TALB", album)] {
        let mut body = vec![0u8]; // latin1 encoding byte
        body.extend_from_slice(text.as_bytes());
        frames.extend_from_slice(id.as_bytes());
        frames.extend_from_slice(&(body.len() as u32).to_be_bytes()); // v2.3: plain u32
        frames.extend_from_slice(&[0, 0]); // frame flags
        frames.extend_from_slice(&body);
    }
    let mut out = b"ID3".to_vec();
    out.extend_from_slice(&[3, 0, 0]); // version 2.3, no flags
    let n = frames.len() as u32;
    // syncsafe size: 7 bits per byte
    out.extend_from_slice(&[
        ((n >> 21) & 0x7f) as u8,
        ((n >> 14) & 0x7f) as u8,
        ((n >> 7) & 0x7f) as u8,
        (n & 0x7f) as u8,
    ]);
    out.extend_from_slice(&frames);
    out
}

/// `count` silent MPEG-1 Layer III frames (128 kbps, 44.1 kHz, joint stereo),
/// which is 417 bytes each. The frame bodies are zero-filled; only the sync
/// headers matter for tag/duration parsing.
fn mpeg_frames(count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(count * 417);
    for _ in 0..count {
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.resize(out.len() + 413, 0);
    }
    out
}

/// A real MP3 with real ID3v2 tags, ~125 KB. Hand-built because no decoder or
/// encoder dependency exists here and lofty ships no test assets.
///
/// This is the fixture the remote-tag path is verified against: it is the
/// format whose tags live at the *front* of the file, so a byte prefix off a
/// share yields them without downloading the whole track.
pub fn write_tagged_mp3(path: &Path, title: &str, artist: &str, album: &str) -> Vec<u8> {
    let mut bytes = id3v2(title, artist, album);
    bytes.extend(mpeg_frames(300));
    fs::write(path, &bytes).unwrap();
    bytes
}

/// A `TPlayApp` with a software audio device.
///
/// `TPlayApp::new` needs a `Mixer` and nothing else, and `rodio::mixer::mixer`
/// builds one with no hardware. It hands back the `MixerSource` that a real
/// cpal callback would pull from — that is `driver` here, and calling `next()`
/// on it is what advances the sink. This is how rodio's own `Sink` tests work.
///
/// The payoff is that the driver runs as fast as the CPU allows, so a test can
/// "play" three seconds of audio in microseconds and assert on what the app did
/// in response. Nothing here waits on a clock.
///
/// Hermetic by construction: `last_dir` points at an empty temp dir, because
/// `TPlayApp::new` calls `navigate_to` on it and would otherwise walk the
/// user's home directory and start a tag scan over it.
pub struct TestApp {
    pub app: tplay::app::TPlayApp,
    driver: rodio::mixer::MixerSource,
    rate: u32,
}

impl TestApp {
    /// Stereo at 44.1 kHz — the rate the bundled test WAVs are written at, so
    /// a `pump` of N seconds means roughly N seconds of the track.
    pub fn new(name: &str) -> Self {
        let dir = test_dir(name);
        let (mixer, driver) = rodio::mixer::mixer(2, 44_100);
        let config = tplay::config::Config {
            library: tplay::config::LibraryData {
                last_dir: dir.to_string_lossy().into_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            app: tplay::app::TPlayApp::new(&config, mixer),
            driver,
            rate: 44_100,
        }
    }

    /// Pull `secs` worth of samples. This is the only thing that makes the sink
    /// advance — `get_pos`, `empty()` and the `pausable` flag are all applied
    /// from `periodic_access` inside the source chain, which only runs when a
    /// consumer asks for samples.
    pub fn pump(&mut self, secs: f32) {
        for _ in 0..(secs * self.rate as f32) as usize {
            self.driver.next();
        }
    }

    /// Pump until the sink has drained or `max_secs` of audio have gone by.
    ///
    /// Bounded on purpose: a track that never drains (an unreadable file, a
    /// source that stalls) must fail the assertion rather than hang the suite.
    pub fn pump_until_empty(&mut self, max_secs: f32) {
        let step = 0.05;
        let mut elapsed = 0.0;
        while !self.app.is_empty() && elapsed < max_secs {
            self.pump(step);
            elapsed += step;
        }
    }
}

/// Asserts two durations are approximately equal (within 1 second).
pub fn assert_duration_approx(actual: Option<Duration>, expected: Duration, msg: &str) {
    match actual {
        Some(d) => {
            let diff = d.abs_diff(expected);
            assert!(
                diff <= Duration::from_secs(1),
                "{} (got {:?}, expected {:?})",
                msg,
                d,
                expected
            );
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
