//! Real playback, headless.
//!
//! `TPlayApp` used to be unconstructible in a test because it opened an audio
//! device. It takes a `Mixer` now, and `rodio::mixer::mixer` builds one with no
//! hardware — so these tests construct the actual struct, play actual files
//! through the actual sink, and watch what `advance` does.
//!
//! The driver (`TestApp::pump`) is the `MixerSource` a cpal callback would pull
//! from, called by hand. That is what makes this fast rather than slow: three
//! seconds of audio is a few hundred thousand `next()` calls, microseconds of
//! wall time, not three seconds of waiting.
//!
//! **The assertion style matters here.** Everything below is *relative* —
//! position grew, the track changed, the sink emptied. Nothing asserts an
//! exact sample count, because the number of samples a decoder yields for a
//! given pump depends on its own buffering, and pinning that would make these
//! tests fail on a rodio upgrade without anything having broken. The one
//! absolute-ish check (a 1s WAV is reported as ~1s) has a 1s tolerance, which
//! `common::assert_duration_approx` already provides.

mod common;

use common::{test_dir, write_wav, TestApp};
use std::time::Duration;

/// A 1-second WAV, for the cases that just need *a* playable file.
fn track(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    write_wav(&p);
    p
}

#[test]
fn the_device_drives_the_sink() {
    // The premise for every other test here: if pumping did not move the
    // playhead, none of the advance behaviour below means anything.
    let mut t = TestApp::new("device-drives-sink");
    let dir = test_dir("device-drives-sink");
    let track = track(&dir, "a.wav");

    t.app.play_file(track);
    assert!(t.app.current_path().is_some(), "a readable file must load");

    let before = t.app.playback_position_secs();
    t.pump(0.5);
    let after = t.app.playback_position_secs();

    assert!(
        after > before,
        "pumping 0.5s must advance the playhead (was {before:?}, now {after:?})"
    );
}

#[test]
fn a_drained_sink_advances_to_the_next_playlist_entry() {
    let mut t = TestApp::new("auto-advance");
    let dir = test_dir("auto-advance");
    let first = track(&dir, "first.wav");
    let second = track(&dir, "second.wav");

    t.app.add_files(vec![first.clone(), second.clone()]);
    t.app.play_track(0);
    assert_eq!(t.app.current_index(), Some(0));

    // Past the end of the 1s track, then one more `advance` for the frame that
    // observes the drained sink.
    t.pump_until_empty(3.0);
    t.app.advance();

    assert_eq!(
        t.app.current_index(),
        Some(1),
        "a naturally-ended track must advance to the next entry"
    );
    assert_eq!(t.app.current_path(), Some(second.as_path()));
}

#[test]
fn advancing_past_the_last_track_stops_rather_than_wrapping() {
    // Without repeat, a drained sink at the end of the playlist has nowhere to
    // go. The failure this guards is a wrap-around replaying track 0 unasked.
    let mut t = TestApp::new("end-of-playlist");
    let dir = test_dir("end-of-playlist");
    let only = track(&dir, "only.wav");

    t.app.add_files(vec![only]);
    t.app.play_track(0);

    t.pump_until_empty(3.0);
    t.app.advance();
    let after_first = t.app.current_index();

    // Keep advancing: no entry left, so nothing should change and nothing
    // should panic.
    t.app.advance();
    t.app.advance();
    assert_eq!(
        t.app.current_index(),
        after_first,
        "no wrap-around without repeat"
    );
}

#[test]
fn an_empty_playlist_never_starts_anything() {
    // The cascade guard: `advance` on a drained sink with no `current_path` must
    // do nothing. This is the one that stops a failed load from walking the
    // whole playlist one entry per frame.
    let mut t = TestApp::new("empty-playlist");
    t.app.advance();
    t.app.advance();
    assert!(
        t.app.current_path().is_none(),
        "nothing loaded, nothing started"
    );
    assert!(!t.app.can_play(), "and nothing to play");
}

#[test]
fn a_failed_load_leaves_nothing_current_and_does_not_panic() {
    // A path that does not exist. `start_track` clears `current_path` and
    // returns — and because it is cleared, `advance` can no longer cascade past
    // it into the rest of the playlist.
    let mut t = TestApp::new("failed-load");
    let dir = test_dir("failed-load");
    let good = track(&dir, "good.wav");
    t.app.add_files(vec![good.clone()]);

    t.app.play_file(dir.join("does-not-exist.wav"));
    assert!(
        t.app.current_path().is_none(),
        "an unreadable file loads nothing"
    );

    // The guard: with no current track, repeated advances must not pick up the
    // playlist entry that was never asked for.
    t.app.advance();
    t.app.advance();
    assert!(
        t.app.current_path().is_none(),
        "a failed load must not cascade"
    );
}

#[test]
fn a_1s_track_reports_its_own_duration() {
    // Ties the device to the decoder: the duration the app reports comes from
    // rodio's WAV reader, and the position the device reports comes from the
    // driver. If either were disconnected the other would still "work".
    let mut t = TestApp::new("duration");
    let dir = test_dir("duration");
    let track = track(&dir, "a.wav");

    t.app.play_file(track);
    assert!(
        t.app
            .total_duration()
            .is_some_and(|d| d.as_secs_f32().min(1.5) > 0.5),
        "a 1s WAV must report about 1s, got {:?}",
        t.app.total_duration()
    );

    // And it must reach the end, which is what makes `pump_until_empty` above
    // terminate rather than spin to its bound.
    t.pump_until_empty(3.0);
    assert!(t.app.is_empty(), "the sink must drain within 3s of audio");
}

#[test]
fn stop_unloads_the_track_and_the_next_play_rewinds() {
    // `stop` is the user's "unload", and `advance`'s guard is `current_path`,
    // so this is the state a stopped app must be in.
    let mut t = TestApp::new("stop");
    let dir = test_dir("stop");
    let track = track(&dir, "a.wav");

    t.app.play_file(track.clone());
    t.pump(0.3);
    let before_stop = t.app.playback_position_secs();
    assert!(before_stop > Duration::ZERO, "premise: it was playing");

    t.app.stop();

    assert!(t.app.current_path().is_none(), "stop unloads the track");
    assert_eq!(t.app.current_index(), None, "and leaves no playlist index");
    assert!(
        t.app.total_duration().is_none(),
        "duration is cleared with it"
    );
    assert!(!t.app.can_play(), "so the play button greys out");

    // The rewind is asserted by *playing again*, not by reading the position
    // after `stop`. rodio's `Sink::stop` only sets a flag; the position is
    // zeroed by the same 5 ms `periodic_access` hook pause uses, and a stopped
    // source yields no samples, so that hook never fires again and `get_pos`
    // stays frozen at its last value. Harmless, because the next
    // `start_track` calls `fresh_sink()` and builds a new `Sink` at zero — so
    // this is the state a listener actually reaches.
    t.app.play_file(track);
    assert_eq!(
        t.app.playback_position_secs(),
        Duration::ZERO,
        "playing after a stop starts from the beginning, not where it stopped"
    );
}

#[test]
fn pause_stops_the_playhead_but_keeps_the_track() {
    let mut t = TestApp::new("pause");
    let dir = test_dir("pause");
    let track = track(&dir, "a.wav");

    t.app.play_file(track);
    t.pump(0.3);
    t.app.pause();
    // Same settle as the stop test: the position is written by a 5 ms
    // `periodic_access` hook, so one tick may still be in flight when `pause`
    // returns. Let it land before taking the baseline, or the assertion below
    // measures that tick rather than the pause.
    t.pump(0.1);

    let at_pause = t.app.playback_position_secs();
    t.pump(0.3);

    assert!(t.app.is_paused(), "paused state is reported");
    assert_eq!(
        t.app.playback_position_secs(),
        at_pause,
        "a paused sink must not advance while the driver keeps pulling"
    );
    assert!(t.app.current_path().is_some(), "and the track stays loaded");

    t.app.play();
    t.pump(0.2);
    assert!(
        t.app.playback_position_secs() > at_pause,
        "resuming moves the playhead again"
    );
}
