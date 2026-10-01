//! TPlayApp public API — static helpers, constants and pure logic
//! invariants.
//!
//! The pure logic behind `TPlayApp`'s public surface. `TPlayApp::new(&config,
//! mixer)` needs neither an audio device nor an eframe `CreationContext`, so
//! this is not the only headless view of the app — `playback_integration.rs`
//! drives real playback. What is pinned here is what a bare struct cannot
//! reach: the formatting, enum and preset helpers, exercised without a sink.

use std::time::Duration;
use tplay::app::{frame_dt_from, Pane, TPlayApp, VizView};
use tplay::audio::eq::EQ_PRESETS;

#[test]
fn fmt_duration_formats_mm_ss() {
    assert_eq!(TPlayApp::fmt_duration(None), "--:--");
    assert_eq!(TPlayApp::fmt_duration(Some(Duration::ZERO)), "00:00");
    assert_eq!(
        TPlayApp::fmt_duration(Some(Duration::from_secs(1))),
        "00:01"
    );
    assert_eq!(
        TPlayApp::fmt_duration(Some(Duration::from_secs(59))),
        "00:59"
    );
    assert_eq!(
        TPlayApp::fmt_duration(Some(Duration::from_secs(60))),
        "01:00"
    );
    assert_eq!(
        TPlayApp::fmt_duration(Some(Duration::from_secs(65))),
        "01:05"
    );
    assert_eq!(
        TPlayApp::fmt_duration(Some(Duration::from_secs(600))),
        "10:00"
    );
    // Minutes are not rolled into hours: 61:01 for 3661 s is intentional.
    assert_eq!(
        TPlayApp::fmt_duration(Some(Duration::from_secs(3661))),
        "61:01"
    );
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
        [
            Pane::NowPlaying,
            Pane::Playlist,
            Pane::Equalizer,
            Pane::Library,
            Pane::Visualizer,
            Pane::AlbumCover
        ]
    );
}

#[test]
fn viz_view_registry_is_stable_and_uniquely_named() {
    // The pane's selector iterates ALL; names must stay unique and the default
    // view is what an empty config falls back to (see config_persistence).
    assert_eq!(VizView::ALL.len(), 9);
    assert_eq!(
        VizView::ALL,
        [
            VizView::Bars,
            VizView::Wave,
            VizView::Radial,
            VizView::Spectrogram,
            VizView::Flame,
            VizView::Chladni,
            VizView::Nebula,
            VizView::Plasma,
            VizView::Trails,
        ]
    );
    assert_eq!(VizView::default(), VizView::Bars);
    let mut names: Vec<&str> = VizView::ALL.iter().map(|v| v.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        VizView::ALL.len(),
        "view names must be unique (the selector keys off them)"
    );
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

/// **`predicted_dt` is 1/60 in every eframe 0.36 app**, so every time-based
/// smoother in the visualizer is calibrated against a clock that does not tick at
/// the display's rate. The hook in `main.rs` measures the real interval instead;
/// this is the policy that decides what it is willing to believe.
///
/// The rejections are the test. Each one is a case where believing the number
/// would be *worse* than leaving egui's default — which is a claim about
/// consequences, so each one names the failure it prevents.
#[test]
fn the_frame_clock_believes_a_frame_and_rejects_everything_else() {
    // The premise, and it is the whole reason this function exists: a real frame
    // on a 240 Hz display is 4.17 ms, and egui reports 16.67 ms for it.
    assert_eq!(
        frame_dt_from(Some(10.0), 10.0 + 1.0 / 240.0),
        Some((1.0 / 240.0) as f32),
        "a 240 Hz frame must be believed as itself, not as the 16.67 ms egui claims"
    );

    // No previous frame: the first frame of a session has nothing to measure
    // against, and it is the one frame with no picture to compare the result to.
    assert_eq!(
        frame_dt_from(None, 10.0),
        None,
        "with no previous frame there is nothing to measure; believing a default here \
         would set the first frame's smoothing from nothing"
    );

    // A clock that did not move. Reporting 0 would be the worst possible answer:
    // every smoother here reads `dt <= 0` as "hold", so the visualizer would
    // freeze and *stay* frozen.
    assert_eq!(frame_dt_from(Some(10.0), 10.0), None);

    // Going backwards is the same failure wearing a different hat.
    assert_eq!(frame_dt_from(Some(10.0), 9.0), None);

    // A gap too long to be a frame: a window drag, a resume, the first frame
    // after a pipeline compile. Measured at 805 ms in a startup trace. One such
    // delta makes every per-frame smoother snap to its target in a single step.
    assert_eq!(frame_dt_from(Some(10.0), 10.0 + 0.805), None);
    assert_eq!(frame_dt_from(Some(10.0), 10.0 + 5.0), None);

    // Steady state is believed, and monotonically: this is what a 120 Hz trace
    // looks like, and what a 60 Hz one looks like, and both must pass through.
    let mut prev = Some(0.0);
    let mut seen = Vec::new();
    for _ in 0..4 {
        let dt = frame_dt_from(prev, prev.unwrap() + 1.0 / 120.0).expect("a 120 Hz frame");
        seen.push(dt);
        prev = Some(prev.unwrap() + 1.0 / 120.0);
    }
    for dt in &seen {
        assert!(
            (*dt - 1.0 / 120.0).abs() < 1e-6,
            "a steady 120 Hz trace must report 8.33 ms every frame, got {dt}"
        );
    }
}
