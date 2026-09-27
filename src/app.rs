//! App state and logic — no UI code here.

use crate::audio;
use crate::audio::transition;
use crate::config;
use crate::library;
use crate::library_db;
use crate::network;
use crate::playlist;
use crate::tracks;
use rodio::{mixer::Mixer, Decoder, Sink, Source};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the playhead really is, given what the sink reports.
///
/// The slow-path seek rebuilds the sink with a `skip_duration(target)` source,
/// and `get_pos()` counts only post-skip samples from there — so the fresh sink
/// under-reports by exactly the skip, and `position_offset` holds it. Every read
/// goes through this so the three call sites can't drift.
///
/// `saturating_add` because the two are independent: rodio's position is real
/// playback time and the offset is a *previous* skip, so a bad pair could
/// overflow where a plain `+` would panic in the audio path.
pub fn effective_pos(sink_pos: Duration, offset: Duration) -> Duration {
    sink_pos.saturating_add(offset)
}

// Docking panes (egui_dock) - used by GUI layer only
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub enum Pane {
    NowPlaying,
    Playlist,
    Equalizer,
    Library,
    Visualizer,
    AlbumCover,
}

impl Pane {
    pub const ALL: [Pane; 6] = [
        Pane::NowPlaying,
        Pane::Playlist,
        Pane::Equalizer,
        Pane::Library,
        Pane::Visualizer,
        Pane::AlbumCover,
    ];
}

/// Config types, re-exported because the GUI and the tests reach them through
/// `app`. EQ presets are deliberately NOT re-exported: they belong in
/// `audio::eq` beside the frequencies they curve.
pub use crate::config::{Config, EqData, LibraryData, VizView};

pub struct TPlayApp {
    /// Where every sink is attached. **Not a device**: the app only ever needs
    /// something to hand `Sink::connect_new`, and `rodio::mixer::mixer` builds
    /// one with no audio hardware involved — which is what lets a test build
    /// this same struct (see `main.rs`'s `TPlay`, which owns the device and
    /// passes its mixer in). A `Mixer` is a cheap `Arc` clone, so keeping it
    /// costs nothing.
    mixer: Mixer,
    sink: Sink,

    current_path: Option<PathBuf>,
    total_duration: Option<Duration>,

    volume: f32,

    /// Output stream buffer size in frames, requested at stream open.
    buffer_size: u32,
    /// Spool cache ceiling in MB (`Config::spool_cache_mb`). A field so
    /// `save_config` writes the user's value back instead of resetting it.
    spool_cache_mb: u32,

    /// Pins the slider at the intended position until get_pos() catches up, so a
    /// skip_duration seek can't snap back to 0.
    seek_target: Option<f32>,

    /// The slow-path seek rebuilds the sink with a skip_duration(target) source,
    /// and get_pos() counts only post-skip samples — so the fresh sink
    /// under-reports by exactly the skip amount.
    position_offset: Duration,

    playlist: Vec<PathBuf>,
    /// Index of the loaded track in `playlist`. `None` = direct open, which
    /// breaks the sequential flow.
    current_index: Option<usize>,
    /// The `.tplay` file this playlist came from: `Some` → Save overwrites it,
    /// `None` → Save asks for a name. Cleared by New Playlist.
    playlist_file: Option<PathBuf>,
    /// In-memory playlist differs from its file, so New/Load would lose edits.
    /// Set on content changes, cleared on save/load/new — shuffle/repeat are
    /// appwide and don't count.
    playlist_dirty: bool,

    shuffle: bool,
    /// Repeat mode: loop the playlist (shuffled if shuffle is also on)
    repeat: bool,

    /// Indices already played in this shuffle cycle (history for prev/next).
    played: Vec<usize>,
    /// RNG state for shuffle (XorShift64).
    rng_state: u64,

    /// Owns the handle `EqSource` reads, plus the presets and the ±12 dB clamp.
    eq: audio::eq::EqSettings,
    /// Visualization ring buffer — written by the tap source, read by the GUI.
    viz: audio::viz::VizBuf,

    /// The six user-tunable playback settings. `config::Prefs` owns them because
    /// they map 1:1 onto `Config` fields; the app only ever sees the group.
    prefs: config::Prefs,
    /// Incoming track's sink during a crossfade/gapless. While `Some`, `sink`
    /// is the **outgoing** track and `xf_sink` the incoming one — note that
    /// `current_path` was already flipped to the incoming track at arm time, so
    /// "the current track" names the one playing from `xf_sink`, not from
    /// `sink`. Faded or held per frame in `advance()`.
    xf_sink: Option<Sink>,
    /// The outgoing track's duration, captured at arm time — the arm flips
    /// `total_duration` to the incoming track for the seek bar, so the fade
    /// math needs the old total.
    xf_out_total: Option<Duration>,

    /// The config.json content currently on disk, and when it was written.
    /// `flush_config` compares a fresh snapshot against it, so *what* changed is
    /// never tracked by hand — a setter that forgets to flag itself cannot lose
    /// a setting.
    saved_config: config::Persisted,
    /// The same pair for library.json, which is why the policy is one type rather
    /// than two fields per file.
    saved_db: config::Persisted,

    // ── Library pane ──────────────────────────────────────────────────────
    /// Browsed dir, rows, sort, bookmarks, hidden-folder toggle. What it can't
    /// own stays here: the tag cache, shared with the other two panes.
    library: library::LibraryState,
    /// The tag cache every pane reads, plus the play history only this app knows.
    /// Two maps with no shared insert path, so re-reading a file's tags cannot
    /// cost a play count — see `library_db.rs`.
    db: library_db::TrackDb,
    /// Reads tags into `db`, local or remote. Owns the local scan's receiver, so
    /// the app holds no mpsc plumbing.
    tracks: tracks::TagReader,

    // ── SMB network ────────────────────────────────────────────────────────
    /// All network state + the worker channels.
    network: network::Network,
}

impl TPlayApp {
    /// Everything the app needs, from config alone. `cc` is deliberately not a
    /// parameter: the app holds no `egui::Context` and owns no theme, so it
    /// cannot construct one — `main.rs`'s `TPlay` does both and hands over the
    /// settings `config.json` already carries.
    ///
    /// `db` is passed in for the same reason the config is: the library
    /// database is *read* outside the app, so a test can hand over an empty one
    /// instead of reaching into the real config dir.
    pub fn new(config: &Config, db: library_db::TrackDb, mixer: Mixer) -> Self {
        // Seed the "what's on disk" baselines with what we just read, so the
        // first flush only writes if something has already changed.
        let saved_config =
            config::Persisted::new(&serde_json::to_string_pretty(config).unwrap_or_default());
        let saved_db = config::Persisted::new(&db.snapshot());

        let sink = Sink::connect_new(&mixer);

        // Channels + worker are owned by `network::Network`.
        let mut app = Self {
            mixer,
            sink,
            current_path: None,
            total_duration: None,
            volume: config.volume,
            buffer_size: config.buffer_size,
            spool_cache_mb: config.spool_cache_mb,
            seek_target: None,
            position_offset: Duration::ZERO,
            playlist: Vec::new(),
            current_index: None,
            playlist_file: None,
            playlist_dirty: false,
            shuffle: config.shuffle,
            repeat: config.repeat,
            played: Vec::new(),
            rng_state: 0xC0FFEE, // arbitrary seed
            eq: audio::eq::EqSettings::new(config.eq.enabled, config.eq.gains),
            viz: audio::viz::VizBuf::new(),
            prefs: config::Prefs::from_config(config),
            xf_sink: None,
            xf_out_total: None,
            saved_config,
            saved_db,
            library: library::LibraryState::new(
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
                config
                    .library
                    .favorites
                    .clone()
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|d| d.is_dir())
                    .collect(),
                config.library.show_hidden,
            ),
            db,
            tracks: tracks::TagReader::new(),
            network: network::Network::new(config.servers.clone()),
        };

        app.sink.set_volume(app.volume);

        let p = PathBuf::from(&config.library.last_dir);
        if p.is_dir() {
            app.navigate_to(p);
        }

        // `exists()` is always false for an smb:// path, so a remote target
        // would skip this restore silently — route it through the same async
        // fetch a click uses, and the reply lands on the first frame's drain().
        // It fails until the user logs in again: passwords are session-memory
        // only, so there is nothing to reuse.
        if let Some(pl_path) = config.last_playlist.clone() {
            let path = PathBuf::from(pl_path);
            if network::is_remote(&path) {
                app.network.fetch(path.to_string_lossy().into_owned());
            } else if path.exists() {
                app.load_playlist_from(path);
            }
        }
        app.navigate_to(app.library.dir().to_path_buf());

        app
    }

    /// Write config.json if its content differs from the last write, at most
    /// once per `CONFIG_SAVE_DEBOUNCE_SECS`, and unconditionally when closing.
    ///
    /// The clock and the close flag are the caller's — the app holds no
    /// `egui::Context` (see `main.rs`'s `TPlay`), so `TPlayApp::update` takes
    /// them as arguments.
    ///
    /// The "did it change" test is a string compare against the previous
    /// snapshot, not a dirty flag raised by each setter: the flag was a manual
    /// obligation with ~20 call sites, and a missed one silently lost a
    /// setting. This is the shape `gui/coordinator.rs` already used for
    /// dock_layout.json. Cost is one ~500-byte serialization per frame.
    pub fn flush_config(&mut self, now: f64, closing: bool, theme_id: &str) {
        if !self.saved_config.due(now, closing) {
            return;
        }
        let config = self.snapshot(theme_id);
        let json = serde_json::to_string_pretty(&config).unwrap_or_default();
        if !self.saved_config.wants_write(&json, now, closing) {
            return;
        }
        self.saved_config.note_written(&json, now);
        config::save(&config);
    }

    /// The library database's half of the same policy. Same throttle, same
    /// content compare — and the same reason `due` comes first, which matters
    /// more here: the content is the whole tag cache, so the serialization this
    /// skips is the entire library.
    fn flush_db(&mut self, now: f64, closing: bool) {
        if !self.saved_db.due(now, closing) {
            return;
        }
        let json = self.db.snapshot();
        if !self.saved_db.wants_write(&json, now, closing) {
            return;
        }
        // `persist` stamps `first_seen` for anything never written, so the string
        // it returns is the one that reached the file — and the one to compare
        // against next frame, or every frame after this would look changed.
        self.saved_db
            .note_written(&self.db.persist(now_epoch()), now);
    }

    /// The database: the tag cache every pane reads, and the play history.
    /// Writing it is the app's own business — a pane reads, `apply_edit` and
    /// `start_track` write — so there is deliberately no `db_mut`.
    pub fn db(&self) -> &library_db::TrackDb {
        &self.db
    }

    /// Every setting, as the file wants it. `theme_id` is passed in because the
    /// theme belongs to the GUI layer — the app persists which theme is
    /// selected, it does not own one.
    ///
    /// Every field listed explicitly, no `..Default::default()`: a new
    /// `Config` field must be a compile error here, not a silent reset.
    fn snapshot(&self, theme_id: &str) -> Config {
        Config {
            theme: theme_id.to_owned(),
            eq: EqData {
                enabled: self.eq.enabled(),
                gains: self.eq.gains(),
            },
            shuffle: self.shuffle,
            repeat: self.repeat,
            viz_view: self.prefs.viz_view(),
            remaining: self.prefs.remaining(),
            gapless: self.prefs.gapless(),
            crossfade: self.prefs.crossfade(),
            crossfade_secs: self.prefs.crossfade_secs(),
            balance: self.prefs.balance(),
            volume: self.volume,
            buffer_size: self.buffer_size,
            spool_cache_mb: self.spool_cache_mb,
            last_playlist: self
                .playlist_file
                .as_ref()
                .and_then(|p| p.to_str())
                .map(str::to_owned),
            library: LibraryData {
                favorites: self
                    .library
                    .favorites()
                    .iter()
                    .filter_map(|d| d.to_str().map(str::to_owned))
                    .collect(),
                last_dir: self.library.dir().to_string_lossy().into_owned(),
                show_hidden: self.library.show_hidden(),
            },
            servers: self.network.servers().to_vec(),
        }
    }

    /// Append tracks, skipping any id already present.
    ///
    /// ponytail: ids arrive **un-normalized** here, where `apply_playlist` runs
    /// them through `tracks::normalize`. The Library hands over `read_dir` paths
    /// under the folder as navigated, so on a symlinked music dir one file can
    /// have two ids — which `dedup`'s literal `==` misses, `tag_cache` then
    /// scans twice, and `playlist_reordered`'s `position()` (which assumes ids
    /// are unique) can resolve to the wrong row. Normalize at the three call
    /// sites if it bites; it is not normalized here because that would also
    /// silently drop a file deleted between listing and add.
    pub fn add_files(&mut self, paths: Vec<PathBuf>) {
        let mut added_count = 0;
        for path in paths {
            if !self.playlist.iter().any(|p| p == &path) {
                self.playlist.push(path);
                added_count += 1;
            }
        }
        if added_count > 0 {
            self.playlist_dirty = true;
            // Tag the new tracks in the background (dedup handles repeats).
            self.ensure_tags(self.playlist.clone());
        }
    }

    /// Replace the sink with a fresh empty one, dropping the old decoder/file.
    /// Shared by `start_track` and `play_now`'s spool request, which tears the
    /// current track down until the download lands.
    fn fresh_sink(&mut self) {
        self.cancel_xf();
        self.seek_target = None;
        self.position_offset = Duration::ZERO;
        self.viz.clear();
        self.sink = Sink::connect_new(&self.mixer);
        self.sink.set_volume(self.volume);
    }

    /// Load a track's bytes into the sink. `track` is a track **id** — local
    /// path or `smb://` URI — and the only track identity the app carries;
    /// where the bytes actually are is `tracks`' problem.
    ///
    /// Every file read goes through `tracks::{open, info, probe}`, which
    /// resolve internally. One parameter cannot be swapped; the two-path form
    /// this replaces is documented under **Tracks** in AGENTS.md.
    fn start_track(&mut self, track: PathBuf) {
        self.fresh_sink(); // cancels any live crossfade

        // Tag up front so Now Playing shows title · artist immediately instead
        // of waiting on a scan (one file, negligible cost). Keyed by the id,
        // which is what every pane looks up by.
        if let Some(info) = tracks::info(&track) {
            self.db.cache_mut().insert(track.clone(), info);
        }
        // One play, once per track that actually starts — `start_track` is the
        // one place that happens, and `seek` does not go through it, so a seek
        // is not a second play.
        self.db.note_played(&track, now_epoch());

        let file = match tracks::open(&track) {
            Some(f) => f,
            None => {
                eprintln!("tplay: no file for {}", track.display());
                self.current_path = None;
                self.total_duration = None;
                return;
            }
        };
        let decoder = match Decoder::try_from(file) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("tplay: decode error: {e}");
                self.current_path = None;
                self.total_duration = None;
                return;
            }
        };
        let total_duration = decoder.total_duration().or_else(|| tracks::probe(&track));
        self.total_duration = total_duration;

        // Always full length, no truncation or pre-mix — the xf_sink in
        // `advance()` is what overlaps tracks.
        let eq_source = audio::eq::EqSource::new(decoder, self.eq.shared());
        let tap_source = audio::viz::TapSource::new(eq_source, self.viz.clone());
        let balance_source =
            audio::balance::BalanceSource::new(tap_source, self.prefs.balance_shared());
        self.sink.append(balance_source);
        self.current_path = Some(track);
    }

    /// Cancel any live crossfade/gapless: stop the xf sink and restore main volume.
    fn cancel_xf(&mut self) {
        if let Some(xf) = self.xf_sink.take() {
            xf.stop();
        }
        self.xf_out_total = None;
        self.sink.set_volume(self.volume);
    }

    /// The index that plays next, **without recording it** — plus the RNG state
    /// the draw produced, for `commit_next_index` to install.
    ///
    /// Pure by construction: the draw comes from a scratch copy of `rng_state`,
    /// so per-frame asking returns the same candidate and burns no entropy.
    /// See **Shuffle order** in AGENTS.md.
    fn peek_next_index(&self) -> Option<(usize, u64)> {
        if self.playlist.is_empty() {
            return None;
        }
        let len = self.playlist.len();
        if len == 1 {
            return self.repeat.then_some(0).map(|i| (i, self.rng_state));
        }

        if !self.shuffle {
            let idx = match (self.repeat, self.current_index) {
                (false, Some(i)) if i + 1 < len => Some(i + 1),
                (true, Some(i)) => Some((i + 1) % len),
                (true, None) => Some(0),
                _ => None,
            };
            return idx.map(|i| (i, self.rng_state));
        }

        // Shuffle: random from the unplayed pool.
        let unplayed: Vec<usize> = (0..len).filter(|i| !self.played.contains(i)).collect();
        let mut rng = self.rng_state;
        let idx = if unplayed.is_empty() {
            // Repeat restarts the cycle, so the draw covers the whole playlist;
            // `commit_next_index` is what clears `played` for it.
            self.repeat.then(|| playlist::rand_usize(&mut rng, len))
        } else {
            Some(unplayed[playlist::rand_usize(&mut rng, unplayed.len())])
        };
        idx.map(|i| (i, rng))
    }

    /// Record `idx` as played: the one place the shuffle cycle advances. Called
    /// once per track that actually starts playing, so `played` changes only on
    /// a play, a playlist edit (`reset_shuffle` on remove/move/reorder/load) or
    /// a stop — never on a frame of deliberation.
    fn commit_next_index(&mut self, idx: usize, rng: u64) {
        self.rng_state = rng;
        if !self.shuffle || self.playlist.len() < 2 {
            return;
        }
        // Exhausted pool + repeat = a new cycle: drop the history so the track
        // about to play isn't immediately in `played`. Spelled as the same
        // "every index played" test the peek used, not a length compare.
        if (0..self.playlist.len()).all(|i| self.played.contains(&i)) {
            self.played.clear();
        }
        self.played.push(idx);
    }

    /// Peek, then commit — the pick-and-play entry point for every caller that
    /// is genuinely about to start a track.
    fn next_track_index(&mut self) -> Option<usize> {
        let (idx, rng) = self.peek_next_index()?;
        self.commit_next_index(idx, rng);
        Some(idx)
    }

    fn reset_shuffle(&mut self) {
        self.played.clear();
    }

    /// The index Prev would go to, **without recording it**. Pure for the same
    /// reason as `peek_next_index`: the transport buttons ask every frame to
    /// decide whether to light up, so it must not touch the history.
    fn peek_prev_index(&self) -> Option<usize> {
        if self.playlist.is_empty() {
            return None;
        }
        let len = self.playlist.len();
        if len == 1 {
            return self.repeat.then_some(0);
        }

        if !self.shuffle {
            return match (self.repeat, self.current_index) {
                (false, Some(i)) if i > 0 => Some(i - 1),
                (true, Some(i)) => Some((i + len - 1) % len),
                (true, None) => Some(len - 1),
                _ => None,
            };
        }

        // Shuffle walks back through the history. Past one entry the target is
        // the second-to-last: stepping back *pops* the track being left, and
        // that pop is `commit_prev`'s job, not ours.
        if self.played.len() > 1 {
            self.played.get(self.played.len() - 2).copied()
        } else if self.repeat {
            // One entry, so step back onto it. **An empty history has nothing to
            // walk**, however repeat is set — and an empty history is reachable
            // by clicking any playlist row with shuffle on, since `play_track`
            // resets it and `start` never pushes.
            self.played.last().copied()
        } else {
            None
        }
    }

    /// Record the step back: the track we are leaving leaves the walk-back too.
    fn commit_prev(&mut self) {
        if self.shuffle && self.playlist.len() > 1 && self.played.len() > 1 {
            self.played.pop();
        }
    }

    fn prev_track_index(&mut self) -> Option<usize> {
        let idx = self.peek_prev_index()?;
        self.commit_prev();
        Some(idx)
    }

    pub fn seek(&mut self, progress: f32) {
        self.seek_target = Some(progress);
        self.cancel_xf(); // a seek invalidates any live crossfade/gapless

        let path = match self.current_path.clone() {
            Some(p) => p,
            None => return,
        };
        let total_secs = match self.total_duration {
            Some(d) => d.as_secs_f32(),
            None => return,
        };
        let target = Duration::from_secs_f32((progress * total_secs).max(0.0));

        if self.sink.try_seek(target).is_ok() {
            // Landed in place: TrackPosition reports the new position, so the
            // slow path's skip offset no longer applies.
            self.position_offset = Duration::ZERO;
            return;
        }

        let was_paused = self.sink.is_paused();

        // The slow path reopens the track. It used to pass `path` straight to
        // `File::open`, which for a remote track is an `smb://` URI — so seeking
        // a non-seekable remote track silently did nothing. `tracks::open`
        // resolves, and the app never sees a local path.
        let file = match tracks::open(&path) {
            Some(f) => f,
            None => {
                eprintln!("seek: no file for {}", path.display());
                return;
            }
        };
        let source = match Decoder::try_from(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("seek decode: {e}");
                return;
            }
        };

        self.sink = Sink::connect_new(&self.mixer);
        self.sink.set_volume(self.volume);
        // get_pos() on the fresh sink counts only post-skip samples; the
        // skipped `target` is the new position offset from here on.
        self.position_offset = target;
        let seeked_source = transition::seek_or_skip(source, target);
        let eq_source = audio::eq::EqSource::new(seeked_source, self.eq.shared());
        let tap_source = audio::viz::TapSource::new(eq_source, self.viz.clone());
        let balance_source =
            audio::balance::BalanceSource::new(tap_source, self.prefs.balance_shared());
        self.sink.append(balance_source);
        if was_paused {
            self.sink.pause();
        }
    }

    pub fn advance(&mut self) {
        // 1) Settle a live xf: fade toward the swap, promote once the outgoing
        // track has drained. `xf_out_total` is the OUTGOING duration —
        // `self.total_duration` was flipped to the incoming track at arm time
        // (the seek bar reads the incoming track during the fade).
        if let Some(xf) = &self.xf_sink {
            let pos = effective_pos(self.sink.get_pos(), self.position_offset);
            let out_total = self.xf_out_total.unwrap_or_default();
            if self.sink.empty() {
                self.finish_xf();
                return;
            }
            let remaining = out_total.saturating_sub(pos);
            // The fade decision is `xf_gains`'s (pure, tested); this is only
            // the effect. Gapless gets (1.0, 0.0) from it too, so the incoming
            // track sits silent until the swap and there is no branch here.
            let (out_gain, in_gain) = transition::xf_gains(
                remaining,
                self.prefs.crossfade(),
                self.prefs.crossfade_secs(),
            );
            self.sink.set_volume(self.volume * out_gain);
            xf.set_volume(self.volume * in_gain);
            return;
        }

        // 2) No live xf — arm one when the current track nears its end. The
        // sink-shape gate lives here because it is `Sink` state with no data
        // equivalent: from the playlist, unpaused, exactly one source queued
        // (the current track, nothing pre-buffered). The mode check is
        // duplicated by `arm_plan` on purpose — with both modes off this block
        // would otherwise run, and peek, every frame of every track for nothing.
        if self.current_index.is_some()
            && self.current_path.is_some()
            && !self.sink.is_paused()
            && self.sink.len() == 1
            && (self.prefs.crossfade() || self.prefs.gapless())
        {
            // PEEK, don't pick: this block runs on every frame until the arm
            // resolves, and a picking call here appended to `played` 60 times a
            // second — the shuffle order was being rewritten by deliberation.
            // Nothing is recorded until the arm actually lands, below.
            let peeked = self.peek_next_index();
            let next = peeked.map(|(i, _)| (i, self.playlist[i].clone()));
            let (ready, tagged) = next
                .as_ref()
                .map(|(_, p)| {
                    (
                        tracks::is_ready(p),
                        self.db.cache().get(p).and_then(|i| i.duration),
                    )
                })
                .unzip();

            let input = transition::ArmInput {
                crossfade: self.prefs.crossfade(),
                gapless: self.prefs.gapless(),
                crossfade_secs: self.prefs.crossfade_secs(),
                total: self.total_duration,
                pos: effective_pos(self.sink.get_pos(), self.position_offset),
                next: next.as_ref().map(|(i, p)| (*i, p.as_path())),
                next_ready: ready.unwrap_or(false),
                next_duration: tagged.flatten(),
            };
            // Every guard lives in `arm_plan`; this is only the side effects.
            if let Some(armed) = transition::arm_plan(&input) {
                // Build the incoming track source (full track, buffered) BEFORE
                // any state flips, so a build failure leaves the arm cleanly
                // skipped rather than half-applied. A `None` here is a gap, not
                // a crash: `advance` returns and natural advance picks the track
                // up a moment later.
                let Some(xf_source) = transition::build_gapless_next(
                    &armed.track,
                    self.eq.shared(),
                    self.prefs.balance_shared(),
                    self.viz.clone(),
                ) else {
                    return;
                };
                // The incoming track is now really playing, so this is the one
                // moment the shuffle cycle may advance — one push, with the RNG
                // draw the peek already made. A skipped or failed arm commits
                // nothing, and natural advance picks fresh at the track end.
                if let Some((_, rng)) = peeked {
                    self.commit_next_index(armed.index, rng);
                }
                // Capture the outgoing duration for the fade math BEFORE
                // flipping total_duration to the incoming track below.
                self.xf_out_total = Some(armed.out_total);
                // Second sink on the same mixer, so they play simultaneously.
                let xf_sink = Sink::connect_new(&self.mixer);
                xf_sink.append(xf_source);
                xf_sink.set_volume(0.0);
                self.xf_sink = Some(xf_sink);
                // Pre-flip the playlist metadata so Now Playing shows the new track.
                self.current_index = Some(armed.index);
                // Tagged by the id, so every pane still finds the entry.
                if let Some(info) = tracks::info(&armed.track) {
                    self.total_duration = info.duration;
                    self.db.cache_mut().insert(armed.track.clone(), info);
                }
                if self.total_duration.is_none() {
                    self.total_duration = tracks::probe(&armed.track);
                }
                self.current_path = Some(armed.track);
                self.seek_target = None;
                return;
            }
        }

        // 3) Natural advance (no xf armed, modes off, or conditions not met).
        if !self.sink.empty() || self.sink.is_paused() || self.current_path.is_none() {
            return;
        }
        if let Some(next) = self.next_track_index() {
            self.start(next);
        }
    }

    /// The outgoing track ended: promote the incoming sink, restore full volume.
    fn finish_xf(&mut self) {
        if let Some(xf) = self.xf_sink.take() {
            self.sink.stop();
            // The xf sink's position is already absolute, hence no offset here.
            self.sink = xf;
            self.sink.set_volume(self.volume);
            self.xf_out_total = None;
            self.position_offset = Duration::ZERO;
        }
    }

    pub fn fmt_duration(d: Option<Duration>) -> String {
        d.map(|d| {
            let s = d.as_secs();
            format!("{:02}:{:02}", s / 60, s % 60)
        })
        .unwrap_or_else(|| "--:--".into())
    }

    pub fn format_freq(f: f32) -> String {
        if f >= 1000.0 {
            format!("{}K", (f / 1000.0) as i32)
        } else {
            format!("{}", f as i32)
        }
    }

    /// Public actions called by GUI layer.
    /// The transport play button: resume what is already loaded, else start
    /// playback. The track's source is not this method's concern — `play_now`
    /// resolves it.
    pub fn play(&mut self) {
        if self.sink.is_paused() && !self.sink.empty() {
            self.sink.play();
            if let Some(xf) = &self.xf_sink {
                xf.play();
            }
        } else if self.current_path.is_none() && self.network.pending().is_some() {
            // A track is already requested and on its way — it starts on its own
            // when it lands. Without this arm the fresh empty sink and the `None`
            // current_path would fall through to `play_first_track` and start a
            // *different* track out from under the pending one. Empty by design.
        } else if let Some(path) = self.current_path.clone() {
            // Replay. Goes through `play_now`, never `start_track`:
            // `current_path` is an `smb://` URI for a remote track, and opening
            // that as a path always fails.
            self.play_now(path);
        } else if !self.playlist.is_empty() {
            self.play_first_track();
        }
        // Nothing loaded and an empty playlist: nothing to play. Tracks are
        // added from the Library pane, so play on an empty playlist is a no-op.
    }

    fn play_first_track(&mut self) {
        if self.shuffle {
            self.reset_shuffle();
            let len = self.playlist.len();
            let idx = playlist::rand_usize(&mut self.rng_state, len);
            self.played.push(idx);
            self.start(idx);
        } else {
            self.start(0);
        }
    }

    pub fn pause(&mut self) {
        self.sink.pause();
        if let Some(xf) = &self.xf_sink {
            xf.pause();
        }
    }

    /// Stop playback and reset song + playlist state: the current track is
    /// unloaded and the play position rewound, so the next Play starts from the
    /// top of the playlist (or shuffle order) instead of resuming.
    pub fn stop(&mut self) {
        self.cancel_xf();
        self.sink.stop();
        self.current_path = None;
        self.current_index = None;
        self.total_duration = None;
        self.seek_target = None;
        self.position_offset = Duration::ZERO;
        self.viz.clear();
        self.reset_shuffle();
        // A spooled track loads on arrival — stop cancels that intent.
        self.network.discard_pending();
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.sink.set_volume(volume);
        // A live xf's volume is re-applied each frame in `advance()` scaled by
        // `self.volume`, so a change mid-fade lands on the next frame.
    }

    pub fn next_track(&mut self) {
        if let Some(next_idx) = self.next_track_index() {
            self.start(next_idx);
        }
    }

    pub fn prev_track(&mut self) {
        if let Some(prev_idx) = self.prev_track_index() {
            self.start(prev_idx);
        }
    }

    pub fn remove_track(&mut self, index: usize) {
        self.playlist_dirty = true;
        // Asked *before* the removal, and not derivable afterwards: a `None`
        // index afterwards means two different things — the playing track was
        // removed, or there was never an index (a direct open). Keying the
        // unload on `current_index.is_none()` could not tell them apart, so
        // removing an *unrelated* track while a directly-opened file played
        // blanked Now Playing for the file actually playing.
        let was_current = self.current_index == Some(index);
        self.playlist.remove(index);
        self.current_index = self.current_index.and_then(|ci| {
            if ci == index {
                None
            } else if ci > index {
                Some(ci - 1)
            } else {
                Some(ci)
            }
        });
        if was_current {
            // The track that was playing has left the list, so unload it rather
            // than leave the sink running: audio would carry on with nothing to
            // show for it, and `advance`'s `current_path` guard would refuse to
            // move on. `stop` covers the xf sink, the position and the history.
            self.stop();
            return;
        }
        self.cancel_xf();
        self.reset_shuffle();
    }

    pub fn move_track(&mut self, from: usize, to: usize) {
        if from != to {
            self.playlist_dirty = true;
            let item = self.playlist.remove(from);
            self.playlist.insert(to, item);
            self.current_index = self.current_index.map(|ci| {
                if ci == from {
                    to
                } else if from < ci && ci <= to {
                    ci - 1
                } else if to <= ci && ci < from {
                    ci + 1
                } else {
                    ci
                }
            });
            self.cancel_xf();
            self.reset_shuffle();
        }
    }

    pub fn play_track(&mut self, index: usize) {
        self.reset_shuffle();
        self.start(index);
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.reset_shuffle();
    }

    pub fn toggle_repeat(&mut self) {
        self.repeat = !self.repeat;
    }

    // ── Playlist editing — content ops, from the Playlist pane's ops row ──────
    //
    // The ordering half is `playlist.rs` (pure, tested); what is left here is the
    // part that has to know about state, and it is the same for all three.

    /// Sort the playlist by a Library column — a `library::SORT_OPTIONS` index,
    /// so the playlist orders exactly like the file list beside it.
    pub fn sort_playlist(&mut self, col: usize) {
        playlist::sort_tracks(&mut self.playlist, self.db.cache(), col);
        self.playlist_reordered();
    }

    /// Play the playlist back to front.
    pub fn reverse_playlist(&mut self) {
        self.playlist.reverse();
        self.playlist_reordered();
    }

    /// Shuffle the playlist's order, once, with the same RNG the shuffle
    /// playback picks from.
    pub fn randomize_playlist(&mut self) {
        playlist::shuffle_tracks(&mut self.playlist, &mut self.rng_state);
        self.playlist_reordered();
    }

    /// What every content edit to the playlist has to repair.
    ///
    /// The track that was playing has to keep playing, so `current_index` — an
    /// index into a list that just moved — is re-found from the track *id*,
    /// which is stable across a reorder (a local path or an `smb://` URI alike)
    /// and exact because playlist ids are unique. Without this a sort moves the
    /// highlight off the row that is playing, and `advance` carries on from
    /// whatever track slid into the old index.
    ///
    /// A direct open (`play_file`) has no index to follow and must not acquire
    /// one, or the track drops into the playlist's sequential flow.
    fn playlist_reordered(&mut self) {
        self.playlist_dirty = true;
        // A pending crossfade's incoming track is pinned to an index that just
        // moved, so it goes the same way a seek or a remove sends it.
        self.cancel_xf();
        // The `played` history is a list of positions in the old order.
        self.reset_shuffle();
        if self.current_index.is_some() {
            self.current_index = self
                .current_path
                .as_ref()
                .and_then(|cur| self.playlist.iter().position(|p| p == cur));
        }
    }

    // ── Playlists — `.tplay`/`.m3u`/`.m3u8`/`.pls` files on disk, found in the
    //    Library like any other file. Shuffle/repeat are appwide settings
    //    (config.json), never playlist content.

    /// Write the current playlist (paths only) and record it as the file future
    /// saves overwrite without re-opening the dialog.
    ///
    /// A remote (`smb://`) target goes to the SMB worker instead: the write is
    /// async, so `playlist_file`/`playlist_dirty` are only updated once the
    /// `Event::Saved` reply confirms it. Local writes are synchronous — a
    /// failure leaves the playlist dirty and untracked so the user can retry,
    /// rather than silently claiming a save that never happened.
    pub fn save_playlist_to(&mut self, path: PathBuf) {
        if network::is_remote(&path) {
            // Same bytes the local write would produce: the format comes from the
            // target's extension, which on a URI is the remote filename's.
            match library::playlist_text(&path, &self.playlist) {
                Ok(text) => {
                    self.network.save(path.to_string_lossy().into_owned(), text);
                }
                Err(e) => eprintln!("tplay: could not serialize playlist: {e}"),
            }
            return;
        }
        if let Err(e) = library::write_playlist(&path, &self.playlist) {
            eprintln!("tplay: could not write playlist {}: {}", path.display(), e);
            return;
        }
        self.playlist_file = Some(path);
        self.playlist_dirty = false;
    }

    /// Re-list the current remote directory. Called after a playlist lands on a
    /// share so the new file shows up without navigating away and back.
    fn refresh_network_dir(&mut self) {
        if let Some(b) = self.network.browse().cloned() {
            if b.busy {
                return;
            }
            match &b.share {
                Some(share) => self.network.browse_open(
                    network::dir_uri(&b.host, share, &b.rel),
                    Some(share.clone()),
                    b.rel,
                ),
                None => self.network.browse_server(b.host),
            }
        }
    }

    /// Replace the current playlist from a `.tplay` file. Tracks that no
    /// longer exist are dropped; unparseable files leave the playlist alone.
    pub fn load_playlist_from(&mut self, path: PathBuf) {
        let base = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let Some(paths) = library::read_playlist(&path, base) else {
            return;
        };
        self.apply_playlist(paths, Some(path));
    }

    /// Apply a downloaded remote playlist. `base` is the share directory the
    /// playlist was browsed at — relative entries must resolve against THAT,
    /// not against the spool cache copy we just read from.
    fn load_fetched_playlist(&mut self, uri: String, local: PathBuf) {
        let base = network::uri_parent(&uri);
        let Some(paths) = library::read_playlist(&local, std::path::Path::new(base)) else {
            eprintln!("tplay: could not parse playlist {uri}");
            return;
        };
        self.apply_playlist(paths, Some(PathBuf::from(uri)));
    }

    /// Replace the playlist with what a Smart View selects.
    ///
    /// The same machinery a `.tplay` load uses, reached through the same tail, and
    /// with **no tracked file** — a view is a query, not something to Save over, so
    /// Save asks where to go exactly as it does for a new playlist.
    ///
    /// A view that selects nothing replaces the playlist with nothing. That is the
    /// consistent reading of "the playlist is this view", and the alternative —
    /// quietly leaving the old playlist in place — makes an empty view look like a
    /// click that did not work.
    pub fn load_smart_view(&mut self, view: &library_db::SmartView) {
        let tracks = self.db.select(&view.rule, now_epoch());
        self.apply_playlist(tracks, None);
    }

    /// The saved Smart Views, in sidebar order.
    pub fn smart_views(&self) -> &[library_db::SmartView] {
        self.db.views()
    }

    /// Save a view, replacing one of the same name.
    pub fn add_smart_view(&mut self, view: library_db::SmartView) {
        self.db.add_view(view);
    }

    /// Forget a view by name, reporting whether there was one to forget.
    pub fn remove_smart_view(&mut self, name: &str) -> bool {
        self.db.remove_view(name)
    }

    /// Shared tail of every playlist-load path: filter, stop, scan, track.
    fn apply_playlist(&mut self, paths: Vec<PathBuf>, file: Option<PathBuf>) {
        // Remote tracks can't canonicalize and are kept verbatim; local paths
        // that no longer exist are dropped.
        self.playlist = paths.into_iter().filter_map(tracks::normalize).collect();
        // Duplicate ids collapse here, and this is the only place one can
        // arrive: `add_files` dedups as tracks go in, so a repeated path can
        // only have come out of the file being loaded.
        playlist::dedup(&mut self.playlist);
        self.stop();
        self.ensure_tags(self.playlist.clone());
        self.playlist_file = file;
        self.playlist_dirty = false;
    }

    /// Clear the playlist for a fresh build (confirm dialog lives in the GUI).
    /// Unsets the tracked file — a new playlist must ask where it's saved.
    pub fn new_playlist(&mut self) {
        self.stop();
        self.playlist.clear();
        self.playlist_file = None;
        self.playlist_dirty = false;
    }

    /// The `.tplay` file this playlist is saved to / was loaded from, if any.
    pub fn playlist_file(&self) -> Option<&std::path::Path> {
        self.playlist_file.as_deref()
    }

    /// Whether the in-memory playlist has edits not yet written to its file
    /// (or no file yet at all). The GUI confirms New/Load only when true.
    pub fn playlist_dirty(&self) -> bool {
        self.playlist_dirty
    }

    /// Display name of the current playlist: the tracked file's stem when
    /// saved/loaded, else "Untitled". Single source — no pane re-derives it.
    pub fn playlist_name(&self) -> String {
        self.playlist_file
            .as_ref()
            .and_then(|f| f.file_stem())
            .and_then(|s| s.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| "Untitled".to_string())
    }

    // ── Library ────────────────────────────────────────────────────────────

    /// Write a tag edit to one track and take the file's own answer back into the
    /// cache.
    ///
    /// The seam every tag mutation goes through, so there is exactly one place
    /// that can fail a write, one that refreshes the cache, and one that re-sorts.
    /// `Err` means **nothing changed** — the cache and the file list are left
    /// exactly as they were, because a half-applied edit is worse than a refused
    /// one.
    pub fn apply_edit(&mut self, track: &Path, edit: tracks::Edit) -> Result<(), String> {
        let info = tracks::write_tags(track, &edit)?;
        self.db.cache_mut().insert(track.to_path_buf(), info);
        // Re-sort, because the entries the pane is drawing *are* the sorted list,
        // and a tag that is a sort key has just changed. `apply_sort` rather than
        // `set_sort`: the column has not changed, and re-setting it would flip the
        // direction.
        //
        // Not observable yet — a rating is not one of `SORT_OPTIONS`, so nothing
        // written through today's `Edit` can move a row. It stays because the
        // alternative is a manual obligation on whoever adds the first sortable
        // field, and this repo has already deleted one of those for losing data
        // silently. Phase 4 covers it.
        self.library.apply_sort(self.db.cache());
        Ok(())
    }

    /// "Played 7 times · last 3 d ago" for a track, or `None` when it has no play
    /// history — an unplayed track has nothing to say.
    ///
    /// The one line that joins two owners: the clock is the app's (it is what
    /// stamps the history, and it has to be wall-clock so a persisted stamp still
    /// means something next session) and the counts are the database's. The
    /// wording is `PlayStats::describe`, which is pure and tested with literals.
    pub fn play_history(&self, track: &Path) -> Option<String> {
        self.db
            .stats_of(track)
            .and_then(|s| s.describe(now_epoch()))
    }

    /// List a directory and start tagging its audio files in the background.
    /// Persists the last browsed dir on the way.
    ///
    /// The split is the point: `LibraryState::open` owns *what the folder
    /// contains and how it sorts*, and returns the paths that need scanning.
    /// Starting that scan stays here, because the tag cache and the `TagReader`
    /// are the app's, shared with the other two panes.
    pub fn navigate_to(&mut self, dir: PathBuf) {
        let Some(scan) = self.library.open(dir, self.db.cache()) else {
            return;
        };
        self.ensure_tags(scan);
    }

    /// Header click: pick a new column (ascending) or flip the active one and
    /// re-sort the current folder in place.
    pub fn set_library_sort(&mut self, key: usize) {
        self.library.set_sort(key, self.db.cache());
    }

    /// Ensure the given audio files have tag info in the cache. Returns whether
    /// anything was started, so the caller can ask for a repaint — the app has
    /// no `Context` to ask with (see `main.rs`'s `TPlay`).
    ///
    /// Which transport a track needs is not this function's business — the
    /// `TagReader` splits the batch and hands each half to the thread or the SMB
    /// worker. Both land in the same `tag_cache`, remote keyed by URI, so every
    /// pane fills in identically.
    pub fn ensure_tags(&mut self, paths: Vec<PathBuf>) -> bool {
        let cache = self.db.cache();
        self.tracks.request(cache, &mut self.network, &paths)
    }

    /// Drain finished local tag results into the cache. Returns whether anything
    /// arrived or the scan ended, so the caller can ask for a repaint.
    pub fn drain_tag_scan(&mut self) -> bool {
        let cache = self.db.cache_mut();
        self.tracks.drain_into(cache)
    }

    /// Play a library file directly. `current_index = None` is the "direct
    /// open" semantics: the playlist's sequential flow isn't touched and
    /// auto-advance won't cascade off it.
    pub fn play_file(&mut self, track: PathBuf) {
        self.current_index = None;
        self.play_now(track);
    }

    /// Common playback start: set current_index and play the track.
    fn start(&mut self, idx: usize) {
        self.current_index = Some(idx);
        let track = self.playlist[idx].clone();
        self.play_now(track);
    }

    /// Make `track` the playing track, from whatever source it is. This is the
    /// **only** way a track starts playing: its bytes are used directly if they
    /// are on hand, otherwise it is requested and played when it lands, which
    /// `update()` promotes on the network event. `current_path` stays `None`
    /// until then, which is what stops `advance()` cascading past a pending
    /// track.
    ///
    /// Not to be confused with the public `play_track(index)`, the Playlist
    /// pane's "play row N" verb, which additionally resets shuffle. Callers own
    /// `current_index` — `play_file` (direct open) sets `None`, `start`
    /// (playlist flow) sets the index, and neither decision belongs in here.
    ///
    /// The trap this exists to kill is `File::open("smb://…")`, which always
    /// fails *quietly* — so a caller that skips this path breaks playback
    /// silently rather than loudly. Note also that a remote track is a *ready*
    /// track whenever its spool copy exists, which is why this is one entry
    /// point and not two.
    fn play_now(&mut self, track: PathBuf) {
        if tracks::is_ready(&track) {
            self.start_track(track);
        } else {
            // No bytes yet: unload the current track now (as loading would) and
            // clear current_path so Now Playing shows "Loading from server…"
            // while `Network::pending()` is set. `Network::spool` also marks the
            // track played, so its cache file is exempt from eviction while the
            // download is still in flight — tag-driven spools go via
            // `fetch_tags` and stay evictable.
            self.fresh_sink();
            self.current_path = None;
            self.total_duration = None;
            self.network.spool(track);
        }
    }

    // ── SMB network ────────────────────────────────────────────────────────
    // State lives in `network::Network`, and the GUI reaches it through these two
    // accessors — browsing, spooling, credentials and the worker channels are
    // its own methods, not the app's. Persistence is the config compare in
    // `flush_config`, so `servers` is written whenever that list actually
    // changes and there is no wrapper here to forget to flag it.

    pub fn network(&self) -> &network::Network {
        &self.network
    }

    pub fn network_mut(&mut self) -> &mut network::Network {
        &mut self.network
    }

    // Read-only getters

    pub fn current_path(&self) -> Option<&std::path::Path> {
        self.current_path.as_deref()
    }
    pub fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
    pub fn volume(&self) -> f32 {
        self.volume
    }
    pub fn playlist(&self) -> &[PathBuf] {
        &self.playlist
    }
    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> bool {
        self.repeat
    }
    /// Visualization buffer (shared with the tap source).
    pub fn viz(&self) -> &audio::viz::VizBuf {
        &self.viz
    }

    // ── Owned state groups ───────────────────────────────────────────────
    //
    // The app *owns* four groups that have an owner of their own, and these
    // accessors are the whole boundary: a pane that draws the equalizer depends
    // on `EqSettings`, not on a set of method names this file invented for it.
    //
    // What remains on `TPlayApp` either touches state only the app has, or
    // performs an effect that spans owners (persist, re-list, scan, rebuild the
    // sink, drain the network).

    /// The six user-tunable playback settings. Their clamping lives in
    /// `Prefs`; persistence is the config compare in `flush_config`, so a
    /// caller just sets the value and forgets about the file.
    pub fn prefs(&self) -> &config::Prefs {
        &self.prefs
    }
    pub fn prefs_mut(&mut self) -> &mut config::Prefs {
        &mut self.prefs
    }

    pub fn library(&self) -> &library::LibraryState {
        &self.library
    }
    pub fn library_mut(&mut self) -> &mut library::LibraryState {
        &mut self.library
    }

    /// The 10-band EQ. Gains live in an `Arc` the running source reads, so
    /// setting one is live: no sink rebuild, no audio restart.
    pub fn eq(&self) -> &audio::eq::EqSettings {
        &self.eq
    }
    pub fn eq_mut(&mut self) -> &mut audio::eq::EqSettings {
        &mut self.eq
    }

    /// Whether any file in the browsed folder is still missing from the tag
    /// cache (i.e. its scan is pending or underway).
    ///
    /// Local by construction, not by filtering: `LibraryState::entries` is only
    /// ever filled by `navigate_to`, from `library::list_dir` on a `dir` that
    /// had to pass `is_dir()` — and an `smb://` URI never does. So there is no
    /// remote entry here to skip, and the share browser computes its own count
    /// over its own entries. Spans two owners (browse state + tag cache), which
    /// is why it is here and not on either.
    pub fn library_scanning(&self) -> bool {
        self.library.entries().iter().any(|e| {
            !e.is_dir()
                && !library::is_playlist(e.path())
                && !self.db.cache().contains_key(e.path())
        })
    }

    /// Toggle hidden-folder display and re-list the current dir so the change
    /// lands immediately. The re-list is why this is an app method and not a
    /// `library_mut().set_show_hidden(..)`.
    pub fn set_show_hidden(&mut self, show: bool) {
        if self.library.set_show_hidden(show) {
            self.navigate_to(self.library.dir().to_path_buf());
        }
    }

    /// What the play button has to do: resume/replay the loaded track, or start
    /// the first track of a non-empty playlist. False when nothing is loaded and
    /// the playlist is empty (`play()` is then a no-op), and while a track is
    /// spooling — that one starts on its own when it lands, and `play()` returns
    /// early for it, so the button would do nothing either way.
    pub fn can_play(&self) -> bool {
        if self.current_path.is_some() {
            return true;
        }
        self.network.pending().is_none() && !self.playlist.is_empty()
    }

    /// Both transport predicates ask the picker rather than re-implementing it.
    /// As independent copies of `peek_next_index` / `prev_track_index`'s
    /// branches they had already drifted: the prev one reported `true` for an
    /// empty shuffle history, lighting a button that did nothing.
    pub fn has_next_track(&self) -> bool {
        self.peek_next_index().is_some()
    }

    pub fn has_prev_track(&self) -> bool {
        self.peek_prev_index().is_some()
    }

    /// Where the playhead is, as a fraction of the track. Derived from
    /// `playback_position_secs` rather than re-reading the sink: two copies of
    /// the xf-else-main branch is how the seek bar and the time label would
    /// come to disagree about which sink is playing.
    pub fn playback_position(&self) -> f32 {
        // During crossfade/gapless the xf_sink plays the incoming track, whose
        // position starts at 0 and progresses normally.
        let pos = self.playback_position_secs().as_secs_f32();
        self.total_duration
            .map(|d| (pos / d.as_secs_f32()).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    }

    /// The playhead in seconds — the primitive both the seek bar (as a fraction)
    /// and the time label read, so they cannot disagree.
    pub fn playback_position_secs(&self) -> Duration {
        if let Some(xf) = &self.xf_sink {
            // The xf sink was built by seeking the *incoming* track, so its
            // position is already absolute — no offset.
            xf.get_pos()
        } else {
            effective_pos(self.sink.get_pos(), self.position_offset)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    pub fn seek_target(&self) -> Option<f32> {
        self.seek_target
    }

    /// Everything one frame needs from the app, in order. The shell draws first
    /// (`gui::coordinator::update_ui`), then calls this.
    ///
    /// Returns whether the GUI should keep repainting: something is in flight
    /// (a scan, a spool, a tag batch) or audio is playing. The app cannot ask
    /// for a repaint itself — it has no `egui::Context` — so each of the three
    /// things that would have called it returns a bool and the shell ORs them
    /// together.
    pub fn update(&mut self, now: f64, closing: bool, theme_id: &str) -> bool {
        self.advance();
        let scanning = self.drain_tag_scan();
        // SMB replies: browse listings are applied inside `Network::drain`; a
        // completed spool promotes playback, a fetched `.tplay` loads, and a
        // save confirms. Drain every event, not just the first — a save reply
        // must not be stranded behind an unrelated one.
        while let Some(ev) = self.network.drain() {
            match ev {
                network::Event::Spooled { uri, result } => match result {
                    Ok(_) => self.start_track(PathBuf::from(uri)),
                    Err(e) => {
                        eprintln!("tplay: spool {uri} failed: {e}");
                        self.current_path = None;
                        self.total_duration = None;
                    }
                },
                network::Event::Fetched { uri, result } => match result {
                    Ok(local) => self.load_fetched_playlist(uri, local),
                    Err(e) => eprintln!(
                        "tplay: could not fetch playlist {uri}: {e} (no session password for \
                         that share yet — log in from the Library's Network section)"
                    ),
                },
                network::Event::Saved { uri, result } => match result {
                    // Only now is the save real: track the URI, clear dirty,
                    // and re-list so the file appears in the share.
                    Ok(()) => {
                        self.playlist_file = Some(PathBuf::from(uri));
                        self.playlist_dirty = false;
                        self.refresh_network_dir();
                    }
                    Err(e) => eprintln!("tplay: could not save playlist to {uri}: {e}"),
                },
                network::Event::Tagged(results) => {
                    // Same cache the local scan fills, keyed by URI, so rows
                    // switch from filename to tagged title on their own.
                    let cache = self.db.cache_mut();
                    self.tracks.absorb(cache, results);
                    // Housekeeping after a batch, not per frame: this walks the
                    // spool dir, and tag spools are what fill it.
                    let budget = u64::from(self.spool_cache_mb).saturating_mul(1024 * 1024);
                    self.network.evict_unplayed(budget);
                }
            }
        }
        // Settings persist on a throttle, not on the setter that changed them —
        // see `config::should_flush`. Last in the frame, and after the drain, so
        // an `Event::Saved` that retargets `playlist_file` is recorded in the
        // same frame it lands.
        self.flush_config(now, closing, theme_id);
        self.flush_db(now, closing);
        scanning || self.network.busy() || (!self.sink.empty() && !self.sink.is_paused())
    }
}

/// Wall-clock seconds since the epoch, for the play history.
///
/// Not `ctx` time: the debounce clock restarts with the process, but a play
/// count and a first-seen stamp have to mean the same thing in a later session.
/// Read from the OS clock rather than `ctx` precisely so a value written to disk
/// is a real time rather than a session-relative one.
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
