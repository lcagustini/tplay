//! Track identity — the one place that knows a track can be local or remote.
//!
//! A local track and an `smb://` track differ in exactly one way: **timing**. A
//! local track's bytes are on disk now; a remote track's may be in the spool
//! cache, or may need a fetch first. Everything else — id, tags, "does it
//! exist", "give me the file to decode" — is the same operation over a different
//! transport, so the operations live here, uniform, with the transport behind
//! them.
//!
//! An `smb://` URI is stored as a `PathBuf` holding the URI, and a URI's last
//! segment is the filename, so it behaves as an id unaided: the `tag_cache` is
//! keyed by it, and every reader treats a cached empty `TrackInfo` as "no tags"
//! exactly as it does for a local file.
//!
//! One trap, and the reason this module exists: `Path::is_relative` is **true**
//! for an `smb://` URI (anything without a leading `/` counts as relative on
//! Unix), so code keying off it joins a remote track onto whatever base it is
//! given. `library::read_playlist` is the one place still branching on
//! `is_remote` itself, because there the question really is about path
//! semantics.
//!
//! ## The rule this module exists to enforce
//!
//! **The app holds one path per track, and never learns that a local path
//! exists.** Everything below takes a track id and returns the *thing asked
//! for* — a `File`, a `TrackInfo`, a duration, art bytes — never a resolved
//! path to pass along. The history of the two-path form this replaced, and the
//! two silent crashes it caused, is under **Tracks: one surface per source** in
//! AGENTS.md.
//!
//! [`write_tags`] is the one operation that goes the other way, and it sits here
//! for the same reason the readers do: whether a track *can* be written is a fact
//! about transports, and a remote track's spool-cache copy is not the track.
//!
//! Greppable form: `app.rs` and `src/gui/` never call `File::open` /
//! `read_info` / `probe_duration` / `read_cover` on a track id, and never bind a
//! resolved path. They use `open` / `info` / `probe` / `cover` / `is_ready` /
//! `write_tags`.

use crate::audio;
use crate::library::{self, TagCache, TrackInfo};
use crate::network;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::tag::items::popularimeter::{Popularimeter, StarRating};
use lofty::tag::{ItemKey, Tag, TagType};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

// ── Reading a track ──────────────────────────────────────────────────────────

/// Open `track`'s file for decoding, or `None` if it has no bytes on hand.
///
/// Resolves internally, which is the point: a caller that passes a raw id to
/// `File::open` gets an `smb://` URI, and a remote track has no file at its URI.
pub fn open(track: &Path) -> Option<File> {
    open_in(track, &network::spool_dir())
}

/// `open` against an injected spool dir.
pub fn open_in(track: &Path, dir: &Path) -> Option<File> {
    local_file_in(track, dir).and_then(|p| File::open(p).ok())
}

/// `track`'s tags and duration, read from its file. `None` if it has no bytes
/// on hand; a file with no tags is `Some` with blank fields — a final answer,
/// not a missing one (see `TagReader::absorb`).
pub fn info(track: &Path) -> Option<TrackInfo> {
    info_in(track, &network::spool_dir())
}

/// `info` against an injected spool dir.
pub fn info_in(track: &Path, dir: &Path) -> Option<TrackInfo> {
    local_file_in(track, dir)
        .as_deref()
        .and_then(library::read_info)
}

/// `track`'s duration, probed from its file. `None` if it has no bytes on hand
/// or the format does not yield a duration.
pub fn probe(track: &Path) -> Option<Duration> {
    probe_in(track, &network::spool_dir())
}

/// `probe` against an injected spool dir.
pub fn probe_in(track: &Path, dir: &Path) -> Option<Duration> {
    local_file_in(track, dir)
        .as_deref()
        .and_then(audio::probe_duration)
}

/// `track`'s embedded album art, read from its file. `None` if it has no bytes
/// on hand or carries no picture.
pub fn cover(track: &Path) -> Option<Vec<u8>> {
    cover_in(track, &network::spool_dir())
}

/// `cover` against an injected spool dir.
pub fn cover_in(track: &Path, dir: &Path) -> Option<Vec<u8>> {
    local_file_in(track, dir)
        .as_deref()
        .and_then(library::read_cover)
}

/// Can `track`'s bytes be opened synchronously — the pre-buffer question.
///
/// A predicate, not a path, so a caller asking it has no way to keep the
/// resolved path and pass it somewhere it should not go. See `local_file_now`
/// for why this is not just `open(...).is_some()`.
pub fn is_ready(track: &Path) -> bool {
    local_file_now(track).is_some()
}

// ── Resolving a track to bytes on disk ───────────────────────────────────────

/// The local file behind `track`: a remote track's spool-cache copy, or `None`
/// if it has not been spooled yet. A local path comes back unchanged **whether
/// or not it exists**, so a mid-load track still resolves and the reader reports
/// its own failure rather than being silently skipped.
///
/// The primitive every reader here is built on, `pub` only so tests can inject a
/// spool dir. Callers outside this module use
/// `open`/`info`/`probe`/`cover`/`is_ready` and never see a resolved path.
pub fn local_file_in(track: &Path, dir: &Path) -> Option<PathBuf> {
    if !network::is_remote(track) {
        return Some(track.to_path_buf());
    }
    let local = network::cache_path_in(&track.to_string_lossy(), dir);
    local.is_file().then_some(local)
}

/// The local file for `track` **if it is readable right now** — the pre-buffer
/// question, i.e. can this track be opened synchronously.
///
/// Deliberately separate from `local_file_in`, which answers a different
/// question. For a *local* track with no file yet (mid-load, or deleted off
/// disk) the two disagree: art and tags should still be attempted — the track's
/// identity is known and the reader reports the failure in its own terms —
/// whereas pre-buffering has nothing to open and must wait. Collapsing them
/// would blank the art and tags of every track that fails to open, and turn
/// "there is no file to pre-buffer" into "this is a remote track".
///
/// Private because `is_ready` is its public form: a caller asking the pre-buffer
/// question wants a yes/no, not a path it could pass somewhere it should not.
fn local_file_now(track: &Path) -> Option<PathBuf> {
    local_file_now_in(track, &network::spool_dir())
}

/// `local_file_now` against an injected spool dir.
pub fn local_file_now_in(track: &Path, dir: &Path) -> Option<PathBuf> {
    local_file_in(track, dir).filter(|f| f.is_file())
}

// ── Writing a track's tags ────────────────────────────────────────────────────

/// The tag changes one write applies. `None` leaves a field alone; an **empty**
/// string removes that tag rather than writing a blank one.
///
/// This struct is the whole reason the app has one tag-write path. The rating
/// arrived first and the text fields came later, and neither added a second
/// function: a new editable field is one line here and one line in `write_tags`.
/// `Default` is derived, so a caller writing only the rating spells
/// `..Default::default()` — which is also what makes that the *common* case, and
/// the reason the fields are ordered rating-last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edit<'a> {
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub track_no: Option<&'a str>,
    /// `0` clears the rating; `1`–`5` set it. Clamped, because a `StarRating`
    /// holds nothing else and a hand-edited value must not reach the file.
    pub rating: Option<u8>,
}

/// Apply `edit` to `track`'s own file and hand back what the file then says.
///
/// **The one tag-write path**, and the only writer here — the module's readers
/// return the thing asked for, and this is the one operation that changes
/// something. It lives here for the same reason they do: a track can be local or
/// remote, and the answer to "can this be written" is a fact about transports
/// that only this module knows. A remote track is refused rather than pointed at
/// its spool-cache copy — that copy is a cache of bytes, not the track, so a
/// rating written there is invisible to the server and lost at the next eviction.
///
/// Returns the **file's own** `TrackInfo` read back after the write, not the
/// fields that were asked for: the caller's cache then holds what is on disk even
/// if a format silently declined one of them.
pub fn write_tags(track: &Path, edit: &Edit) -> Result<TrackInfo, String> {
    if network::is_remote(track) {
        return Err("a track on a server has no file of its own to write".into());
    }
    let mut tagged =
        lofty::read_from_path(track).map_err(|e| format!("{}: {e}", track.display()))?;
    {
        // A file with no tags at all has nothing to insert into, and that is the
        // normal state of a fresh rip, so the tag is created rather than the
        // write refused.
        if tagged.primary_tag().is_none() && tagged.first_tag().is_none() {
            let file = FileType::from_path(track)
                .ok_or_else(|| format!("{}: unknown file type", track.display()))?;
            for kind in std::iter::once(file.primary_tag_type()).chain(WRITABLE_TAGS) {
                if !file.tag_support(kind).is_writable() {
                    continue;
                }
                // Two lofty facts make this a loop with a check rather than one
                // computed choice. `tag_support` is optimistic: it reports ID3v2
                // as writable for a WAV, which the RIFF writer then refuses. And
                // `insert_tag` hands back the tag it *replaced*, so its result
                // cannot distinguish a refusal from a success — the only honest
                // check is what the file now carries.
                tagged.insert_tag(Tag::new(kind));
                if !tagged.tags().is_empty() {
                    break;
                }
            }
            if tagged.tags().is_empty() {
                return Err(format!("{}: no writable tag type", track.display()));
            }
        }
        // `match`, not `or_else`: the closure form borrows `tagged` a second
        // time while the first borrow is live.
        let tag = match tagged.primary_tag_mut() {
            Some(t) => t,
            None => tagged
                .first_tag_mut()
                .ok_or_else(|| format!("{}: no supported tag type", track.display()))?,
        };
        if let Some(n) = edit.rating {
            if n == 0 {
                // A `Popularimeter` cannot express "unrated" — its scale starts at
                // one star — so clearing is a removal, not a zero.
                tag.remove_key(ItemKey::Popularimeter);
            } else {
                tag.insert_text(
                    ItemKey::Popularimeter,
                    Popularimeter::musicbee(star_rating(n), 0).to_string(),
                );
            }
        }
        for (key, value) in [
            (ItemKey::TrackTitle, edit.title),
            (ItemKey::TrackArtist, edit.artist),
            (ItemKey::AlbumTitle, edit.album),
            (ItemKey::TrackNumber, edit.track_no),
        ] {
            let Some(value) = value else { continue };
            if value.is_empty() {
                // An empty text frame is not "no value" to every reader: some
                // display a blank cell, which is exactly what a missing tag avoids.
                // Removing is also what "cleared the field" means to the user.
                tag.remove_key(key);
            } else {
                tag.insert_text(key, value.to_string());
            }
        }
    }
    tagged
        .save_to_path(track, WriteOptions::default())
        .map_err(|e| format!("{}: {e}", track.display()))?;
    library::read_info(track)
        .ok_or_else(|| format!("{}: unreadable after writing", track.display()))
}

/// The tag kinds a write will create, in preference order. Container-specific, so
/// this is a short list of the ones this app can read back — not every type lofty
/// knows.
const WRITABLE_TAGS: [TagType; 4] = [
    TagType::Id3v2,
    TagType::VorbisComments,
    TagType::Mp4Ilst,
    TagType::RiffInfo,
];

/// A `u8` rating as the enum lofty stores. Saturating rather than panicking: a
/// rating arrives from a UI and from `apply_edit`'s public signature, and a
/// clamp is the only thing between that and a file rewrite.
fn star_rating(n: u8) -> StarRating {
    match n {
        0 | 1 => StarRating::One,
        2 => StarRating::Two,
        3 => StarRating::Three,
        4 => StarRating::Four,
        _ => StarRating::Five,
    }
}

// ── Normalizing a playlist entry ─────────────────────────────────────────────

/// Resolve a playlist entry to the id the rest of the app stores.
///
/// A local path is canonicalized, and dropped if it no longer exists — a
/// playlist must not resurrect a deleted file. A remote URI is kept
/// **verbatim**: it cannot be canonicalized (there is no such file), and
/// rewriting it would break the spool-cache key playback and tagging both use.
pub fn normalize(track: PathBuf) -> Option<PathBuf> {
    if network::is_remote(&track) {
        Some(track)
    } else {
        track.canonicalize().ok()
    }
}

// ── Splitting a batch across the two tag transports ──────────────────────────

/// Split a batch of wanted tracks into its local and remote halves, skipping any
/// the cache already has.
///
/// Pure, so it is the tested unit and `TagReader::request` stays a thin wrapper.
/// The halves are genuinely different machines — a thread with an mpsc, an SMB
/// worker command with a reply `Event` — so the split is real; it just does not
/// belong to the caller.
///
/// The cache test lives *here* rather than in the caller, and that is the point:
/// the share browser asks for every uncached track every frame, and filtering into
/// a `missing` vector first meant a `PathBuf` clone per uncached row per frame
/// before the split even started. One pass, and a remote row costs the one
/// `String` the worker command needs rather than a clone plus a String.
///
/// The local half still clones, because it moves into a spawned thread and has to
/// be `'static`. One clone per uncached row is the floor here without changing
/// `scan_files` to take references, which would just move the clone.
pub fn split_for_tags(wanted: &[PathBuf], cache: &TagCache) -> (Vec<PathBuf>, Vec<String>) {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for p in wanted {
        if cache.contains_key(p) {
            continue;
        }
        if network::is_remote(p) {
            remote.push(p.to_string_lossy().into_owned());
        } else {
            local.push(p.clone());
        }
    }
    (local, remote)
}

// ── Reading tags ─────────────────────────────────────────────────────────────

/// Per-batch cap on printed tag failures before switching to a count. Bad
/// credentials fail hundreds of rows, and one line each buries the problem.
const MAX_TAG_FAILURES_LOGGED: usize = 3;

/// Reads tags into the one shared cache, whatever transport each track needs.
///
/// The two transports are genuinely different machines — a thread with an mpsc,
/// an SMB worker command with a reply `Event` — and this type does not pretend
/// otherwise. What it removes is the *caller's* need to know which is which: the
/// app asks for tracks and drains results, and the split lives here.
///
/// One local scan at a time. A new request replaces the in-flight one, safe
/// because the cache is per-track: a dropped scan costs a rescan of its own
/// files, nothing more.
pub struct TagReader {
    /// Receiver for the in-flight `scan_files` thread. `None` = no local scan
    /// running, which is also what "Scanning…" reads as done.
    local_rx: Option<Receiver<(PathBuf, TrackInfo)>>,
}

impl Default for TagReader {
    fn default() -> Self {
        Self::new()
    }
}

impl TagReader {
    pub fn new() -> Self {
        Self { local_rx: None }
    }

    /// Ask for tags for any of `wanted` the cache lacks.
    ///
    /// Returns whether a local scan started, so the caller can request a repaint
    /// and start draining. Safe every frame: cached tracks are skipped and the
    /// network side keeps its own in-flight set, so a running batch is not
    /// re-queued.
    pub fn request(
        &mut self,
        cache: &HashMap<PathBuf, TrackInfo>,
        network: &mut network::Network,
        wanted: &[PathBuf],
    ) -> bool {
        let (local, remote) = split_for_tags(wanted, cache);

        if !remote.is_empty() {
            network.fetch_tags(remote);
        }
        if local.is_empty() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || library::scan_files(local, tx));
        self.local_rx = Some(rx);
        true
    }

    /// Drain finished local results into `cache`. Returns whether anything
    /// arrived or the scan ended, so the caller can request a repaint. Dropping
    /// the receiver when the thread finishes is what clears "Scanning…".
    pub fn drain_into(&mut self, cache: &mut HashMap<PathBuf, TrackInfo>) -> bool {
        let (mut ended, mut new) = (false, false);
        if let Some(rx) = &self.local_rx {
            loop {
                match rx.try_recv() {
                    Ok((path, info)) => {
                        cache.insert(path, info);
                        new = true;
                    }
                    // The thread is still working — keep the receiver.
                    Err(TryRecvError::Empty) => break,
                    // The thread is done: the scan is finished whether or not its
                    // last result was queued, so drop the receiver.
                    Err(TryRecvError::Disconnected) => {
                        ended = true;
                        break;
                    }
                }
            }
        }
        if ended {
            self.local_rx = None;
        }
        new || ended
    }

    /// Apply a batch of remote tag results to `cache`, keyed by URI.
    ///
    /// The two outcomes are deliberately **not** symmetric, and the asymmetry is
    /// the point:
    ///
    /// * `Ok` is cached even when every field is blank: the definitive "read it,
    ///   it has no tags" answer. `title_or_stem` falls back to the stem on an
    ///   empty title so it displays exactly like no cache entry, and it is what
    ///   stops a folder of untagged files re-requesting forever.
    /// * `Err` is **not** cached. A failed transfer says nothing about the tags
    ///   — a blip, expired credentials — so caching it as untagged would blank
    ///   the row for the rest of the session. Leaving it out is what makes it
    ///   retryable; `Network`'s attempt count stops that retrying forever.
    pub fn absorb(
        &self,
        cache: &mut HashMap<PathBuf, TrackInfo>,
        results: Vec<(String, Result<TrackInfo, String>)>,
    ) {
        let mut failed = 0usize;
        for (uri, res) in results {
            match res {
                Ok(info) => {
                    cache.insert(PathBuf::from(uri), info);
                }
                Err(e) => {
                    failed += 1;
                    if failed <= MAX_TAG_FAILURES_LOGGED {
                        eprintln!("tplay: could not read tags for {uri}: {e}");
                    }
                }
            }
        }
        if failed > MAX_TAG_FAILURES_LOGGED {
            eprintln!(
                "tplay: {failed} more remote track(s) failed to read; \
                 see the first error above"
            );
        }
    }
}
