//! The seek-position arithmetic, on its own.
//!
//! The slow path that sets the offset cannot be *forced* — `try_seek` succeeds
//! for every format a test could cheaply produce, so the `skip_duration`
//! fallback never runs, with or without a device. That leaves one option: name
//! the arithmetic and test it directly.
//!
//! The failure this guards is silent by nature. A fast seek lands in place and
//! the offset is zero, so a broken `effective_pos` still shows the right
//! position for most of a session — it goes wrong only after a slow-path seek,
//! where the seek bar and the time label quietly start reading low by exactly
//! the skip amount. Nothing crashes; the UI just lies.

use std::time::Duration;
use tplay::app::effective_pos;

#[test]
fn a_zero_offset_is_the_identity() {
    // The overwhelmingly common case: a fresh sink, or one a *fast* seek
    // landed in. Nothing may perturb the position rodio reports.
    for secs in [0, 1, 30, 3600] {
        let pos = Duration::from_secs(secs);
        assert_eq!(
            effective_pos(pos, Duration::ZERO),
            pos,
            "{secs}s must pass through"
        );
    }
}

#[test]
fn the_offset_is_added_to_the_sinks_own_count() {
    // A slow-path seek rebuilds the sink at the target, so get_pos() restarts
    // near zero and the skip has to be put back for the playhead to be right.
    assert_eq!(
        effective_pos(Duration::from_secs(2), Duration::from_secs(30)),
        Duration::from_secs(32),
        "2s into a 30s skip is 32s into the track"
    );
}

#[test]
fn an_offset_alone_reports_the_skip_target() {
    // A seek to 0 on the slow path: the fresh sink has played nothing yet, so
    // the position must still be the target rather than 0.
    assert_eq!(
        effective_pos(Duration::ZERO, Duration::from_secs(45)),
        Duration::from_secs(45)
    );
}

#[test]
fn sub_second_offsets_are_not_truncated() {
    // Millisecond-precision offsets are ordinary — `progress * total_secs` is
    // a float, and a seek can land anywhere. Truncating to whole seconds here
    // would make every sub-second seek read as up to 1s early.
    let pos = effective_pos(Duration::from_millis(1_500), Duration::from_millis(250));
    assert_eq!(pos, Duration::from_millis(1_750));
}

#[test]
fn the_sum_saturates_rather_than_panicking() {
    // The two inputs are independent — one is real playback time, the other a
    // *previous* skip — so a bad pair must not be able to panic in the audio
    // path. Duration::MAX is the ceiling; a plain `+` would overflow and panic.
    let huge = Duration::MAX;
    assert_eq!(
        effective_pos(huge, Duration::from_secs(1)),
        huge,
        "must clamp, not panic"
    );
    assert_eq!(effective_pos(Duration::from_secs(1), huge), huge);
    assert_eq!(effective_pos(huge, huge), huge);
}
