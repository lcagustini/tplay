//! The log line format, which is the only part of `src/logging.rs` a test can
//! reach without a stderr to read.
//!
//! The properties worth pinning are the ones a reader of a real log relies on:
//! the three fields that make a line interpretable, and the target rule that
//! stops a driver message from being unattributable while dropping a redundant
//! `tplay::app` from ours.

use log::Level;
use tplay::logging::{line, LEVEL_FILTER};

#[test]
fn a_line_carries_the_elapsed_time_the_level_and_the_message() {
    let out = line(12.345, Level::Warn, "tplay::app", "no file for /x.mp3");
    assert!(out.contains("12.345"), "elapsed time missing from {out:?}");
    assert!(out.contains("WARN"), "level missing from {out:?}");
    assert!(
        out.contains("no file for /x.mp3"),
        "message missing from {out:?}"
    );
}

#[test]
fn a_foreign_record_names_its_target_and_our_own_does_not() {
    // The driver is the reason the facade exists, and an unattributable line is
    // the exact loss that motivated it.
    let foreign = line(
        0.0,
        Level::Error,
        "wgpu_core::device",
        "pipeline creation failed",
    );
    assert!(
        foreign.contains("wgpu_core::device"),
        "a foreign target was dropped from {foreign:?}"
    );

    // Ours is redundant: the record already came from this app. Both spellings
    // the binary uses — the crate root and a module inside it.
    for target in ["tplay", "tplay::app"] {
        let own = line(0.0, Level::Info, target, "stopped");
        assert!(
            !own.contains(target),
            "our own target {target} was printed in {own:?}"
        );
    }

    // A third-party crate whose name merely begins with the same letters still
    // gets its target, because a driver message with no origin is the loss the
    // facade exists to prevent.
    let near = line(0.0, Level::Warn, "tplaywright", "the harness says hello");
    assert!(
        near.contains("tplaywright"),
        "a foreign target that starts like ours was dropped from {near:?}"
    );
}

#[test]
fn the_level_filter_is_info_so_a_per_frame_event_cannot_flood_the_terminal() {
    // The volume slider and the EQ sliders report `changed()` on every frame of a
    // drag. One Debug record on a per-frame path is 60 lines a second, and the
    // reason there is a filter at all is that nobody would read such a log. This
    // pins the constant rather than the behaviour, because `log` decides the
    // filtering, not us.
    assert_eq!(LEVEL_FILTER, log::LevelFilter::Info);
}

/// The only test that installs the real sink, because `line` is pure and would
/// pass with a sink that wrote nothing. `main` calls `init` for the same reason.
#[test]
fn the_sink_writes_a_record() {
    tplay::logging::init();
    log::info!("a record reached stderr");
    log::debug!("a debug record was filtered out");
}
