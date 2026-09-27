//! The playlist content operations: sorting, randomizing, deduplicating — plus
//! the state repairs `TPlayApp` owes every one of them.
//!
//! `library_search.rs` already pins `sort_key` — the comparator underneath
//! `sort_tracks` — so these do not re-test the key encoding. They pin what the
//! *playlist* adds on top: that a `Vec<PathBuf>` of track ids goes through it
//! intact (a permutation, never a lossy filter), that a remote id is a track id
//! like any other, and that randomizing is a real shuffle of a real RNG.
//!
//! The last group is the part that only exists on the app: a reorder moves
//! indices under a track that is playing, and nothing about the content ops
//! would catch a highlight that stopped following it.

mod common;

use common::{test_dir, write_wav, TestApp};
use std::path::PathBuf;
use std::time::Duration;
use tplay::app::TPlayApp;
use tplay::library::{TagCache, TrackInfo};
use tplay::playlist::{dedup, shuffle_tracks, sort_tracks};

/// Track ids under one directory. Kept absolute so a relative/absolute mix
/// cannot accidentally agree with the expected order.
fn ids(names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|n| PathBuf::from("/music").join(n))
        .collect()
}

/// The same tracks as a title-sorted permutation, for order assertions.
fn names(tracks: &[PathBuf]) -> Vec<String> {
    tracks
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// `TagCache` for `tracks`, with tags given in the same order.
fn cache(tracks: &[PathBuf], infos: Vec<TrackInfo>) -> TagCache {
    tracks.iter().cloned().zip(infos).collect()
}

/// A multiset comparison: sorted copies must match, so a sort that dropped or
/// duplicated a track fails here whatever the order assertion says.
fn same_multiset(before: &[PathBuf], after: &[PathBuf]) -> bool {
    let mut a: Vec<&PathBuf> = before.iter().collect();
    let mut b: Vec<&PathBuf> = after.iter().collect();
    a.sort();
    b.sort();
    a == b
}

// ── Sorting ───────────────────────────────────────────────────────────────────

#[test]
fn sorting_by_title_uses_the_stem_while_a_track_is_untagged() {
    // A freshly loaded playlist is sorted before the tag scan fills in, so the
    // Title column has to be usable on filenames alone.
    let mut tracks = ids(&["C.mp3", "a.mp3", "B.mp3"]);
    sort_tracks(&mut tracks, &TagCache::new(), 0);
    assert_eq!(names(&tracks), ["a.mp3", "B.mp3", "C.mp3"]);
    assert!(same_multiset(&ids(&["C.mp3", "a.mp3", "B.mp3"]), &tracks));
}

#[test]
fn sorting_by_artist_sinks_untagged_tracks_last() {
    let tracks = ids(&["a.mp3", "b.mp3", "c.mp3"]);
    let tags = cache(
        &tracks,
        vec![
            TrackInfo {
                artist: "Zzz".into(),
                ..Default::default()
            },
            TrackInfo {
                artist: "aaa".into(),
                ..Default::default()
            },
            TrackInfo::default(), // no entry at all
        ],
    );
    let mut sorted = tracks.clone();
    sort_tracks(&mut sorted, &tags, 1);
    // aaa, Zzz, then the untagged one — the DEL prefix in `sort_key` is what
    // puts it last, not the filename.
    assert_eq!(names(&sorted), ["b.mp3", "a.mp3", "c.mp3"]);
}

#[test]
fn sorting_by_album_orders_by_track_number_not_alphabetically() {
    // One album, tracks 2 and 10: a plain string sort puts "10" first, which is
    // the bug the zero-padded composite key in `sort_key` exists to prevent.
    let tracks = ids(&["x.mp3", "y.mp3"]);
    let tags = cache(
        &tracks,
        vec![
            TrackInfo {
                album: "One".into(),
                track_no: Some("10".into()),
                ..Default::default()
            },
            TrackInfo {
                album: "One".into(),
                track_no: Some("2".into()),
                ..Default::default()
            },
        ],
    );
    let mut sorted = tracks.clone();
    sort_tracks(&mut sorted, &tags, 2);
    assert_eq!(names(&sorted), ["y.mp3", "x.mp3"]);
}

#[test]
fn sorting_by_duration_orders_by_length() {
    let tracks = ids(&["long.mp3", "short.mp3", "medium.mp3"]);
    let tags = cache(
        &tracks,
        vec![
            TrackInfo {
                duration: Some(Duration::from_secs(300)),
                ..Default::default()
            },
            TrackInfo {
                duration: Some(Duration::from_secs(3)),
                ..Default::default()
            },
            TrackInfo {
                duration: Some(Duration::from_secs(30)),
                ..Default::default()
            },
        ],
    );
    let mut sorted = tracks.clone();
    sort_tracks(&mut sorted, &tags, 3);
    assert_eq!(names(&sorted), ["short.mp3", "medium.mp3", "long.mp3"]);
}

#[test]
fn sorting_leaves_every_track_present_exactly_once_on_every_column() {
    // The invariant behind all four column assertions: a comparator must not be
    // able to drop or duplicate. A `sort_by` that panicked or a key that
    // panicked mid-way would fail here.
    let tracks = ids(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
    let tags = cache(
        &tracks,
        (0..5)
            .map(|i| TrackInfo {
                title: format!("T{i}"),
                artist: format!("A{}", 4 - i),
                album: format!("Al{i}"),
                track_no: Some((i + 1).to_string()),
                duration: Some(Duration::from_secs(i * 7)),
            })
            .collect(),
    );
    for col in 0..4 {
        let mut sorted = tracks.clone();
        sort_tracks(&mut sorted, &tags, col);
        assert_eq!(
            sorted.len(),
            tracks.len(),
            "column {col} changed the length"
        );
        assert!(
            same_multiset(&tracks, &sorted),
            "column {col} lost or duplicated a track: {:?}",
            names(&sorted)
        );
    }
}

#[test]
fn a_remote_track_sorts_like_a_local_one() {
    // The claim that lets this module have no `is_remote` branch: a URI's last
    // segment is the filename, so the same key handles it. Premise: the URI and
    // the local file are NOT already in title order.
    let mut tracks = vec![
        PathBuf::from("smb://nas/media/zebra.mp3"),
        PathBuf::from("/music/apple.mp3"),
    ];
    sort_tracks(&mut tracks, &TagCache::new(), 0);
    assert_eq!(tracks[0], PathBuf::from("/music/apple.mp3"));
    assert_eq!(tracks[1], PathBuf::from("smb://nas/media/zebra.mp3"));
}

// ── Randomizing ───────────────────────────────────────────────────────────────

#[test]
fn shuffle_keeps_every_track_present_exactly_once() {
    let before = ids(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3", "f.mp3"]);
    let mut tracks = before.clone();
    shuffle_tracks(&mut tracks, &mut 0xC0FFEE);
    assert!(same_multiset(&before, &tracks), "got {:?}", names(&tracks));
}

#[test]
fn shuffle_is_reproducible_for_a_given_seed() {
    let before = ids(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
    let mut one = before.clone();
    let mut two = before.clone();
    shuffle_tracks(&mut one, &mut 0xC0FFEE);
    shuffle_tracks(&mut two, &mut 0xC0FFEE);
    assert_eq!(one, two);
}

#[test]
fn shuffle_reorders_and_does_not_always_produce_one_fixed_order() {
    // Two claims in one, because either failure is the same bug seen from both
    // sides: a loop that never swaps (or swaps with 0) leaves every seed
    // returning the input, and a stub returning a constant gives one order.
    let before = ids(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
    let mut moved = 0;
    let mut orders: Vec<Vec<PathBuf>> = Vec::new();
    for mut seed in 1..200u64 {
        let mut tracks = before.clone();
        shuffle_tracks(&mut tracks, &mut seed);
        if tracks != before {
            moved += 1;
        }
        orders.push(tracks);
    }
    // NOT 199: five tracks have 120 permutations, so a correct shuffle hands
    // back the identity for one of them and ~1.7 of 199 seeds are expected to
    // land on it. A floor leaves room for that and still fails any shuffle that
    // barely moves.
    assert!(moved >= 190, "only {moved} of 199 seeds reordered the list");
    // The real discriminator: a no-op or a constant stub yields ONE distinct
    // order, a real shuffle yields most of the 120.
    let mut unique = orders.clone();
    unique.sort();
    unique.dedup();
    assert!(unique.len() > 50, "only {} distinct orders", unique.len());
    for tracks in &orders {
        assert!(same_multiset(&before, tracks));
    }
}

#[test]
fn shuffle_handles_an_empty_and_a_single_track_list() {
    // The `(1..len).rev()` range is what makes these no-ops; a `0..len` loop
    // with a `rand(0)` inside it would panic on the empty list.
    let mut empty: Vec<PathBuf> = Vec::new();
    shuffle_tracks(&mut empty, &mut 0xC0FFEE);
    assert!(empty.is_empty());

    let mut one = ids(&["only.mp3"]);
    shuffle_tracks(&mut one, &mut 0xC0FFEE);
    assert_eq!(names(&one), ["only.mp3"]);
}

// ── Deduplicating ─────────────────────────────────────────────────────────────

#[test]
fn dedup_keeps_the_first_occurrence_and_the_order() {
    let mut tracks = ids(&["a.mp3", "b.mp3"]);
    tracks.push(PathBuf::from("/music/a.mp3"));
    tracks.push(PathBuf::from("/music/c.mp3"));
    dedup(&mut tracks);
    assert_eq!(names(&tracks), ["a.mp3", "b.mp3", "c.mp3"]);
}

#[test]
fn dedup_collapses_repeats_that_are_not_adjacent() {
    // `Vec::dedup` only removes *adjacent* repeats, so an implementation that
    // leans on it keeps this list at four entries.
    let mut tracks = ids(&["a.mp3", "b.mp3", "a.mp3", "b.mp3"]);
    dedup(&mut tracks);
    assert_eq!(names(&tracks), ["a.mp3", "b.mp3"]);
}

#[test]
fn dedup_keeps_a_remote_and_a_local_track_that_merely_look_alike() {
    // Deduplication is on the id, and the ids are stored verbatim — so a share
    // and a local mount of the same file are two tracks, as they are everywhere
    // else in the app.
    let mut tracks = vec![
        PathBuf::from("smb://nas/media/song.mp3"),
        PathBuf::from("/mnt/media/song.mp3"),
    ];
    dedup(&mut tracks);
    assert_eq!(tracks.len(), 2);
}

// ── What the app owes every reorder ───────────────────────────────────────────

/// Four playable 1s WAVs added in an order that none of the ops below leaves
/// where it is, with `a.wav` (index 2) playing. Returns the app and the tracks.
fn scrambled_playlist(name: &str) -> (TestApp, Vec<PathBuf>) {
    let mut t = TestApp::new(name);
    let dir = test_dir(name);
    let tracks: Vec<PathBuf> = ["d.wav", "b.wav", "a.wav", "c.wav"]
        .iter()
        .map(|n| {
            let p = dir.join(n);
            write_wav(&p);
            p
        })
        .collect();
    t.app.add_files(tracks.clone());
    t.app.play_track(2);
    assert_eq!(t.app.current_index(), Some(2), "premise: a.wav is playing");
    (t, tracks)
}

#[test]
fn a_reorder_keeps_the_playing_track_current() {
    // The failure this guards: `current_index` is an index, and a sort/randomize
    // moves what lives under it. A reorder that only shuffled the Vec would
    // leave the app pointing at whatever track slid into the old slot, and
    // `advance` would carry on from there.
    let ops: [(&str, fn(&mut TPlayApp)); 3] = [
        ("sort_by_title", |a| a.sort_playlist(0)),
        ("reverse", TPlayApp::reverse_playlist),
        ("randomize", TPlayApp::randomize_playlist),
    ];
    for (label, op) in ops {
        let (mut t, tracks) = scrambled_playlist(label);
        let playing = tracks[2].clone();
        op(&mut t.app);

        assert_eq!(
            t.app.current_path(),
            Some(playing.as_path()),
            "{label}: a reorder must not change the playing track"
        );
        let idx = t
            .app
            .current_index()
            .unwrap_or_else(|| panic!("{label}: the playlist flow keeps an index"));
        assert_eq!(
            t.app.playlist()[idx],
            playing,
            "{label}: the index must follow the track, not the slot"
        );
        assert!(same_multiset(&tracks, t.app.playlist()), "{label}");
        assert!(
            t.app.playlist_dirty(),
            "{label}: an edit is unsaved until written"
        );
    }
}

#[test]
fn a_reorder_drops_the_shuffle_history() {
    // `played` is a list of positions in the *old* order, so a reorder has to
    // clear it. `has_prev_track` is the one place that reads the history from
    // outside, which makes it the only way to see this headlessly — and it is
    // the button the user would press if the history survived.
    let (mut t, _tracks) = scrambled_playlist("reorder-history");
    t.app.toggle_shuffle();
    t.app.next_track();
    t.app.next_track();
    assert!(
        t.app.has_prev_track(),
        "premise: two plays leave a walk-back"
    );

    t.app.reverse_playlist();
    assert!(
        !t.app.has_prev_track(),
        "a reordered list has no walk-back to offer"
    );
}

#[test]
fn a_reorder_does_not_pull_a_directly_opened_track_into_the_playlist_flow() {
    // `play_file` is the Library's direct open: `current_index = None`, so the
    // track is playing *outside* the playlist and auto-advance does not cascade
    // off it. Re-finding the index by track id must not hand it one, or a
    // reorder would silently enrol the track in the sequential flow.
    let mut t = TestApp::new("reorder-direct");
    let dir = test_dir("reorder-direct");
    let mut paths = Vec::new();
    for n in ["a.wav", "b.wav"] {
        let p = dir.join(n);
        write_wav(&p);
        paths.push(p);
    }
    t.app.add_files(paths.clone());
    t.app.play_file(paths[1].clone());
    assert_eq!(
        t.app.current_index(),
        None,
        "premise: a direct open has no index"
    );

    t.app.sort_playlist(0);
    assert_eq!(t.app.current_index(), None, "a reorder must not invent one");
    assert_eq!(t.app.current_path(), Some(paths[1].as_path()));
}

// ── What the app owes every removal ───────────────────────────────────────────

/// `n` playable 1s WAVs in a temp dir, added to the playlist in order.
fn playable(name: &str, files: &[&str]) -> (TestApp, Vec<PathBuf>) {
    let mut t = TestApp::new(name);
    let dir = test_dir(name);
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|n| {
            let p = dir.join(n);
            write_wav(&p);
            p
        })
        .collect();
    t.app.add_files(paths.clone());
    (t, paths)
}

#[test]
fn removing_the_playing_track_stops_it_playing() {
    // The row is gone, so there is nothing for the sink to be playing *as* any
    // more: it used to keep going with `current_path` cleared, which left audio
    // with no Now Playing entry and no auto-advance to end it.
    let (mut t, paths) = playable("remove-current", &["a.wav", "b.wav"]);
    t.app.play_track(0);
    t.pump(0.05);
    assert_eq!(t.app.current_path(), Some(paths[0].as_path()), "premise");

    t.app.remove_track(0);
    assert_eq!(t.app.current_path(), None);
    assert_eq!(t.app.current_index(), None);
    // Immediate tell: a stopped player reports no duration, and Now Playing's
    // seek bar is enabled off exactly this.
    assert_eq!(
        t.app.total_duration(),
        None,
        "a stopped player must not report a track length"
    );
    // The real tell: the sink itself. `stop` only sets a flag, so the audio has
    // to be pulled before the queue drains — pump a little, then it must be
    // empty rather than still playing a track that is no longer in the list.
    t.pump(0.05);
    assert!(
        t.app.is_empty(),
        "the removed track's audio must stop, not run on unlabelled"
    );
}

#[test]
fn removing_an_unrelated_track_leaves_a_directly_opened_file_alone() {
    // `play_file` is a direct open, so `current_index` is `None` from the start
    // and "the index I had is gone" cannot be told from "there was never an
    // index". Keying the unload on `current_index.is_none()` therefore blanked
    // Now Playing for the file that was actually playing, on removing any row at
    // all — including one that had nothing to do with it.
    let (mut t, paths) = playable("remove-direct", &["a.wav", "b.wav", "c.wav"]);
    t.app.play_file(paths[2].clone());
    assert_eq!(
        t.app.current_index(),
        None,
        "premise: a direct open has no index"
    );
    assert_eq!(t.app.current_path(), Some(paths[2].as_path()), "premise");

    t.app.remove_track(0);
    assert_eq!(
        t.app.current_path(),
        Some(paths[2].as_path()),
        "removing an unrelated row must not touch what is playing"
    );
    assert_eq!(t.app.current_index(), None, "and must not invent an index");
    t.pump(0.05);
    assert!(
        !t.app.is_empty(),
        "the directly-opened track is still in the sink"
    );
}
