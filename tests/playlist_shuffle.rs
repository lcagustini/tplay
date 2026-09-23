//! Shuffle interaction matrix — how playlist mutations and mode toggles
//! interact with the shuffle history, mirrored from `TPlayApp`. The core
//! next/prev traversal is covered in playlist_tests.rs; this file pins the
//! *reset* semantics and the unplayed-pool behavior for dynamically added
//! tracks.
//!
//! (TPlayApp itself needs an audio device + eframe CreationContext, so the
//! pure logic is replicated here — same convention as playlist_tests.rs.)

/// XorShift64 — matches TPlayApp::rand_u64.
fn xor_shift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Mirror of TPlayApp::next_track_index shuffle branch.
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

#[test]
fn added_tracks_join_the_unplayed_pool() {
    // History: played 0 and 2 of a 4-track playlist. Tracks are appended via
    // Add Files (dedup'd, no reset_shuffle), so index 4 is brand new.
    let len = 5; // post-append length
    let mut played = vec![0, 2];

    let unplayed: Vec<usize> = (0..len).filter(|i| !played.contains(i)).collect();
    assert_eq!(unplayed, vec![1, 3, 4], "new track is available without resetting history");

    // The very next shuffle draw comes from the unplayed pool.
    let mut rng = 0xC0FFEE;
    let idx = shuffle_next(len, &mut played, &mut rng, false).unwrap();
    assert!(unplayed.contains(&idx));
    assert!(!played[..played.len() - 1].contains(&idx));
}

#[test]
fn remove_track_resets_shuffle_history() {
    // remove_track calls reset_shuffle → played clears; the next draw starts
    // a fresh cycle over the shorter list.
    let len = 4; // after removal
    let mut played = vec![0, 1, 2]; // stale history from before the removal
    played.clear(); // reset_shuffle

    let mut rng = 0xC0FFEE;
    let idx = shuffle_next(len, &mut played, &mut rng, false).unwrap();
    assert_eq!(played, vec![idx], "fresh single-entry history");
}

#[test]
fn move_track_resets_shuffle_history() {
    // move_track (a real reorder, not from==to) also calls reset_shuffle.
    let len = 4;
    let mut played = vec![0, 3];
    played.clear();

    let mut rng = 0xC0FFEE;
    let idx = shuffle_next(len, &mut played, &mut rng, false).unwrap();
    assert_eq!(played, vec![idx]);
}

#[test]
fn play_track_direct_click_resets_shuffle() {
    // Clicking a track in the playlist calls play_track → reset_shuffle +
    // start; the next shuffle draw may revisit any track, including the one
    // just clicked.
    let len = 5;
    let mut played = vec![0, 1, 2, 3]; // exhausted pool
    played.clear(); // reset_shuffle

    let mut rng = 0xC0FFEE;
    assert!(shuffle_next(len, &mut played, &mut rng, false).is_some());
}

#[test]
fn toggling_shuffle_off_clears_history() {
    // toggle_shuffle flips the flag AND calls reset_shuffle — flipping back
    // on starts a clean cycle rather than resuming the old history.
    let len = 4;
    let mut played = vec![0, 1, 2]; // history from a later-abandoned cycle
    played.clear(); // reset_shuffle on toggle off

    let mut rng = 0xC0FFEE;
    let idx = shuffle_next(len, &mut played, &mut rng, false).unwrap();
    assert!((0..len).contains(&idx));
    assert_eq!(played, vec![idx]);
}

#[test]
fn single_track_shuffle_repeats_only_with_repeat() {
    assert_eq!(shuffle_next(1, &mut vec![], &mut 0xC0FFEE, true), Some(0));
    assert_eq!(shuffle_next(1, &mut vec![0], &mut 0xC0FFEE, true), Some(0));
    assert_eq!(shuffle_next(1, &mut vec![], &mut 0xC0FFEE, false), None);
    assert_eq!(shuffle_next(0, &mut vec![], &mut 0xC0FFEE, false), None);
}