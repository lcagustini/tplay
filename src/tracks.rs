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
//! path to pass along.
//!
//! The earlier shape resolved in the app and passed the pair around
//! (`load_file_as(local, display)`), which satisfied a weaker, greppable rule
//! ("no `is_remote` in `app.rs`") while actually being the bug's shape: two
//! paths for one track, correct only if the caller does not swap them. It did
//! get swapped — `TPlayApp::play()` handed an `smb://` URI to `File::open`, and
//! the crossfade arm did the same through a builder that `.expect()`ed. Passing
//! a pair is not agnostic; it is the source distinction wearing a disguise. So
//! the resolution lives here and the pair is gone.
//!
//! Greppable form: `app.rs` and `src/gui/` never call `File::open` /
//! `read_info` / `probe_duration` / `read_cover` on a track id, and never bind a
//! resolved path. They use `open` / `info` / `probe` / `cover` / `is_ready`.

use crate::audio;
use crate::library::{self, TrackInfo};
use crate::network;
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
    local_file_in(track, dir).as_deref().and_then(library::read_info)
}

/// `track`'s duration, probed from its file. `None` if it has no bytes on hand
/// or the format does not yield a duration.
pub fn probe(track: &Path) -> Option<Duration> {
    probe_in(track, &network::spool_dir())
}

/// `probe` against an injected spool dir.
pub fn probe_in(track: &Path, dir: &Path) -> Option<Duration> {
    local_file_in(track, dir).as_deref().and_then(audio::probe_duration)
}

/// `track`'s embedded album art, read from its file. `None` if it has no bytes
/// on hand or carries no picture.
pub fn cover(track: &Path) -> Option<Vec<u8>> {
    cover_in(track, &network::spool_dir())
}

/// `cover` against an injected spool dir.
pub fn cover_in(track: &Path, dir: &Path) -> Option<Vec<u8>> {
    local_file_in(track, dir).as_deref().and_then(library::read_cover)
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

/// Split a batch of wanted tracks into its local and remote halves.
///
/// Pure, so it is the tested unit and `TagReader::request` stays a thin wrapper.
/// The halves are genuinely different machines — a thread with an mpsc, an SMB
/// worker command with a reply `Event` — so the split is real; it just does not
/// belong to the caller.
pub fn split_for_tags(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<String>) {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for p in paths {
        if network::is_remote(&p) {
            remote.push(p.to_string_lossy().into_owned());
        } else {
            local.push(p);
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
        let missing: Vec<PathBuf> = wanted
            .iter()
            .filter(|p| !cache.contains_key(*p))
            .cloned()
            .collect();
        let (local, remote) = split_for_tags(missing);

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
