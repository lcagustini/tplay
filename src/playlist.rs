//! Playlist *content* operations — the ordered list's what, not its state.
//!
//! A playlist is a `Vec<PathBuf>` of track ids (see `tracks.rs`): a local path
//! or an `smb://` URI, indistinguishable to everything here. This module is what
//! you can do to that list's contents — sort it, randomize it, drop duplicate
//! paths — and nothing else.
//!
//! Three things are deliberately **not** here, and the split is the point:
//!
//! * **The file formats.** `library::{playlist_text, read_playlist,
//!   write_playlist, is_playlist}` own the wire shapes — `.tplay` (ours) plus
//!   `.m3u`/`.m3u8`/`.pls` (what other players speak). Reading a playlist off
//!   disk is `library`'s job; editing the list in memory is this one's.
//! * **The state.** `TPlayApp` owns the list itself plus everything that has to
//!   be repaired when it changes: `playlist_dirty`, `current_index` (re-found by
//!   track id, so the row that is playing stays current), the shuffle `played`
//!   history, and `cancel_xf()`. Those are state transitions and a sink effect,
//!   so the applying methods live on the app (`sort_playlist` and friends) and
//!   call in here for the content half.
//! * **Shuffle navigation.** `peek_next_index`/`commit_next_index` stay beside
//!   `played` in `app.rs` — the peek/commit split is a load-bearing invariant
//!   documented there. This module owns only the draw.
//!
//! Same split as `transition::arm_plan`: the decision is pure and testable on
//! its own, the effects stay where the state is.

use crate::library::{self, TagCache};
use std::collections::HashSet;
use std::path::PathBuf;

// ── Sorting ───────────────────────────────────────────────────────────────────

/// Sort tracks in place by Library column `col` — the same `SORT_OPTIONS` index
/// the Library header uses, so both lists order identically and neither needs a
/// second copy of the column list.
///
/// A remote track is an `smb://` URI in a `PathBuf`, and `library::sort_key`
/// handles it like any other path (its `file_stem` is the remote name), so a
/// mixed local/remote playlist sorts with no branch here. Untagged tracks fall
/// back to the stem and missing tag values sort last, exactly as they do in the
/// Library — which is what makes the columns usable at all, since the tag scan
/// is still filling in behind a freshly loaded playlist.
///
/// Ascending only: a stable sort, so tracks with equal keys keep their order.
pub fn sort_tracks(tracks: &mut [PathBuf], tags: &TagCache, col: usize) {
    tracks.sort_by_cached_key(|p| {
        library::sort_key(
            &library::Entry {
                path: p.clone(),
                is_dir: false,
            },
            tags.get(p),
            col,
        )
    });
}

// ── Randomizing ───────────────────────────────────────────────────────────────

/// Fisher-Yates over any list, using the app's own shuffle RNG.
///
/// A pull from the unplayed pool cannot randomize a *whole* playlist — it
/// picks one track — so this is a separate algorithm rather than a reuse of
/// `peek_next_index`. Empty and single-item lists are no-ops by the range, not
/// by a guard.
pub fn shuffle_tracks<T>(items: &mut [T], rng: &mut u64) {
    for i in (1..items.len()).rev() {
        items.swap(i, rand_usize(rng, i + 1));
    }
}

// ── Deduplicating ─────────────────────────────────────────────────────────────

/// Drop repeated track ids, keeping the first occurrence and the order.
///
/// A retained `Vec`, not `Vec::dedup`: that only collapses *adjacent* repeats,
/// and the repeats that actually reach a playlist arrive from a loaded file
/// where the same path is listed twice with other entries between them.
pub fn dedup(tracks: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    tracks.retain(|p| seen.insert(p.clone()));
}

// ── The shuffle RNG ───────────────────────────────────────────────────────────

/// Tiny inline RNG (XorShift64) — replaces the fastrand dependency.
///
/// Lives here because every draw it serves is a playlist shuffle: the shuffle
/// pick, `play_first_track`'s opening draw, and `shuffle_tracks`. State 0 is a
/// fixed point of the recurrence (the algorithm's known weakness), so a seed
/// must be non-zero — `TPlayApp` uses `0xC0FFEE`.
pub fn rand_u64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// One draw in `0..max` from the shuffle RNG.
pub fn rand_usize(state: &mut u64, max: usize) -> usize {
    (rand_u64(state) as usize) % max
}
