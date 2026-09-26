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

/// Mirror of TPlayApp::peek_next_index (shuffle branch) — pure: it reads
/// `played`/`rng_state` and mutates neither, and the draw comes from a scratch
/// copy so repeated peeks are idempotent and cost no entropy.
fn shuffle_peek(len: usize, played: &[usize], rng_state: u64, repeat: bool) -> Option<(usize, u64)> {
    if len == 0 { return None; }
    if len == 1 { return repeat.then_some(0).map(|i| (i, rng_state)); }

    let unplayed: Vec<usize> = (0..len).filter(|i| !played.contains(i)).collect();
    let mut rng = rng_state;
    let idx = if unplayed.is_empty() {
        repeat.then(|| (xor_shift(&mut rng) as usize) % len)
    } else {
        Some(unplayed[(xor_shift(&mut rng) as usize) % unplayed.len()])
    };
    idx.map(|i| (i, rng))
}

/// Mirror of TPlayApp::commit_next_index — the only writer of `played`.
fn shuffle_commit(len: usize, played: &mut Vec<usize>, rng_state: &mut u64, idx: usize, rng: u64) {
    *rng_state = rng;
    if len < 2 { return; }
    // Exhausted pool + repeat starts a new cycle.
    if (0..len).all(|i| played.contains(&i)) {
        played.clear();
    }
    played.push(idx);
}

/// Mirror of TPlayApp::next_track_index shuffle branch — peek, then commit.
fn shuffle_next(len: usize, played: &mut Vec<usize>, rng_state: &mut u64, repeat: bool) -> Option<usize> {
    let (idx, rng) = shuffle_peek(len, played, *rng_state, repeat)?;
    shuffle_commit(len, played, rng_state, idx, rng);
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

// ── Peek is pure: deliberation must not touch the order ─────────────────────
//
// The crossfade/gapless arm re-evaluates itself on every frame until it
// resolves. It used to call the *picking* function to do that, which appended
// to `played` each time — so the shuffle order was being rewritten by
// deliberation rather than by playback, and a peek that is never committed must
// leave no trace at all.

/// The load-bearing one: a thousand peeks are worth exactly zero peeks.
#[test]
fn peeking_repeatedly_changes_neither_history_nor_rng() {
    let len = 8;
    let played = vec![3, 1];
    let rng = 0xC0FFEE;

    let first = shuffle_peek(len, &played, rng, false).expect("a candidate exists");
    for _ in 0..1000 {
        assert_eq!(
            shuffle_peek(len, &played, rng, false),
            Some(first),
            "a peek must be idempotent — the arm asks every frame"
        );
    }
    // `played` is only ever handed in as a shared slice, so the assertion that
    // matters is the caller's: a peek took `&[usize]`, it cannot have written.
}

/// An armed/skipped arm that never commits leaves the order untouched, so the
/// track that really starts is picked fresh rather than inheriting a stale entry.
#[test]
fn an_uncommitted_peek_leaves_the_order_alone() {
    let len = 6;
    let played = vec![0, 2];
    let rng = 0xC0FFEE;

    // The arm peeks on many frames, then the build fails and it bails.
    for _ in 0..120 {
        shuffle_peek(len, &played, rng, false);
    }
    assert_eq!(played, vec![0, 2], "deliberation must not mark anything played");
    assert_eq!(rng, 0xC0FFEE, "a peek must not consume entropy");
}

/// The other half: a track that really starts advances the cycle exactly once.
#[test]
fn an_armed_track_commits_exactly_one_entry() {
    let len = 6;
    let mut played = vec![0, 2];
    let mut rng = 0xC0FFEE;

    let (peeked, peek_rng) = shuffle_peek(len, &played, rng, false).unwrap();
    for _ in 0..120 {
        shuffle_peek(len, &played, rng, false); // frames before the arm lands
    }
    shuffle_commit(len, &mut played, &mut rng, peeked, peek_rng);

    assert_eq!(played, vec![0, 2, peeked], "one play, one history entry");
    assert_ne!(rng, 0xC0FFEE, "the commit installs the draw the peek made");
    assert!(!played[..2].contains(&peeked), "the new entry is the committed one");
}

/// `prev` pops the history, so a history polluted by deliberation made it jump
/// to tracks that never played. One commit per play keeps it walkable.
#[test]
fn history_length_tracks_plays_not_frames() {
    let len = 5;
    let mut played: Vec<usize> = vec![];
    let mut rng = 0xC0FFEE;

    // 5 tracks × ~2s of 60fps arm-window deliberation, then the track starts.
    for track in 0..len {
        for _ in 0..120 {
            shuffle_peek(len, &played, rng, false);
        }
        shuffle_next(len, &mut played, &mut rng, false).expect("a candidate exists");
        assert_eq!(played.len(), track + 1, "history grows by one per track, not per frame");
    }
}

/// Repeat + an exhausted pool restarts the cycle — on the commit, not on the
/// peek, or the order would reset 60 times a second.
#[test]
fn repeat_restarts_the_cycle_on_commit_not_on_peek() {
    let len = 3;
    let mut played = vec![0, 1, 2]; // exhausted
    let mut rng = 0xC0FFEE;

    for _ in 0..120 {
        shuffle_peek(len, &played, rng, true);
    }
    assert_eq!(played, vec![0, 1, 2], "peeking at an exhausted pool must not clear it");

    let (idx, r) = shuffle_peek(len, &played, rng, true).unwrap();
    shuffle_commit(len, &mut played, &mut rng, idx, r);
    assert_eq!(played, vec![idx], "the commit is what starts the new cycle");
    assert!((0..len).contains(&idx));
}