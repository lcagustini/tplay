//! Track identity — the one place that knows a track can be local or remote.
//!
//! A local track and an `smb://` track differ in exactly one way: **timing**. A
//! local track's bytes are on disk now; a remote track's may be sitting in the
//! spool cache, or may need a fetch first. Everything else — its id, its tags,
//! "does it exist", "give me the file to decode" — is the same operation over a
//! different transport. So the operations live here, uniform, and the transport
//! stays behind them.
//!
//! A `smb://` URI is stored as a `PathBuf` holding the URI, and a URI's last
//! segment is the filename, so it behaves as an id without any help: the
//! `tag_cache` is keyed by it and every reader treats a cached empty
//! `TrackInfo` as "no tags", exactly as it does for a local file.
//!
//! One trap, and it is the reason this module exists: `Path::is_relative` is
//! **true** for an `smb://` URI (anything without a leading `/` counts as
//! relative on Unix), so any code keying off it will join a remote track onto
//! whatever base it is given. `library::read_playlist` is the one place that
//! still branches on `is_remote` itself, because there the question really is
//! about path semantics.

use crate::library::{self, TrackInfo};
use crate::network;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

// ── Resolving a track to bytes on disk ───────────────────────────────────────

/// The local file to read for `track`, for code that needs real **bytes**
/// rather than a playlist entry — album art, a decoder, a probe.
///
/// A remote track has no file at its URI: there is nothing on disk named
/// `smb://…`. So a remote track resolves to its spool-cache copy, and `None`
/// when it has not been spooled yet — there is genuinely nothing local to read.
/// A local path comes back unchanged **whether or not it exists**, so a track
/// that is mid-load still resolves and the caller's own error handling decides
/// what a missing file means.
pub fn local_file(track: &Path) -> Option<PathBuf> {
    local_file_in(track, &network::spool_dir())
}

/// `local_file` against an injected spool dir, so tests stay hermetic.
pub fn local_file_in(track: &Path, dir: &Path) -> Option<PathBuf> {
    if !network::is_remote(track) {
        return Some(track.to_path_buf());
    }
    let local = network::cache_path_in(&track.to_string_lossy(), dir);
    local.is_file().then_some(local)
}

/// The local file for `track` **if it is readable right now** — the
/// pre-buffer question, i.e. can this track be opened synchronously.
///
/// Deliberately separate from `local_file`, which answers a different question.
/// For a *local* track that has no file yet (mid-load, or deleted off disk) the
/// two disagree: art should still be attempted, because the track's identity is
/// known and the caller reports a read failure in its own terms, whereas
/// pre-buffering has nothing to open and must wait. Collapsing them into one
/// function would blank the album art of every track that fails to open, and
/// turn "there is no file to pre-buffer" into "this is a remote track".
pub fn local_file_now(track: &Path) -> Option<PathBuf> {
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
/// playlist should not resurrect a file the user deleted. A remote URI is kept
/// **verbatim**: it cannot be canonicalized (there is no such file), and
/// rewriting it would break the spool-cache key that playback and tagging both
/// look it up by.
pub fn normalize(track: PathBuf) -> Option<PathBuf> {
    if network::is_remote(&track) {
        Some(track)
    } else {
        track.canonicalize().ok()
    }
}

// ── Splitting a batch across the two tag transports ──────────────────────────

/// Split a batch of wanted tracks into the local half and the remote half.
///
/// Pure, so it is the tested unit and the tag reader's `request` stays a thin
/// wrapper. The two halves are genuinely different machines — a background
/// thread with an mpsc, and an SMB worker command with a reply `Event` — so this
/// split is real; it just does not belong to the caller.
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

/// Cap on how many per-track tag failures one batch prints before switching to a
/// count. Browsing a share with bad credentials fails hundreds of rows, and one
/// line each buries the actual problem under the retries.
const MAX_TAG_FAILURES_LOGGED: usize = 3;

/// Reads tags into the one shared cache, whatever transport each track needs.
///
/// The two transports are genuinely different machines — a background thread
/// with an mpsc, and an SMB worker command with a reply `Event` — and this type
/// does not pretend otherwise. What it removes is the *caller's* need to know
/// which is which: the app asks for tracks and drains results, and the split
/// between the two lives here.
///
/// One local scan runs at a time. A new request replaces the in-flight one, and
/// that is safe because the cache is per-track: a dropped scan costs a rescan of
/// its own files, nothing more.
pub struct TagReader {
    /// Receiver for the in-flight local `scan_files` thread. `None` when no
    /// local scan is running, which is also what "Scanning…" reads as done.
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

    /// Ask for tags for any of `wanted` the cache doesn't have yet.
    ///
    /// Returns whether a local scan was started, so the caller knows to ask for
    /// a repaint and start draining. Safe to call every frame: cached tracks are
    /// skipped, and the network side keeps its own in-flight set, so a batch
    /// that is still running is not re-queued.
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
    /// arrived or the scan ended, so the caller can request a repaint. Drops the
    /// receiver once the thread finishes, which is what clears "Scanning…".
    pub fn drain_into(&mut self, cache: &mut HashMap<PathBuf, TrackInfo>) -> bool {
        let (mut ended, mut new) = (false, false);
        if let Some(rx) = &self.local_rx {
            loop {
                match rx.try_recv() {
                    Ok((path, info)) => {
                        cache.insert(path, info);
                        new = true;
                    }
                    // The thread is still working — leave the receiver in place.
                    Err(TryRecvError::Empty) => break,
                    // The thread is done. The scan is finished whether or not
                    // its last result was queued, so drop the receiver.
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
    /// the whole point:
    ///
    /// * `Ok` is cached even when every field is blank. That is the definitive
    ///   "read it, it has no tags" answer, `title_or_stem` falls back to the
    ///   stem on an empty title so it displays exactly like no cache entry, and
    ///   it is what stops a folder of untagged files from re-requesting forever.
    /// * `Err` is **not** cached. A failed transfer says nothing about the
    ///   file's tags — it may be a blip, or expired credentials — so caching it
    ///   as untagged would blank the row for the rest of the session. Leaving it
    ///   out is what makes it retryable; `Network`'s attempt count is what stops
    ///   that retrying forever.
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
