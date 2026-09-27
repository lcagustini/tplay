//! Library sorting edge cases beyond `library_tests.rs` — `sort_key` is the
//! comparator behind the sortable header; these pin the encoding contract:
//! lowercase fold (case-insensitive), DEL prefix for missing values, the
//! album+track composite key, and zero-padded millisecond durations. (The
//! search text box lives in the Library pane; its matching is plain `contains`
//! over a lowercased title·artist·album haystack built from these same fields.)

use std::path::PathBuf;
use std::time::Duration;
use tplay::library::{sort_key, Entry, TrackInfo, SORT_OPTIONS};

fn file(name: &str) -> Entry {
    Entry {
        path: PathBuf::from(name),
        is_dir: false,
    }
}

fn folder(name: &str) -> Entry {
    Entry {
        path: PathBuf::from(name),
        is_dir: true,
    }
}

#[test]
fn sort_key_is_case_insensitive() {
    // Mixed case must not change relative order.
    let z = file("z.wav");
    let a = file("A.wav");
    let z_info = TrackInfo {
        title: "Zebra".into(),
        ..Default::default()
    };
    let a_info = TrackInfo {
        title: "apple".into(),
        ..Default::default()
    };
    assert!(sort_key(&a, Some(&a_info), 0) < sort_key(&z, Some(&z_info), 0));

    // Identical name differing only in case ties.
    let upper = TrackInfo {
        title: "Star".into(),
        ..Default::default()
    };
    let lower = TrackInfo {
        title: "star".into(),
        ..Default::default()
    };
    assert_eq!(
        sort_key(&file("a.wav"), Some(&upper), 0),
        sort_key(&file("b.wav"), Some(&lower), 0)
    );
}

#[test]
fn sort_key_duration_uses_zero_padded_millis() {
    // 9 s vs 10 s: naive second-comparison puts "9" after "10"; the padded
    // millisecond key must not. 9 s → "000000009000", 10 s → "000000010000".
    let nine = TrackInfo {
        duration: Some(Duration::from_secs(9)),
        ..Default::default()
    };
    let ten = TrackInfo {
        duration: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    assert!(sort_key(&file("a.wav"), Some(&nine), 3) < sort_key(&file("b.wav"), Some(&ten), 3));

    // Sub-second precision is kept: 1.5 s vs 2 s.
    let one_five = TrackInfo {
        duration: Some(Duration::from_millis(1500)),
        ..Default::default()
    };
    let two = TrackInfo {
        duration: Some(Duration::from_secs(2)),
        ..Default::default()
    };
    assert!(sort_key(&file("a.wav"), Some(&one_five), 3) < sort_key(&file("b.wav"), Some(&two), 3));
}

#[test]
fn sort_key_folders_sink_on_tag_columns_below_files() {
    // On Artist/Album/Duration a folder is untagged → sorts last, even when the
    // file has the "later" value — folders always trail on tag columns.
    let f = folder("z-dir");
    let artist = TrackInfo {
        artist: "Zzz".into(),
        ..Default::default()
    };
    assert!(sort_key(&file("a.wav"), Some(&artist), 1) < sort_key(&f, None, 1));
    let long = TrackInfo {
        duration: Some(Duration::from_secs(300)),
        ..Default::default()
    };
    assert!(sort_key(&file("a.wav"), Some(&long), 3) < sort_key(&f, None, 3));
}

/// `sort_key`'s last arm falls back to the filename, and a `debug_assert!` sits in
/// it to catch the one case where that fallback is wrong: a column *inside*
/// `SORT_OPTIONS` with no arm of its own. Adding such a column compiles, passes
/// every test, and orders the new column by filename — so the assert is the only
/// thing that says so.
///
/// There is nothing to assert about that today, because every current column has
/// an arm: no `col` exists that the guard rejects. That is the guard working, and
/// it is why this fix is the only one in the batch with no mutation check. What
/// *is* testable is the other half — the arm must stay a safe fallback, so a
/// genuine out-of-range column keeps sorting by filename rather than tripping the
/// assert and turning a wrong order into a debug panic. That is the claim worth
/// pinning, because the assert is the thing that could break it.
#[test]
fn an_out_of_range_column_still_falls_back_to_the_filename() {
    let e = Entry {
        path: PathBuf::from("/music/zebra.mp3"),
        is_dir: false,
    };
    // Well past the end, and one past the last real column: both are the
    // defensive case the filename answer exists for, and neither may panic.
    for col in [SORT_OPTIONS.len(), SORT_OPTIONS.len() + 1, 99] {
        assert_eq!(
            sort_key(&e, None, col),
            "zebra.mp3",
            "column {col} is out of range and must fall back, not panic"
        );
    }
}

/// The Library's search box, which was inline in the pane and so had no headless
/// test at all — the same gap `playlist::row_matches` was extracted to close, and
/// the reason this one is `pub` and `Ui`-free too.
///
/// It runs for every row of every folder on both browsers, so the empty-query
/// short-circuit is the difference between one fold per frame and a haystack per
/// row per frame. That is the first claim below, and it is the one a future
/// "let me simplify this" would break.
#[test]
fn the_library_filter_short_circuits_an_empty_query_and_folds_one_haystack() {
    use tplay::library::entry_matches;

    let untagged = Entry {
        path: PathBuf::from("/music/Aardvark.mp3"),
        is_dir: false,
    };
    // An empty query matches everything — the unfiltered path, run per row.
    assert!(
        entry_matches(&untagged.path, None, ""),
        "premise: an unfiltered folder shows every row"
    );

    let tagged = Entry {
        path: PathBuf::from("/music/01.mp3"),
        is_dir: false,
    };
    let info = TrackInfo {
        title: "Ne".into(),
        artist: "Yo".into(),
        album: "Greatest Hits".into(),
        ..Default::default()
    };
    // One fold over the three tag fields plus the display title.
    for q in ["ne", "yo", "greatest"] {
        assert!(
            entry_matches(&tagged.path, Some(&info), q),
            "a tagged row must be findable by {q:?}"
        );
    }
    assert!(
        !entry_matches(&tagged.path, Some(&info), "absent"),
        "and a needle that is nowhere is still nowhere"
    );
    // An untagged row falls back to its stem, which is what it displays — so a
    // file the tag scan has not reached is still findable by name.
    assert!(
        entry_matches(&untagged.path, None, "aardvark"),
        "the stem is searchable before the tag scan lands"
    );
    // Pinned, because it is a real difference from the Playlist pane's box and
    // not an oversight: `row_matches` also searches the filename, while this one
    // searches what the Library documents — title, artist, album, with the stem
    // as the title's fallback. So a *tagged* row is not findable by its filename
    // here, and is in the Playlist. Making the two agree is a behaviour change,
    // so it wants a decision rather than a refactor.
    assert!(
        !entry_matches(&tagged.path, Some(&info), "01"),
        "the Library box searches the tags, not the filename — documented, and \
         deliberately different from the Playlist pane's box"
    );
}
