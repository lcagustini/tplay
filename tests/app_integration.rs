//! TPlayApp public API — static helpers, constants and pure logic
//! invariants.
//!
//! `TPlayApp::new()` needs an audio output device and an eframe
//! `CreationContext`, so end-to-end playback can't run headless; the pure
//! logic behind the public surface is pinned here instead.

use tplay::app::{EQ_PRESETS, Pane, TPlayApp, VizView};
use std::time::Duration;

#[test]
fn fmt_duration_formats_mm_ss() {
    assert_eq!(TPlayApp::fmt_duration(None), "--:--");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::ZERO)), "00:00");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::from_secs(1))), "00:01");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::from_secs(59))), "00:59");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::from_secs(60))), "01:00");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::from_secs(65))), "01:05");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::from_secs(600))), "10:00");
    // Minutes are not rolled into hours: 61:01 for 3661 s is intentional.
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::from_secs(3661))), "61:01");
}

#[test]
fn format_freq_uses_k_for_kilohertz() {
    assert_eq!(TPlayApp::format_freq(20.0), "20");
    assert_eq!(TPlayApp::format_freq(999.0), "999");
    assert_eq!(TPlayApp::format_freq(1000.0), "1K");
    assert_eq!(TPlayApp::format_freq(5000.0), "5K");
    assert_eq!(TPlayApp::format_freq(16000.0), "16K");
}

#[test]
fn pane_all_is_stable() {
    assert_eq!(Pane::ALL.len(), 6);
    assert_eq!(
        Pane::ALL,
        [Pane::NowPlaying, Pane::Playlist, Pane::Equalizer, Pane::Library, Pane::Visualizer, Pane::AlbumCover]
    );
}

#[test]
fn viz_view_registry_is_stable_and_uniquely_named() {
    // The pane's selector iterates ALL; names must stay unique and the default
    // view is what an empty config falls back to (see config_persistence).
    assert_eq!(VizView::ALL.len(), 2);
    assert_eq!(VizView::ALL, [VizView::Bars, VizView::Wave]);
    assert_eq!(VizView::default(), VizView::Bars);
    let mut names: Vec<&str> = VizView::ALL.iter().map(|v| v.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), VizView::ALL.len(), "view names must be unique (the selector keys off them)");
}

#[test]
fn eq_presets_are_unique_and_flat_first() {
    assert_eq!(EQ_PRESETS[0].0, "Flat");
    assert_eq!(EQ_PRESETS[0].1, [0.0; 10]);
    let mut names: Vec<&str> = EQ_PRESETS.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        EQ_PRESETS.len(),
        "preset names must be unique (the dropdown keys off them)"
    );
}

#[test]
fn probe_duration_reads_wav_length() {
    #[path = "common.rs"]
    mod common;
    let dir = common::test_dir("probe_duration");
    let wav = dir.join("tone.wav");
    common::write_wav(&wav);

    let d = tplay::audio::probe_duration(&wav);
    common::assert_duration_approx(d, Duration::from_secs(1), "symphonia wav duration");

    let _ = std::fs::remove_dir_all(&dir);
}