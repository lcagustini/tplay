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

use crate::library::{self, TagCache, TrackInfo};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const FILE: &str = "library.json";

/// Seconds in a day, for the one date rule. Not a `Duration` because it divides
/// rather than measures.
const DAY_SECS: u64 = 86_400;

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

/// One saved query: a set of optional constraints, AND-ed together.
///
/// A struct of `Option`s rather than an enum, for two reasons. It is *more*
/// expressive — "rated 4+ **and** played 3+" is one rule rather than a new
/// variant — and it is forward compatible: `#[serde(default)]` on the struct means
/// a `library.json` written by a build carrying a constraint this one has never
/// heard of still loads. A serde enum fails on the unknown variant instead, and
/// that failure costs the whole file, play history included.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Rule {
    pub rating_at_least: Option<u8>,
    /// `Some(0)` is "unrated", so this doubles as the unrated filter.
    pub rating_at_most: Option<u8>,
    pub played_at_least: Option<u32>,
    /// The N most-played tracks. A **ranking** rather than a filter, so unlike the
    /// rest it changes the result's order as well as its membership.
    pub top_played: Option<usize>,
    pub added_within_days: Option<u32>,
}

impl Rule {
    /// True when no constraint is set at all.
    ///
    /// Such a rule matches **nothing**, not everything — see [`TrackDb::select`].
    pub fn is_empty(&self) -> bool {
        self.rating_at_least.is_none()
            && self.rating_at_most.is_none()
            && self.played_at_least.is_none()
            && self.top_played.is_none()
            && self.added_within_days.is_none()
    }
}

/// A named rule, as saved in `library.json` and listed in the Library sidebar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmartView {
    pub name: String,
    #[serde(default)]
    pub rule: Rule,
}

/// The view kinds the sidebar offers, each a preset that **writes** a `Rule`.
///
/// Data, not a match, so a new constraint is one field on `Rule` plus one line
/// here and the menu needs no new branch. The name carries the parameters
/// ("Rated 4+"), which is also what lets two views of the same kind coexist.
pub fn presets() -> Vec<(&'static str, SmartView)> {
    vec![
        (
            "Rated 4 or better",
            SmartView {
                name: "Rated 4+".into(),
                rule: Rule {
                    rating_at_least: Some(4),
                    ..Rule::default()
                },
            },
        ),
        (
            "Unrated",
            SmartView {
                name: "Unrated".into(),
                rule: Rule {
                    rating_at_most: Some(0),
                    ..Rule::default()
                },
            },
        ),
        (
            "Played 3 times or more",
            SmartView {
                name: "Played 3+".into(),
                rule: Rule {
                    played_at_least: Some(3),
                    ..Rule::default()
                },
            },
        ),
        (
            "25 most played",
            SmartView {
                name: "Top 25 played".into(),
                rule: Rule {
                    top_played: Some(25),
                    ..Rule::default()
                },
            },
        ),
        (
            "Added in the last 90 days",
            SmartView {
                name: "Added in 90 days".into(),
                rule: Rule {
                    added_within_days: Some(90),
                    ..Rule::default()
                },
            },
        ),
    ]
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
    views: Vec<SmartView>,
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

    /// The saved Smart Views, in sidebar order.
    pub fn views(&self) -> &[SmartView] {
        &self.views
    }

    /// Add a view, replacing one of the same name.
    ///
    /// Replace rather than append-duplicate, because the name is what identifies a
    /// view to the user *and* to the sidebar's ✕ — two rows reading "Unrated"
    /// would make the second undeletable in practice.
    pub fn add_view(&mut self, view: SmartView) {
        match self.views.iter_mut().find(|v| v.name == view.name) {
            Some(existing) => *existing = view,
            None => self.views.push(view),
        }
    }

    /// Forget a view by name, reporting whether there was one. A missing name is
    /// not an error: the ✕ can outlive its row, the same way an armed playlist
    /// index can.
    pub fn remove_view(&mut self, name: &str) -> bool {
        let before = self.views.len();
        self.views.retain(|v| v.name != name);
        self.views.len() != before
    }

    /// The tracks `rule` selects, in playlist order.
    ///
    /// `now` is a parameter so this stays pure: the same rule at the same instant
    /// gives the same list, which is both what makes it testable and what stops a
    /// view's contents moving with the frame clock.
    ///
    /// An **empty rule matches nothing**, deliberately. The alternative — treating
    /// it as "no constraints, so everything" — means a view whose one constraint
    /// was dropped from its definition silently becomes the whole library, and the
    /// next click replaces the playlist with it.
    ///
    /// Order is the library's own (`sort_key` on the title column, the same default
    /// the file list uses), except for `top_played`, which is a ranking. Ties break
    /// on the track id, because a `HashMap`'s iteration order is randomized per
    /// process and a view whose order changed every launch would reshuffle the
    /// playlist each time it was clicked.
    pub fn select(&self, rule: &Rule, now: u64) -> Vec<PathBuf> {
        if rule.is_empty() {
            return Vec::new();
        }
        let hit = |p: &Path, i: &TrackInfo| self.matches(p, i, rule, now);
        match rule.top_played {
            Some(limit) => {
                let mut ranked: Vec<(u32, &Path)> = self
                    .tags
                    .iter()
                    .filter(|(p, i)| hit(p, i))
                    .map(|(p, _)| (self.stats.get(p).map_or(0, |s| s.plays), p.as_path()))
                    .collect();
                ranked.sort_by(|(pa, a), (pb, b)| pb.cmp(pa).then_with(|| a.cmp(b)));
                ranked
                    .into_iter()
                    .take(limit)
                    .map(|(_, p)| p.to_path_buf())
                    .collect()
            }
            None => {
                // Decorate, sort, undecorate: the key is computed once per track
                // rather than once per comparison.
                let mut keyed: Vec<(String, &Path)> = self
                    .tags
                    .iter()
                    .filter(|(p, i)| hit(p, i))
                    .map(|(p, i)| {
                        (
                            library::sort_key(
                                &library::Entry {
                                    path: p.to_path_buf(),
                                    is_dir: false,
                                },
                                Some(i),
                                0,
                            ),
                            p.as_path(),
                        )
                    })
                    .collect();
                keyed.sort();
                keyed.into_iter().map(|(_, p)| p.to_path_buf()).collect()
            }
        }
    }

    fn matches(&self, track: &Path, info: &TrackInfo, rule: &Rule, now: u64) -> bool {
        if let Some(min) = rule.rating_at_least {
            if info.rating < min {
                return false;
            }
        }
        if let Some(max) = rule.rating_at_most {
            if info.rating > max {
                return false;
            }
        }
        let plays = self.stats.get(track).map_or(0, |s| s.plays);
        if rule.top_played.is_some() && plays == 0 {
            // A ranking of play counts must not be padded out to `limit` with
            // tracks nobody has played.
            return false;
        }
        if let Some(min) = rule.played_at_least {
            if plays < min {
                return false;
            }
        }
        if let Some(days) = rule.added_within_days {
            // No stats entry means never persisted, so there is no first-seen: the
            // track is *new*, and a date rule must not claim it either way.
            let Some(s) = self.stats.get(track) else {
                return false;
            };
            if now.saturating_sub(s.first_seen) / DAY_SECS > u64::from(days) {
                return false;
            }
        }
        true
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
            views: self.views.clone(),
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
            views: f.views,
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
    #[serde(default)]
    views: Vec<SmartView>,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(FILE))
}
