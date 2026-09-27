//! The library database — `library.json` and the two-map split that makes a
//! re-scan harmless.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tplay::library::TrackInfo;
use tplay::library_db::{PlayStats, Rule, SmartView, TrackDb};
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
        .apply_edit(
            &file,
            Edit {
                rating: Some(5),
                ..Default::default()
            },
        )
        .expect("a local track is writable");
    assert_eq!(
        t.app.db().cache().get(&file).expect("cached").rating,
        5,
        "the cache holds the file's answer, not what was asked for"
    );

    let uri = PathBuf::from("smb://nas/media/remote.wav");
    t.app
        .apply_edit(
            &uri,
            Edit {
                rating: Some(1),
                ..Default::default()
            },
        )
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

// ── Smart Views ───────────────────────────────────────────────────────────────

/// Epoch a date rule is measured against; any fixed instant, since `select` takes
/// the clock as a parameter.
const NOW: u64 = 1_700_000_000;

/// A database over `dir` holding the given `(name, title, rating)` triples, and
/// `plays` for the first `n` of them.
fn views_db(dir: &Path, tracks: &[(&str, &str, u8)], plays: &[(&str, u32)]) -> TrackDb {
    let mut db = TrackDb::at(&dir.join("library.json"));
    for (name, title, rating) in tracks {
        db.cache_mut().insert(
            dir.join(name),
            TrackInfo {
                title: (*title).into(),
                rating: *rating,
                ..Default::default()
            },
        );
    }
    for (name, times) in plays {
        for _ in 0..*times {
            db.note_played(&dir.join(name), NOW - 10 * 86_400);
        }
    }
    db
}

/// A view's contents, as names — the ids are full temp paths, and the order is
/// the part worth asserting.
fn names(ids: &[PathBuf]) -> Vec<String> {
    ids.iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_view_selects_by_rating_in_playlist_order() {
    let dir = test_dir("a_view_selects_by_rating_in_playlist_order");
    // Deliberately listed in an order that is neither the title order nor the
    // rating order, so both are visible in the result.
    let db = views_db(
        &dir,
        &[
            ("c.mp3", "Cherry", 5),
            ("a.mp3", "Apple", 4),
            ("b.mp3", "Banana", 0),
            ("d.mp3", "Damson", 5),
        ],
        &[],
    );

    let rule = Rule {
        rating_at_least: Some(4),
        ..Rule::default()
    };
    assert_eq!(names(&db.select(&rule, NOW)), ["a.mp3", "c.mp3", "d.mp3"]);

    // A whole-star field is a filter, not a ranking: the result is in title order,
    // so 5-star and 4-star tracks are interleaved rather than grouped by rating.
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn constraints_are_anded_not_alternatives() {
    let dir = test_dir("constraints_are_anded_not_alternatives");
    let db = views_db(
        &dir,
        &[("a.mp3", "A", 5), ("b.mp3", "B", 1), ("c.mp3", "C", 4)],
        &[("a.mp3", 5), ("b.mp3", 5), ("c.mp3", 1)],
    );

    // "rated 4+ and played 3+" is one rule. An enum of alternatives could not
    // express it without a new variant for the combination.
    let rule = Rule {
        rating_at_least: Some(4),
        played_at_least: Some(3),
        ..Rule::default()
    };
    assert_eq!(names(&db.select(&rule, NOW)), ["a.mp3"]);
    fs::remove_dir_all(&dir).unwrap();
}

/// The load-bearing guard. A rule with no constraint matches **nothing**: the
/// alternative would turn a view into the whole library the moment a constraint
/// was dropped from its definition, and the next click would replace the playlist
/// with all of it.
#[test]
fn an_empty_rule_matches_nothing_rather_than_everything() {
    let dir = test_dir("an_empty_rule_matches_nothing_rather_than_everything");
    let db = views_db(&dir, &[("a.mp3", "A", 3), ("b.mp3", "B", 4)], &[]);
    assert!(Rule::default().is_empty());
    assert!(db.select(&Rule::default(), NOW).is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn top_played_ranks_and_never_pads_with_unplayed_tracks() {
    let dir = test_dir("top_played_ranks_and_never_pads_with_unplayed_tracks");
    let db = views_db(
        &dir,
        &[
            ("a.mp3", "A", 0),
            ("b.mp3", "B", 0),
            ("c.mp3", "C", 0),
            ("d.mp3", "D", 0),
        ],
        &[("a.mp3", 1), ("b.mp3", 5), ("c.mp3", 3)],
    );

    let rule = Rule {
        top_played: Some(25),
        ..Rule::default()
    };
    // 25 asked for, 3 played: a ranking is not padded out to its limit with
    // tracks nobody has played, and the order is the play count, not the title.
    assert_eq!(names(&db.select(&rule, NOW)), ["b.mp3", "c.mp3", "a.mp3"]);

    let fewer = Rule {
        top_played: Some(2),
        ..Rule::default()
    };
    assert_eq!(names(&db.select(&fewer, NOW)), ["b.mp3", "c.mp3"]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn added_within_days_reads_the_one_time_stamp() {
    let dir = test_dir("added_within_days_reads_the_one_time_stamp");
    let mut db = views_db(
        &dir,
        &[("old.mp3", "Old", 0), ("fresh.mp3", "Fresh", 0)],
        &[],
    );
    db.note_played(&dir.join("old.mp3"), NOW - 200 * 86_400);
    db.note_played(&dir.join("fresh.mp3"), NOW - 10 * 86_400);
    // "new.mp3" was scanned but never played, so it has no stats entry and no
    // first-seen: it is new, and a date rule must claim it neither way.
    db.cache_mut()
        .insert(dir.join("new.mp3"), TrackInfo::default());

    let rule = Rule {
        added_within_days: Some(90),
        ..Rule::default()
    };
    assert_eq!(names(&db.select(&rule, NOW)), ["fresh.mp3"]);

    let tight = Rule {
        added_within_days: Some(5),
        ..Rule::default()
    };
    assert!(db.select(&tight, NOW).is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

/// Views survive the file, and a view carrying a constraint this build has never
/// heard of still loads — the reason `Rule` is a struct of `Option`s rather than
/// an enum, since a serde enum fails on an unknown variant and takes the whole
/// database (play history included) down with it.
#[test]
fn views_round_trip_and_tolerate_an_unknown_constraint() {
    let dir = test_dir("views_round_trip_and_tools_tolerate");
    let path = dir.join("library.json");
    let mut db = views_db(&dir, &[("a.mp3", "A", 4)], &[]);
    db.add_view(SmartView {
        name: "Rated 4+".into(),
        rule: Rule {
            rating_at_least: Some(4),
            ..Rule::default()
        },
    });
    db.persist(NOW);

    let back = TrackDb::load_from(Some(&path));
    assert_eq!(back.views().len(), 1);
    assert_eq!(back.views()[0].name, "Rated 4+");
    assert_eq!(names(&back.select(&back.views()[0].rule, NOW)), ["a.mp3"]);

    // A file from a build with a constraint this one lacks.
    fs::write(
        &path,
        r#"{"tags":{},"stats":{},"views":[{"name":"Future","rule":{"loudness_at_least":-3}}]}"#,
    )
    .unwrap();
    let future = TrackDb::load_from(Some(&path));
    assert_eq!(
        future.views().len(),
        1,
        "the unknown field is ignored, not fatal"
    );
    assert!(future.views()[0].rule.is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

/// The name is the identity: a sidebar with two rows reading "Unrated" would make
/// the second undeletable in practice, so a same-named view replaces.
#[test]
fn a_view_is_replaced_by_name_and_removal_reports_whether_it_removed_one() {
    let dir = test_dir("a_view_is_replaced_by_name_and_removal_reports");
    let mut db = TrackDb::new();
    for stars in [1u8, 5] {
        db.add_view(SmartView {
            name: "Unrated".into(),
            rule: Rule {
                rating_at_most: Some(stars),
                ..Rule::default()
            },
        });
    }
    assert_eq!(db.views().len(), 1, "the second replaced the first");
    assert_eq!(db.views()[0].rule.rating_at_most, Some(5));

    assert!(db.remove_view("Unrated"));
    assert!(db.views().is_empty());
    assert!(
        !db.remove_view("Unrated"),
        "removing a missing view is not an error"
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// The app's end of a view: the playlist becomes the selection, and nothing is
/// tracked, so Save asks where to go rather than overwriting a file.
#[test]
fn a_view_replaces_the_playlist_and_tracks_no_file() {
    use tplay::tracks::Edit;

    let mut t = TestApp::new("a_view_replaces_the_playlist_and_tracks_no_file");
    let dir = t.app.library().dir().to_path_buf();
    // Ratings are set through the real write path — `apply_edit` writes the file
    // and takes its answer back — so the cache holds what a scan would have found.
    for (name, stars) in [("a.wav", 5u8), ("b.wav", 0), ("c.wav", 4)] {
        let p = dir.join(name);
        write_wav(&p);
        if stars > 0 {
            t.app
                .apply_edit(
                    &p,
                    Edit {
                        rating: Some(stars),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    t.app.add_smart_view(SmartView {
        name: "Rated 4+".into(),
        rule: Rule {
            rating_at_least: Some(4),
            ..Rule::default()
        },
    });

    let view = t.app.smart_views()[0].clone();
    t.app.load_smart_view(&view);

    // Untagged files order by their stem, which is also what the file list does.
    let got: Vec<String> = t
        .app
        .playlist()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(got, ["a.wav", "c.wav"]);
    assert!(
        t.app.playlist_file().is_none(),
        "a view is a query, not a file to Save over"
    );
    assert!(
        !t.app.playlist_dirty(),
        "a fresh selection has no unsaved edits"
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// The claim phase 2 could not test: a rating is not a sort column, so the
/// re-sort inside `apply_edit` had no observable effect and no test could fail
/// without it. A title is one of `SORT_OPTIONS`, so now it does.
///
/// Two rows, both untagged so the file list orders them by stem — `a` before `b` —
/// and one edit that turns `b` into "Aardvark". If the entries were not re-sorted
/// in place, the row would keep its old position and the file list would be
/// showing a stale order behind a fresh tag.
#[test]
fn an_edit_moves_the_row_it_belongs_on() {
    use tplay::tracks::Edit;

    let mut t = TestApp::new("an_edit_moves_the_row_it_belongs_on");
    let dir = t.app.library().dir().to_path_buf();
    for name in ["a.wav", "b.wav"] {
        write_wav(&dir.join(name));
    }
    // The listing was taken when the folder was still empty, so re-list now that
    // the files exist — the same thing opening the folder again would do.
    t.app.navigate_to(dir.clone());
    // `navigate_to` is what the first frame does, but the test drives `new`, so
    // the listing exists; assert it rather than assume it.
    let order = |t: &TestApp| {
        t.app
            .library()
            .entries()
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<String>>()
    };
    assert_eq!(order(&t), ["a", "b"], "premise: untagged rows sort by stem");

    // "Zebra" sorts after "b", so the edited row has to *move down*. A title that
    // merely started with a different letter would not do: an untagged key is the
    // bare stem, so titling `a` "Aardvark" leaves it ahead of "b"... and ahead of
    // its own former self.
    t.app
        .apply_edit(
            &dir.join("a.wav"),
            Edit {
                title: Some("Zebra"),
                ..Default::default()
            },
        )
        .expect("a local write");

    assert_eq!(
        order(&t),
        ["b", "a"],
        "the edited row moved to where its new title sorts"
    );
    assert_eq!(
        t.app.db().cache().get(&dir.join("a.wav")).unwrap().title,
        "Zebra"
    );
    assert_eq!(order(&t).len(), 2, "and no row was duplicated or dropped");
    fs::remove_dir_all(&dir).unwrap();
}
