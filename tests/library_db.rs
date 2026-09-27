//! The library database — `library.json` and the two-map split that makes a
//! re-scan harmless.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tplay::library::TrackInfo;
use tplay::library_db::{PlayStats, TrackDb};
#[path = "common.rs"]
mod common;
use crate::common::{test_dir, write_wav, TestApp};

/// The app half: a real play is counted, and **only** a real play. `start_track`
/// is the single place a play happens, so a seek — which rebuilds the sink
/// without going through it — must not count as a second one.
#[test]
fn playing_a_track_counts_a_play() {
    let mut t = TestApp::new("playing_a_track_counts_a_play");
    let file = t.app.library().dir().join("tone.wav");
    write_wav(&file);

    t.app.play_file(file.clone());
    t.pump(0.2);
    assert_eq!(
        t.app
            .db()
            .stats_of(&file)
            .expect("a play was recorded")
            .plays,
        1
    );

    // `pump_during`, not a bare `seek`: `Sink::try_seek` blocks until the mixer
    // services it, so pumping *after* the call would deadlock — the pump is the
    // thing it is waiting for. See `TestApp::pump_during`.
    t.pump_during(|app| app.seek(0.5));
    t.pump(0.2);
    let stats = t.app.db().stats_of(&file).unwrap();
    assert_eq!(stats.plays, 1, "a seek is not a second play");
    assert!(stats.last_played.is_some());
}

/// A track id in the temp dir, so nothing here can reach a real library.
fn track(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

fn tags(title: &str) -> TrackInfo {
    TrackInfo {
        title: title.into(),
        ..Default::default()
    }
}

/// The whole point of the split: the scan thread's insert path and the play
/// counter's write path share nothing, so re-reading a file's tags cannot cost
/// the user a play count.
///
/// The mutation this pins is putting both in one row type with a merge that
/// copies the mutable fields back — a rule a caller can forget, and forgetting
/// it is silent.
#[test]
fn a_rescan_cannot_reach_a_play_count() {
    let dir = test_dir("a_rescan_cannot_reach_a_play_count");
    let file = track(&dir, "song.mp3");
    let path = dir.join("library.json");

    let mut db = TrackDb::at(&path);
    // First visit: the scan fills the cache, and a play is recorded.
    db.cache_mut().insert(file.clone(), tags("First Read"));
    db.note_played(&file, 1_000);
    db.persist(1_000);

    // Second visit, after the file's tags have changed on disk. This is the
    // same `cache_mut().insert` the scan thread makes, with no knowledge that a
    // play count exists.
    let mut db = TrackDb::load_from(Some(&path));
    db.cache_mut().insert(file.clone(), tags("Re-read"));
    db.persist(2_000);

    let db = TrackDb::load_from(Some(&path));
    assert_eq!(
        db.cache().get(&file).map(|i| i.title.as_str()),
        Some("Re-read"),
        "the newer tags win"
    );
    let stats = db.stats_of(&file).expect("the play count survived");
    assert_eq!(stats.plays, 1);
    assert_eq!(stats.first_seen, 1_000, "and so did the original stamp");

    fs::remove_dir_all(&dir).unwrap();
}

/// `first_seen` means *first*, so a play, a second save and a later save must
/// all leave the original stamp alone — it is written by one method and no
/// other.
#[test]
fn first_seen_is_stamped_once_and_never_moved() {
    let dir = test_dir("first_seen_is_stamped_once_and_never_moved");
    let file = track(&dir, "song.mp3");
    let path = dir.join("library.json");

    let mut db = TrackDb::at(&path);
    db.cache_mut().insert(file.clone(), tags("T"));
    db.persist(1_000);
    assert_eq!(db.stats_of(&file).unwrap().first_seen, 1_000);

    // Played much later, then saved again.
    db.note_played(&file, 500_000);
    db.persist(900_000);

    let db = TrackDb::load_from(Some(&path));
    let stats = db.stats_of(&file).unwrap();
    assert_eq!(
        stats.first_seen, 1_000,
        "the stamp does not follow the play"
    );
    assert_eq!(stats.plays, 1);
    assert_eq!(stats.last_played, Some(500_000));

    fs::remove_dir_all(&dir).unwrap();
}

/// A track that has never been persisted has no first-seen, and saying so is
/// what keeps a date rule honest about it.
#[test]
fn an_unpersisted_track_has_no_first_seen() {
    let dir = test_dir("an_unpersisted_track_has_no_first_seen");
    let file = track(&dir, "song.mp3");
    let mut db = TrackDb::new();
    db.cache_mut().insert(file.clone(), tags("T"));
    assert!(db.stats_of(&file).is_none());

    // A play is enough to make it visible, without waiting for a save.
    db.note_played(&file, 42);
    assert_eq!(db.stats_of(&file).unwrap().first_seen, 42);

    fs::remove_dir_all(&dir).unwrap();
}

/// Every tag field survives, including a sub-second duration: the file stores
/// milliseconds, so a 1.5 s track must not come back as 1 s.
#[test]
fn tags_and_play_history_round_trip() {
    let dir = test_dir("tags_and_play_history_round_trip");
    let file = track(&dir, "song.mp3");
    let path = dir.join("library.json");

    let info = TrackInfo {
        title: "Ne-Yo".into(),
        artist: "Test Artist".into(),
        album: "Test Album".into(),
        track_no: Some("3".into()),
        duration: Some(Duration::from_millis(1_500)),
        rating: 4,
    };
    let mut db = TrackDb::at(&path);
    db.cache_mut().insert(file.clone(), info.clone());
    db.note_played(&file, 7);
    db.persist(1_000);

    let db = TrackDb::load_from(Some(&path));
    let back = db.cache().get(&file).expect("the tag row survived");
    assert_eq!(back.title, "Ne-Yo");
    assert_eq!(back.artist, "Test Artist");
    assert_eq!(back.album, "Test Album");
    assert_eq!(back.track_no.as_deref(), Some("3"));
    assert_eq!(back.rating, 4);
    assert_eq!(back.duration, Some(Duration::from_millis(1_500)));
    assert_eq!(db.stats_of(&file).unwrap().plays, 1);

    fs::remove_dir_all(&dir).unwrap();
}

/// The write throttle compares content, so the file must come out in key order:
/// a `HashMap` serializes in iteration order, which is randomized per process,
/// which would make every launch look changed.
#[test]
fn the_file_is_written_in_key_order() {
    let dir = test_dir("the_file_is_written_in_key_order");
    let mut db = TrackDb::new();
    // Inserted in an order that is not the sorted one, and enough of them that
    // a coincidence of iteration order is not the likely outcome.
    for name in ["m.mp3", "a.mp3", "z.mp3", "b.mp3"] {
        db.cache_mut()
            .insert(track(&dir, name), tags(name.to_uppercase().as_str()));
    }

    let json = db.snapshot();
    let mut last = 0;
    for name in ["a.mp3", "b.mp3", "m.mp3", "z.mp3"] {
        let at = json
            .find(name)
            .unwrap_or_else(|| panic!("{name} is in the file"));
        assert!(at > last, "{name} is out of order — the file is not sorted");
        last = at;
    }

    fs::remove_dir_all(&dir).unwrap();
}

/// Play history exists nowhere else, so a malformed file is kept rather than
/// replaced by the next write. Same contract as `config::load_from`.
#[test]
fn a_malformed_file_is_copied_aside() {
    let dir = test_dir("a_malformed_file_is_copied_aside");
    let path = dir.join("library.json");
    fs::write(&path, "{ this is not json").unwrap();

    let db = TrackDb::load_from(Some(&path));
    assert!(db.cache().is_empty(), "an unreadable file loads as empty");

    let kept = path.with_extension("json.broken");
    assert_eq!(
        fs::read_to_string(&kept).unwrap(),
        "{ this is not json",
        "the original is still there"
    );

    fs::remove_dir_all(&dir).unwrap();
}

/// Forward compatibility both ways: a field this build does not know is
/// ignored, and one it expects but does not find falls back. The second half is
/// what a file written before Smart Views existed has to survive.
#[test]
fn absent_and_unknown_fields_both_load() {
    let dir = test_dir("absent_and_unknown_fields_both_load");
    let path = dir.join("library.json");

    fs::write(&path, "{}").unwrap();
    let db = TrackDb::load_from(Some(&path));
    assert!(db.cache().is_empty() && db.stats_of(&PathBuf::from("x")).is_none());

    fs::write(
        &path,
        r#"{"tags":{},"stats":{},"views":[{"name":"Future"}]}"#,
    )
    .unwrap();
    let db = TrackDb::load_from(Some(&path));
    assert!(
        db.cache().is_empty(),
        "an unknown key is ignored, not fatal"
    );

    fs::remove_dir_all(&dir).unwrap();
}

/// The tooltip's wording, which is the only place the play history is read. Four
/// buckets, because the difference between "4 minutes ago" and "6 minutes ago" is
/// not information anybody reads a tooltip for.
#[test]
fn a_play_history_line_says_how_often_and_how_long_ago() {
    const NOW: u64 = 1_700_000_000;
    let at = |plays: u32, secs_ago: u64| PlayStats {
        plays,
        last_played: Some(NOW - secs_ago),
        first_seen: 0,
    };

    assert!(
        PlayStats::default().describe(NOW).is_none(),
        "never played — a tooltip that always shows teaches the reader to ignore it"
    );
    assert_eq!(
        at(1, 30).describe(NOW).unwrap(),
        "Played once · last just now",
        "one play is not '1 times'"
    );
    assert_eq!(
        at(7, 5 * 60).describe(NOW).unwrap(),
        "Played 7 times · last 5 min ago"
    );
    assert_eq!(
        at(7, 4 * 3600).describe(NOW).unwrap(),
        "Played 7 times · last 4 h ago"
    );
    assert_eq!(
        at(7, 3 * 86_400).describe(NOW).unwrap(),
        "Played 7 times · last 3 d ago"
    );
}

/// The app's half of a tag write: the file's own answer reaches the cache, and a
/// refusal leaves **nothing** behind. There is no half-applied edit — the cache
/// and the file agree or the call did nothing.
#[test]
fn an_edit_lands_in_the_cache_and_a_refusal_changes_nothing() {
    use tplay::tracks::Edit;

    let mut t = TestApp::new("an_edit_lands_in_the_cache_and_a_refusal_changes_nothing");
    let file = t.app.library().dir().join("tone.wav");
    write_wav(&file);

    t.app
        .apply_edit(&file, Edit { rating: Some(5) })
        .expect("a local track is writable");
    assert_eq!(
        t.app.db().cache().get(&file).expect("cached").rating,
        5,
        "the cache holds the file's answer, not what was asked for"
    );

    let uri = PathBuf::from("smb://nas/media/remote.wav");
    t.app
        .apply_edit(&uri, Edit { rating: Some(1) })
        .expect_err("a remote track is refused");
    assert!(
        t.app.db().cache().get(&uri).is_none(),
        "the refusal left no row behind"
    );
}

/// A missing file is a first run, not damage — and leaves nothing behind, which
/// is the difference from the malformed case above.
#[test]
fn a_missing_file_is_a_fresh_database() {
    let dir = test_dir("a_missing_file_is_a_fresh_database");
    let path = dir.join("library.json");
    let db = TrackDb::load_from(Some(&path));
    assert!(db.cache().is_empty());
    assert!(
        !path.with_extension("json.broken").exists(),
        "nothing was kept, because nothing was damaged"
    );

    fs::remove_dir_all(&dir).unwrap();
}
