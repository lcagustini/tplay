//! The library database — `~/.config/tplay/library.json`.
//!
//! Two maps, and **no shared insert path between them**, which is the whole
//! design. `tags` is the tag cache every pane already reads; `stats` is what
//! only this app knows (play count, last played, first seen). The background
//! scan thread fills `tags` by inserting straight into [`TagCache`] — it never
//! sees a `&mut TrackDb`, so it *cannot* overwrite a play count no matter what
//! it reads from a file. An earlier shape merged both into one row type and had
//! a `record()` that copied the mutable fields back; that is a rule a caller can
//! forget, and forgetting it erases a user's history silently. Two maps make it
//! a type error instead.
//!
//! Per-field provenance is the other decision here. **What the file says** —
//! title, artist, album, track number, duration, rating — lives in
//! [`TrackInfo`] and is re-readable from disk at any time. **What we know** —
//! plays, last played, first seen — exists only here, because no tag key is
//! meant to carry it: raising a file's own play counter would mean a disk write
//! on every single play, which fails outright on a read-only mount and is
//! meaningless against a spool-cache copy of a track on a server.
//!
//! `ponytail:` nothing is ever pruned, so an entry for a deleted file lives
//! forever and the file only grows. Pruning needs a directory walk the app does
//! not do, and a stale entry costs bytes rather than correctness. Revisit if
//! the file gets large enough to notice.

use crate::library::{TagCache, TrackInfo};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const FILE: &str = "library.json";

/// What this app knows about a track, as opposed to what its file says.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PlayStats {
    /// Times playback has started on this track.
    pub plays: u32,
    /// Epoch seconds of the last play, `None` if it has never been played.
    pub last_played: Option<u64>,
    /// Epoch seconds of the first [`TrackDb::persist`] that saw this track.
    ///
    /// **Written exactly once, by that one method**, which is why it cannot be
    /// reset by a later save and cannot be skipped by an insert path: a track
    /// that arrives in the cache and is never persisted has no first-seen yet,
    /// and one that has been persisted already has an entry it keeps.
    pub first_seen: u64,
}

impl PlayStats {
    /// The row tooltip's text, or `None` for a track with no play history yet —
    /// an unplayed track has nothing to say, and a tooltip that always appears
    /// teaches the reader to ignore it.
    ///
    /// `now` is a parameter so this stays pure and testable, and so the caller
    /// passes the *same* clock the history was stamped with. That clock has to be
    /// wall-clock epoch seconds: `ctx` time restarts with the process, which
    /// would make every persisted stamp read as decades old.
    pub fn describe(&self, now: u64) -> Option<String> {
        if self.plays == 0 {
            return None;
        }
        let times = match self.plays {
            1 => "Played once".to_string(),
            n => format!("Played {n} times"),
        };
        let last = now.saturating_sub(self.last_played.unwrap_or(self.first_seen));
        Some(format!("{times} · last {}", ago(last)))
    }
}

/// Whole seconds as a short relative phrase. Only four buckets, because the
/// difference between "4 minutes ago" and "6 minutes ago" is not information
/// anybody reads a tooltip for.
fn ago(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    match secs {
        s if s < MIN => "just now".to_string(),
        s if s < HOUR => format!("{} min ago", s / MIN),
        s if s < DAY => format!("{} h ago", s / HOUR),
        s => format!("{} d ago", s / DAY),
    }
}

/// The tag cache plus the per-track stats, and the two files' worth of
/// persistence over them.
///
/// `path` is the file it may write, and **`None` means it may not write at all**
/// — a database nobody was given a location for keeps its history in memory and
/// discards it. That is what makes a test hermetic by construction rather than by
/// remembering to inject a directory: a `TPlayApp` built with a fresh
/// [`TrackDb::new`] physically cannot reach the developer's real library, where
/// the alternative — a `persist_to(path, …)` the *caller* supplies — put one
/// forgotten argument between the app and someone's play history.
#[derive(Default)]
pub struct TrackDb {
    tags: TagCache,
    stats: HashMap<PathBuf, PlayStats>,
    path: Option<PathBuf>,
}

impl TrackDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// The tag cache, shared with every pane. Borrowed, never cloned.
    pub fn cache(&self) -> &TagCache {
        &self.tags
    }

    /// The tag cache, for the two things that write it: the background scan and
    /// the inline reads in `start_track` and the crossfade arm. Both go through
    /// the existing `TagReader`/`&mut TagCache` APIs unchanged — that is the
    /// point of the split, since neither has any business seeing `stats`.
    pub fn cache_mut(&mut self) -> &mut TagCache {
        &mut self.tags
    }

    /// The stats for a track, or `None` when it has never been persisted — which
    /// is the only way a track can be unrated *and* unplayed, and the reason the
    /// two are separate: a rating lives in the file, a play count only here.
    pub fn stats_of(&self, track: &Path) -> Option<&PlayStats> {
        self.stats.get(track)
    }

    /// Record one play. A track with no stats entry yet gets its `first_seen`
    /// stamped now as well, so a play is enough to make a track visible to a
    /// date rule without waiting for a save.
    pub fn note_played(&mut self, track: &Path, at: u64) {
        let s = self.stats.entry(track.to_path_buf()).or_default();
        s.plays = s.plays.saturating_add(1);
        s.last_played = Some(at);
        if s.first_seen == 0 {
            s.first_seen = at;
        }
    }

    /// The file's content, for the caller's write-throttle comparison.
    /// Serialization alone: no stamping, no disk.
    ///
    /// Empty on a serialization failure, which the only failure mode is a track
    /// id that is not valid UTF-8 — serde cannot put one in a JSON string. That
    /// is the safe direction for a content compare: the app sees no change, so it
    /// stops asking. It does mean such a track is never persisted, and one bad
    /// filename in a folder of a thousand costs the whole database its write.
    /// `ponytail:` encode ids as lossy strings if that ever happens in the wild.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&self.file()).unwrap_or_default()
    }

    /// A database bound to a temp dir, for a test that needs its writes to land
    /// somewhere it can read them back.
    pub fn at(p: &Path) -> Self {
        Self {
            path: Some(p.to_path_buf()),
            ..Self::new()
        }
    }

    /// Stamp every track that has never been persisted, then write, and hand
    /// back the string that was written so the caller can keep it as its
    /// baseline.
    ///
    /// The stamp lives here rather than at insert time because this is the one
    /// place no insert path can bypass — and because "first seen" only means
    /// something once it has actually been written down. A database with no path
    /// still stamps, and writes nothing.
    pub fn persist(&mut self, at: u64) -> String {
        self.stamp_first_seen(at);
        match self.path.clone() {
            Some(p) => self.write_to(&p),
            None => self.snapshot(),
        }
    }

    fn stamp_first_seen(&mut self, at: u64) {
        for track in self.tags.keys() {
            self.stats.entry(track.clone()).or_insert(PlayStats {
                first_seen: at,
                ..Default::default()
            });
        }
    }

    fn write_to(&self, path: &Path) -> String {
        let json = self.snapshot();
        // An empty string is a failed serialization (see `snapshot`), never a
        // real database — writing it would replace a whole play history with
        // nothing, and an empty map serializes to `{"tags":{},"stats":{}}`.
        if json.is_empty() {
            return json;
        }
        crate::config::atomic_write(path, &json);
        json
    }

    fn file(&self) -> DbFile {
        DbFile {
            tags: self
                .tags
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            stats: self.stats.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        }
    }

    /// Read library.json. A missing or unreadable file is a first run, not an
    /// error — the cache refills from the folders you browse.
    ///
    /// A *malformed* one is different: it holds play history that exists nowhere
    /// else, so the original is kept beside it rather than overwritten by the
    /// next write. Same contract as `config::load_from`.
    pub fn load() -> Self {
        Self::load_from(path().as_deref())
    }

    /// `load` against an explicit path, so the malformed case is testable
    /// without touching the real config dir.
    pub fn load_from(p: Option<&Path>) -> Self {
        let Some(p) = p else { return Self::new() };
        match std::fs::read_to_string(p) {
            Ok(text) => match serde_json::from_str::<DbFile>(&text) {
                Ok(f) => (f, Some(p.to_path_buf())).into(),
                Err(e) => {
                    let kept = p.with_extension("json.broken");
                    let _ = std::fs::write(&kept, &text);
                    eprintln!(
                        "tplay: {} is malformed ({e}); play history kept at {}",
                        p.display(),
                        kept.display()
                    );
                    Self::at(p)
                }
            },
            // Missing or unreadable: a first run, not damage. Bound to the path
            // anyway, so the first play creates the file.
            Err(_) => Self::at(p),
        }
    }
}

impl From<(DbFile, Option<PathBuf>)> for TrackDb {
    fn from((f, path): (DbFile, Option<PathBuf>)) -> Self {
        Self {
            tags: f.tags.into_iter().collect(),
            stats: f.stats.into_iter().collect(),
            path,
        }
    }
}

/// The on-disk shape.
///
/// `BTreeMap` rather than the `HashMap` the app holds, and that is not tidiness:
/// serde serializes a map in iteration order, and a `HashMap`'s order is
/// randomized per process, so every launch would produce a different string for
/// identical content. The write throttle compares content, so that would make
/// the database look changed on every single start and rewrite the file every
/// time — the same trap `equal_configs_serialize_identically` pins for `Config`.
///
/// Every field carries `#[serde(default)]`, and serde ignores fields it does not
/// know, so a file written by a build with more state (Smart Views, coming next)
/// still loads here and the other way round.
#[derive(Serialize, Deserialize, Default)]
struct DbFile {
    #[serde(default)]
    tags: BTreeMap<PathBuf, TrackInfo>,
    #[serde(default)]
    stats: BTreeMap<PathBuf, PlayStats>,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(FILE))
}
