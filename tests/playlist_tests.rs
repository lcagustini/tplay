//! Playlist logic tests — shuffle, repeat, add/remove/move, navigation.

use tplay::library::{
    default_playlist_name, playlist_file_name, playlist_json, read_playlist, write_playlist,
    DEFAULT_PLAYLIST_NAME,
};
use tplay::audio::eq::EQ_PRESETS;
use std::path::{Path, PathBuf};
#[path = "common.rs"]
mod common;
use crate::common::test_dir;

#[test]
fn playlist_write_read_roundtrip() {
    let dir = test_dir("playlist_write_read_roundtrip");
    let file = dir.join("test.tplay");

    let tracks = vec![
        dir.join("track1.mp3"),
        PathBuf::from("relative/track2.ogg"),
    ];

    write_playlist(&file, &tracks).unwrap();
    let back = read_playlist(&file, &dir).unwrap();

    assert_eq!(back[0], dir.join("track1.mp3"));
    assert_eq!(back[1], dir.join("relative/track2.ogg"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn playlist_read_malformed_returns_none() {
    let dir = test_dir("playlist_read_malformed_returns_none");
    let file = dir.join("bad.tplay");

    std::fs::write(&file, "not json").unwrap();
    assert!(read_playlist(&file, &dir).is_none());

    std::fs::write(&file, "{}").unwrap(); // valid json, wrong structure
    assert!(read_playlist(&file, &dir).is_none());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn playlist_relative_paths_resolve_against_playlist_dir() {
    let dir = test_dir("playlist_relative_paths_resolve_against_playlist_dir");
    let subdir = dir.join("sub");
    std::fs::create_dir_all(&subdir).unwrap();
    let file = subdir.join("playlist.tplay");

    let tracks = vec![
        PathBuf::from("track1.mp3"),
        PathBuf::from("../track2.flac"),
    ];

    write_playlist(&file, &tracks).unwrap();
    let back = read_playlist(&file, &subdir).unwrap();

    // Relative paths are joined with the base but not canonicalized
    assert_eq!(back[0], subdir.join("track1.mp3"));
    assert_eq!(back[1], subdir.join("../track2.flac"));

    std::fs::remove_dir_all(&dir).unwrap();
}

// ── Remote (SMB) playlist support ─────────────────────────────────────

/// The load-bearing case: a playlist read from a share contains `smb://` URIs,
/// and `Path::is_relative` is TRUE for those (no leading `/`). Keying off
/// `is_relative` would join every remote track onto the base and turn
/// `smb://nas/m/x.mp3` into `smb://nas/m/smb://nas/m/x.mp3`.
#[test]
fn smb_uris_are_not_joined_onto_the_base() {
    let dir = test_dir("smb_uris_are_not_joined_onto_the_base");
    let file = dir.join("remote.tplay");
    let share = "smb://192.168.15.59/newhd/music";

    let tracks = vec![
        PathBuf::from("smb://192.168.15.59/newhd/music/a.mp3"),
        PathBuf::from("smb://192.168.15.59/newhd/other/b.flac"),
        dir.join("local.wav"),
    ];
    write_playlist(&file, &tracks).unwrap();
    let back = read_playlist(&file, Path::new(share)).unwrap();

    assert_eq!(back[0], tracks[0], "smb URI must survive verbatim");
    assert_eq!(back[1], tracks[1], "smb URI on another share must survive");
    assert_eq!(back[2], dir.join("local.wav"), "absolute local path untouched");

    std::fs::remove_dir_all(&dir).unwrap();
}

/// A remote playlist is read from its SPOOL CACHE copy, so the base must be
/// passed explicitly as the share directory — resolving against the cache file's
/// own parent would silently drop every relative track.
#[test]
fn relative_entries_resolve_against_a_share_uri_base() {
    let dir = test_dir("relative_entries_resolve_against_a_share_uri_base");
    let cache_copy = dir.join("spool-cache-copy.tplay");
    let share_dir = "smb://nas/media/albums";

    write_playlist(&cache_copy, &[PathBuf::from("01.mp3"), PathBuf::from("sub/02.mp3")]).unwrap();

    // The WRONG base (the cache file's own directory) would yield local paths.
    let wrong = read_playlist(&cache_copy, cache_copy.parent().unwrap()).unwrap();
    assert!(wrong[0].starts_with(dir.to_str().unwrap()), "premise: cache-dir base is wrong");

    // The right base — the share directory the playlist was browsed at.
    let back = read_playlist(&cache_copy, Path::new(share_dir)).unwrap();
    assert_eq!(back[0], PathBuf::from("smb://nas/media/albums/01.mp3"));
    assert_eq!(back[1], PathBuf::from("smb://nas/media/albums/sub/02.mp3"));

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Saves write full `smb://` URIs, so serialize→read must be lossless for a
/// mixed local/remote playlist (the shape a share playlist actually has).
#[test]
fn mixed_playlist_roundtrips_through_json() {
    let dir = test_dir("mixed_playlist_roundtrips_through_json");
    let file = dir.join("mixed.tplay");
    let tracks = vec![
        PathBuf::from("/home/lucas/Music/local.mp3"),
        PathBuf::from("smb://nas/share/remote.mp3"),
    ];

    let json = playlist_json(&tracks).unwrap();
    assert!(json.contains("smb://nas/share/remote.mp3"), "URIs are stored in full");

    std::fs::write(&file, &json).unwrap();
    let back = read_playlist(&file, &dir).unwrap();
    assert_eq!(back, tracks, "mixed playlist must survive a save/load cycle");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn default_playlist_name_keeps_the_tracked_stem() {
    // A loaded playlist keeps its name when saved somewhere new.
    assert_eq!(default_playlist_name(Some(Path::new("/music/road.trip.tplay"))), "road.trip.tplay");
    // Remote targets parse too — only the last segment matters.
    assert_eq!(
        default_playlist_name(Some(Path::new("smb://nas/share/mix.tplay"))),
        "mix.tplay"
    );
    // No tracked file, or a name with no stem to carry over.
    assert_eq!(default_playlist_name(None), DEFAULT_PLAYLIST_NAME);
    assert_eq!(default_playlist_name(Some(Path::new(".tplay"))), DEFAULT_PLAYLIST_NAME);
}

#[test]
fn playlist_file_name_appends_the_extension_and_strips_separators() {
    // The extension is implied — users type a bare name.
    assert_eq!(playlist_file_name("mix").as_deref(), Some("mix.tplay"));
    // Already correct, left alone.
    assert_eq!(playlist_file_name("mix.tplay").as_deref(), Some("mix.tplay"));
    // Case-insensitive: the extension check follows is_playlist.
    assert_eq!(playlist_file_name("mix.TPLAY").as_deref(), Some("mix.TPLAY"));
    // A typed path is sanitized, not sent to the server as a bogus path.
    assert_eq!(playlist_file_name("a/b").as_deref(), Some("a_b.tplay"));
    assert_eq!(playlist_file_name("a\\b").as_deref(), Some("a_b.tplay"));
    // Blank input cancels rather than writing a nameless file.
    assert_eq!(playlist_file_name(""), None);
    assert_eq!(playlist_file_name("   "), None);
    // A name that is only an extension would be a stemless hidden file.
    assert_eq!(playlist_file_name(".tplay").as_deref(), Some(DEFAULT_PLAYLIST_NAME));
}

// ── Shuffle logic tests ────────────────────────────────────────────────

/// Replicates TPlayApp::next_track_index shuffle logic for testing. That is
/// `peek_next_index` + `commit_next_index` composed — the split itself, and the
/// guarantee that a peek mutates nothing, are pinned in playlist_shuffle.rs.
fn shuffle_next(len: usize, played: &mut Vec<usize>, rng_state: &mut u64, repeat: bool) -> Option<usize> {
    if len == 0 { return None; }
    if len == 1 { return repeat.then_some(0); }

    let unplayed: Vec<usize> = (0..len).filter(|i| !played.contains(i)).collect();
    if unplayed.is_empty() {
        if repeat {
            played.clear();
            let idx = (xor_shift(rng_state) as usize) % len;
            played.push(idx);
            return Some(idx);
        }
        return None;
    }
    let idx = unplayed[(xor_shift(rng_state) as usize) % unplayed.len()];
    played.push(idx);
    Some(idx)
}

/// Replicates TPlayApp::prev_track_index shuffle logic for testing. That is
/// `peek_prev_index` + `commit_prev` composed; the split, the idempotence it
/// buys and the empty-history case it fixes are pinned in playlist_shuffle.rs.
fn shuffle_prev(len: usize, played: &mut Vec<usize>, repeat: bool) -> Option<usize> {
    if len == 0 { return None; }
    if len == 1 { return repeat.then_some(0); }

    let idx = if played.len() > 1 {
        played.get(played.len() - 2).copied()
    } else if repeat {
        played.last().copied()
    } else {
        None
    };
    if idx.is_some() && len > 1 && played.len() > 1 {
        played.pop();
    }
    idx
}

/// XorShift64 - matches TPlayApp::rand_u64
fn xor_shift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn shuffle_plays_each_track_once_before_repeat() {
    let len = 5;
    let mut played = Vec::new();
    let mut rng = 0xC0FFEE;
    let mut seen = Vec::new();

    for _ in 0..len {
        let idx = shuffle_next(len, &mut played, &mut rng, false).unwrap();
        assert!(!seen.contains(&idx), "duplicate in shuffle cycle: {:?}", played);
        seen.push(idx);
    }
    assert_eq!(seen.len(), len);
    assert_eq!(played.len(), len);
}

#[test]
fn shuffle_exhausted_returns_none_without_repeat() {
    let len = 3;
    let mut played = Vec::new();
    let mut rng = 0xC0FFEE;

    for _ in 0..len {
        shuffle_next(len, &mut played, &mut rng, false);
    }
    assert!(shuffle_next(len, &mut played, &mut rng, false).is_none());
}

#[test]
fn shuffle_exhausted_restarts_with_repeat() {
    let len = 3;
    let mut played = Vec::new();
    let mut rng = 0xC0FFEE;

    for _ in 0..len {
        shuffle_next(len, &mut played, &mut rng, true);
    }
    // Next call should restart cycle
    let idx = shuffle_next(len, &mut played, &mut rng, true);
    assert!(idx.is_some());
    assert_eq!(played.len(), 1); // history cleared, new cycle started
}

#[test]
fn shuffle_prev_pops_history() {
    let len = 5;
    let mut played = Vec::new();
    let mut rng = 0xC0FFEE;

    // Play 3 tracks
    let t1 = shuffle_next(len, &mut played, &mut rng, false).unwrap();
    let t2 = shuffle_next(len, &mut played, &mut rng, false).unwrap();
    let _t3 = shuffle_next(len, &mut played, &mut rng, false).unwrap();

    // Go back
    let prev = shuffle_prev(len, &mut played, false).unwrap();
    assert_eq!(prev, t2);
    assert_eq!(played.len(), 2);

    let prev = shuffle_prev(len, &mut played, false).unwrap();
    assert_eq!(prev, t1);
    assert_eq!(played.len(), 1);

    // At start of history, no more prev
    assert!(shuffle_prev(len, &mut played, false).is_none());
}

#[test]
fn shuffle_prev_loops_with_repeat() {
    let len = 3;
    let mut played = Vec::new();
    let mut rng = 0xC0FFEE;

    let t1 = shuffle_next(len, &mut played, &mut rng, true).unwrap();
    let _ = shuffle_prev(len, &mut played, true); // back to start, history len 1

    // With repeat, prev at start of history wraps to last played
    let prev = shuffle_prev(len, &mut played, true).unwrap();
    assert_eq!(prev, t1); // loops to the only track in history
}

// ── Sequential (non-shuffle) navigation tests ────────────────────────

fn sequential_next(len: usize, current: Option<usize>, repeat: bool) -> Option<usize> {
    if len == 0 { return None; }
    if len == 1 { return repeat.then_some(0); }

    match (repeat, current) {
        (false, Some(i)) if i + 1 < len => Some(i + 1),
        (true, Some(i)) => Some((i + 1) % len),
        (true, None) => Some(0),
        _ => None,
    }
}

fn sequential_prev(len: usize, current: Option<usize>, repeat: bool) -> Option<usize> {
    if len == 0 { return None; }
    if len == 1 { return repeat.then_some(0); }

    match (repeat, current) {
        (false, Some(i)) if i > 0 => Some(i - 1),
        (true, Some(i)) => Some((i + len - 1) % len),
        (true, None) => Some(len - 1),
        _ => None,
    }
}

#[test]
fn sequential_next_wraps_with_repeat() {
    assert_eq!(sequential_next(5, Some(4), true), Some(0));
    assert_eq!(sequential_next(5, Some(2), true), Some(3));
}

#[test]
fn sequential_next_stops_at_end_without_repeat() {
    assert_eq!(sequential_next(5, Some(4), false), None);
    assert_eq!(sequential_next(5, Some(2), false), Some(3));
}

#[test]
fn sequential_prev_wraps_with_repeat() {
    assert_eq!(sequential_prev(5, Some(0), true), Some(4));
    assert_eq!(sequential_prev(5, Some(2), true), Some(1));
}

#[test]
fn sequential_prev_stops_at_start_without_repeat() {
    assert_eq!(sequential_prev(5, Some(0), false), None);
    assert_eq!(sequential_prev(5, Some(2), false), Some(1));
}

#[test]
fn single_track_repeats_only_with_repeat() {
    assert_eq!(sequential_next(1, Some(0), true), Some(0));
    assert_eq!(sequential_next(1, Some(0), false), None);
    assert_eq!(sequential_prev(1, Some(0), true), Some(0));
    assert_eq!(sequential_prev(1, Some(0), false), None);
}

// ── Playlist mutation tests ──────────────────────────────────────────

#[test]
fn remove_track_adjusts_current_index() {
    let mut playlist = vec![
        PathBuf::from("a.mp3"),
        PathBuf::from("b.mp3"),
        PathBuf::from("c.mp3"),
    ];
    let mut current_index = Some(1); // playing b.mp3

    // Remove track before current
    playlist.remove(0);
    if current_index == Some(0) { current_index = None; }
    else if current_index.unwrap() > 0 { current_index = Some(current_index.unwrap() - 1); }
    assert_eq!(current_index, Some(0)); // now points to b.mp3 (was index 1)
    assert_eq!(playlist[0], PathBuf::from("b.mp3"));

    // Remove current track
    playlist.remove(0);
    if current_index == Some(0) { current_index = None; }
    else if current_index.unwrap() > 0 { current_index = Some(current_index.unwrap() - 1); }
    assert_eq!(current_index, None); // current track removed
    assert_eq!(playlist.len(), 1);
}

#[test]
fn move_track_adjusts_current_index() {
    let mut playlist = vec![
        PathBuf::from("a.mp3"),
        PathBuf::from("b.mp3"),
        PathBuf::from("c.mp3"),
        PathBuf::from("d.mp3"),
    ];
    let mut current_index = Some(1); // playing b.mp3

    // Move a (0) to after c (2) -> playlist: b, c, a, d
    let item = playlist.remove(0);
    playlist.insert(2, item);
    if current_index == Some(0) { current_index = Some(2); }
    else if 0 < current_index.unwrap() && current_index.unwrap() <= 2 { current_index = Some(current_index.unwrap() - 1); }
    assert_eq!(current_index, Some(0)); // b.mp3 now at index 0

    // Move d (3) to before b (0) -> playlist: d, b, c, a
    let item = playlist.remove(3);
    playlist.insert(0, item);
    if current_index == Some(3) { current_index = Some(0); }
    else if 0 <= current_index.unwrap() && current_index.unwrap() < 3 { current_index = Some(current_index.unwrap() + 1); }
    assert_eq!(current_index, Some(1)); // b.mp3 now at index 1
}

// ── EQ presets ────────────────────────────────────────────────────────

#[test]
fn eq_presets_have_correct_length() {
    for (name, gains) in EQ_PRESETS {
        assert_eq!(gains.len(), 10, "preset {} has wrong band count", name);
        for &g in &gains {
            assert!((-12.0..=12.0).contains(&g), "preset {} gain {} out of range", name, g);
        }
    }
}

#[test]
fn eq_flat_preset_is_zero_gains() {
    let (name, gains) = EQ_PRESETS[0];
    assert_eq!(name, "Flat");
    assert_eq!(gains, [0.0; 10]);
}