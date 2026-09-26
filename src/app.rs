//! App state and logic — no UI code here.

use crate::audio;
use crate::audio::transition;
use crate::config;
use crate::gui::theme::{self, Theme, Themes};
use crate::library;
use crate::network;
use crate::tracks;
use eframe::egui;
use rodio::{cpal::BufferSize, Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Tiny inline RNG (XorShift64) - replaces fastrand dependency.
fn rand_u64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
fn rand_usize(state: &mut u64, max: usize) -> usize {
    (rand_u64(state) as usize) % max
}

// Docking panes (egui_dock) - used by GUI layer only
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub enum Pane { NowPlaying, Playlist, Equalizer, Library, Visualizer, AlbumCover }

impl Pane {
    pub const ALL: [Pane; 6] = [Pane::NowPlaying, Pane::Playlist, Pane::Equalizer, Pane::Library, Pane::Visualizer, Pane::AlbumCover];
}

/// Config types, re-exported because the GUI and the tests reach them through
/// `app`. EQ presets are deliberately NOT re-exported: they belong in
/// `audio::eq` beside the frequencies they curve.
pub use crate::config::{Config, EqData, LibraryData, VizView};

pub struct TPlayApp {
    /// Owns the cpal output stream + mixer; must outlive every Sink.
    output: OutputStream,
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
    /// Incoming track's sink during a crossfade/gapless. While `Some`, the
    /// current track plays in `sink` and the next in `xf_sink`, faded or held
    /// per frame in `advance()`.
    xf_sink: Option<Sink>,
    /// The outgoing track's duration, captured at arm time — the arm flips
    /// `total_duration` to the incoming track for the seek bar, so the fade
    /// math needs the old total.
    xf_out_total: Option<Duration>,

    /// A setting changed since the last write; `flush_config` clears it.
    config_dirty: bool,
    /// `ctx` time of the last config.json write, for the debounce window.
    last_config_save: f64,

    /// Active theme + loadable list + decoded icons.
    theme: theme::ThemeState,
    /// Needed to (re)load icon textures on theme switch.
    ctx: egui::Context,

    // ── Library pane ──────────────────────────────────────────────────────
    /// Browsed dir, rows, sort, bookmarks, hidden-folder toggle. What it can't
    /// own stays here: the tag cache, shared with the other two panes.
    library: library::LibraryState,
    /// Shared tag/duration cache for every scanned or played track, keyed by id.
    /// Filenames stand in until an entry lands.
    tag_cache: library::TagCache,
    /// Reads tags into `tag_cache`, local or remote. Owns the local scan's
    /// receiver, so the app holds no mpsc plumbing.
    tracks: tracks::TagReader,

    // ── SMB network ────────────────────────────────────────────────────────
    /// All network state + the worker channels.
    network: network::Network,
}

impl TPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let themes = Themes::load();

        let config = config::load();

        // A too-small buffer is the classic cause of ALSA "underrun occurred" at
        // track transitions (the crossfade/gapless arm decodes two files at
        // once). Fall back to the device-chosen default if it rejects the
        // fixed size.
        let output = match OutputStreamBuilder::from_default_device()
            .map(|b| b.with_buffer_size(BufferSize::Fixed(config.buffer_size.clamp(512, 65536))).open_stream())
        {
            Ok(Ok(s)) => s,
            _ => OutputStreamBuilder::open_default_stream().expect("No audio output device found"),
        };
        let sink = Sink::connect_new(output.mixer());
        let theme = theme::ThemeState::load(&ctx, themes, &config.theme);

        // Channels + worker are owned by `network::Network`.
        let mut app = Self {
            output,
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
            prefs: config::Prefs::from_config(&config),
            xf_sink: None,
            xf_out_total: None,
            config_dirty: false,
            last_config_save: 0.0,
            theme,
            ctx,
            library: library::LibraryState::new(
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
                config.library.favorites.into_iter().map(PathBuf::from).filter(|d| d.is_dir()).collect(),
                config.library.show_hidden,
            ),
            tag_cache: library::TagCache::new(),
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
        if let Some(pl_path) = config.last_playlist {
            let path = PathBuf::from(pl_path);
            if network::is_remote(&path) {
                app.fetch_remote_playlist(path.to_string_lossy().into_owned());
            } else if path.exists() {
                app.load_playlist_from(path);
            }
        }
        app.navigate_to(app.library.dir().to_path_buf());

        app
    }

    /// Record that a setting changed; `update()` writes it out (throttled).
    ///
    /// This used to *be* `save_config()`, and every settings setter called it —
    /// so a slider drag serialized the whole config.json ~60×/sec on the UI
    /// thread. `flush_config` is now the only writer.
    fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
    }

    /// Write config.json if anything changed, at most once per
    /// `CONFIG_SAVE_DEBOUNCE_SECS`, and unconditionally when closing.
    fn flush_config(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let closing = ctx.input(|i| i.viewport().close_requested());
        if !config::should_flush(self.config_dirty, now, self.last_config_save, closing) {
            return;
        }
        self.save_config();
        self.config_dirty = false;
        self.last_config_save = now;
    }

    /// Save all settings to unified config.json
    fn save_config(&self) {
        // Every field listed explicitly, no `..Default::default()`: a new
        // `Config` field must be a compile error here, not a silent reset.
        let config = Config {
            theme: self.theme.current().id.clone(),
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
            last_playlist: self.playlist_file.as_ref().and_then(|p| p.to_str()).map(str::to_owned),
            library: LibraryData {
                favorites: self.library.favorites().iter().filter_map(|d| d.to_str().map(str::to_owned)).collect(),
                last_dir: self.library.dir().to_string_lossy().into_owned(),
                show_hidden: self.library.show_hidden(),
            },
            servers: self.network.servers().to_vec(),
        };
        config::save(&config);
    }

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
    /// Shared by `start_track` and the remote spool wait (which pauses playback
    /// until the download lands).
    fn fresh_sink(&mut self) {
        self.cancel_xf();
        self.seek_target = None;
        self.position_offset = Duration::ZERO;
        self.viz.clear();
        self.sink = Sink::connect_new(self.output.mixer());
        self.sink.set_volume(self.volume);
    }

    /// Load a track's bytes into the sink. `track` is a track **id** — local
    /// path or `smb://` URI — and the only track identity the app carries;
    /// where the bytes actually are is `tracks`' problem.
    ///
    /// Every file read goes through `tracks::{open, info, probe}`, which
    /// resolve internally. That is the whole rule: an earlier version took
    /// `(local, display)` and let the caller resolve, putting two paths for one
    /// track in circulation — and `File::open("smb://…")` fails quietly rather
    /// than loudly, so a swapped pair was a silent, app-killing bug. One
    /// parameter cannot be swapped.
    fn start_track(&mut self, track: PathBuf) {
        self.fresh_sink(); // cancels any live crossfade

        // Tag up front so Now Playing shows title · artist immediately instead
        // of waiting on a scan (one file, negligible cost). Keyed by the id,
        // which is what every pane looks up by.
        if let Some(info) = tracks::info(&track) {
            self.tag_cache.insert(track.clone(), info);
        }

        let file = match tracks::open(&track) {
            Some(f) => f,
            None => { eprintln!("tplay: no file for {}", track.display()); self.current_path = None; self.total_duration = None; return; }
        };
        let decoder = match Decoder::try_from(file) {
            Ok(d) => d,
            Err(e) => { eprintln!("tplay: decode error: {e}"); self.current_path = None; self.total_duration = None; return; }
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
    /// That is the whole point of the split — the arm evaluates this every
    /// frame, and a mutating pick there rewrote the shuffle order 60 times a
    /// second. See **Shuffle order** in AGENTS.md.
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
            self.repeat.then(|| rand_usize(&mut rng, len))
        } else {
            Some(unplayed[rand_usize(&mut rng, unplayed.len())])
        };
        idx.map(|i| (i, rng))
    }

    /// Record `idx` as played: the one place the shuffle cycle advances. Called
    /// once per track that actually starts playing, so `played` changes only on
    /// a play, a playlist edit (`reset_shuffle` on add/remove/move) or a stop —
    /// never on a frame of deliberation.
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
            // walk**, however repeat is set: this used to read `self.repeat` and
            // report `true`, lighting a button that then did nothing. An empty
            // history is reachable by clicking any playlist row with shuffle on
            // — `play_track` resets it and `start` never pushes.
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

        let path = match self.current_path.clone() { Some(p) => p, None => return };
        let total_secs = match self.total_duration { Some(d) => d.as_secs_f32(), None => return };
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
        let file   = match tracks::open(&path)             { Some(f) => f, None => { eprintln!("seek: no file for {}", path.display()); return; } };
        let source = match Decoder::try_from(file)         { Ok(s) => s, Err(e) => { eprintln!("seek decode: {e}"); return; } };

        self.sink = Sink::connect_new(self.output.mixer());
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
        if was_paused { self.sink.pause(); }
    }

    pub fn advance(&mut self) {
        // 1) Settle a live xf: fade toward the swap, promote once the outgoing
        // track has drained. `xf_out_total` is the OUTGOING duration —
        // `self.total_duration` was flipped to the incoming track at arm time
        // (the seek bar reads the incoming track during the fade).
        if let Some(xf) = &self.xf_sink {
            let pos = self.sink.get_pos().saturating_add(self.position_offset);
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
                .map(|(_, p)| (tracks::is_ready(p), self.tag_cache.get(p).and_then(|i| i.duration)))
                .unzip();

            let input = transition::ArmInput {
                crossfade: self.prefs.crossfade(),
                gapless: self.prefs.gapless(),
                crossfade_secs: self.prefs.crossfade_secs(),
                total: self.total_duration,
                pos: self.sink.get_pos().saturating_add(self.position_offset),
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
                let xf_sink = Sink::connect_new(self.output.mixer());
                xf_sink.append(xf_source);
                xf_sink.set_volume(0.0);
                self.xf_sink = Some(xf_sink);
                // Pre-flip the playlist metadata so Now Playing shows the new track.
                self.current_index = Some(armed.index);
                // Tagged by the id, so every pane still finds the entry.
                if let Some(info) = tracks::info(&armed.track) {
                    self.total_duration = info.duration;
                    self.tag_cache.insert(armed.track.clone(), info);
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
        }).unwrap_or_else(|| "--:--".into())
    }

    pub fn format_freq(f: f32) -> String {
        if f >= 1000.0 { format!("{}K", (f / 1000.0) as i32) } else { format!("{}", f as i32) }
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
            let idx = rand_usize(&mut self.rng_state, len);
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
        self.mark_config_dirty();
    }

    /// Set one band's gain in dB, live on the running source — no sink rebuild,
    /// no audio restart. `EqSettings` clamps to ±12 dB and ignores an
    /// out-of-range band, so a slider re-reporting the same value costs one `if`
    /// and no config write. (The preset label is *derived* from the gains by
    /// `eq::preset_for`, so a manual tweak needs nothing marking.)
    pub fn set_eq_gain(&mut self, band: usize, gain_db: f32) {
        if self.eq.set_band(band, gain_db) { self.mark_config_dirty(); }
    }

    /// Select an EQ preset by name (see `EQ_PRESETS`), or None for custom.
    pub fn set_eq_preset(&mut self, name: Option<String>) {
        if self.eq.set_preset(name.as_deref()) { self.mark_config_dirty(); }
    }

    pub fn eq_enabled(&self) -> bool { self.eq.enabled() }

    pub fn eq_gains(&self) -> [f32; 10] { self.eq.gains() }

    /// The preset the current gains match, or `None` = Custom (the ComboBox
    /// holds a `None` option, so the caller needs the Option, not the label).
    pub fn eq_preset(&self) -> Option<&'static str> { self.eq.preset() }

    pub fn eq_preset_name(&self) -> &'static str { self.eq.preset_name() }

    /// Switch theme by id. The icon re-decode lives in `ThemeState::set` — the
    /// app no longer knows that switching a theme invalidates the textures.
    pub fn set_theme(&mut self, id: &str) {
        if self.theme.set(&self.ctx, id) { self.mark_config_dirty(); }
    }

    pub fn theme(&self) -> &Arc<Theme> { self.theme.current() }

    /// All loadable themes, for the Theme dropdown.
    pub fn themes(&self) -> &[Arc<Theme>] { self.theme.list() }

    /// Texture for a pane icon in the current theme (falls back to the
    /// default theme's), or `None` → the pane renders a unicode glyph.
    pub fn theme_icon(&self, icon: theme::Icon) -> Option<&egui::TextureHandle> {
        self.theme.icon(icon)
    }

    /// Toggle EQ on/off. Applies live — the source starts/stops filtering in place.
    pub fn toggle_eq(&mut self) {
        self.eq.toggle();
        self.mark_config_dirty();
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
        if self.current_index.is_none() {
            self.current_path = None;
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
        self.mark_config_dirty();
    }

    pub fn toggle_repeat(&mut self) {
        self.repeat = !self.repeat;
        self.mark_config_dirty();
    }

    // ── Playlists — plain `.tplay` files on disk, found in the Library like
    //    any other file. Shuffle/repeat are appwide settings (config.json),
    //    never playlist content.

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
            match library::playlist_json(&self.playlist) {
                Ok(json) => {
                    self.network.save(path.to_string_lossy().into_owned(), json);
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
        self.mark_config_dirty();
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
        let Some(paths) = library::read_playlist(&path, base) else { return };
        self.apply_playlist(paths, path);
    }

    /// Ask the worker to download a remote `.tplay`. The reply arrives as
    /// `Event::Fetched` and is applied by `load_fetched_playlist`.
    pub fn fetch_remote_playlist(&mut self, uri: String) {
        self.network.fetch(uri);
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
        self.apply_playlist(paths, PathBuf::from(uri));
    }

    /// Shared tail of both playlist-load paths: filter, stop, scan, track.
    fn apply_playlist(&mut self, paths: Vec<PathBuf>, file: PathBuf) {
        // Remote tracks can't canonicalize and are kept verbatim; local paths
        // that no longer exist are dropped.
        self.playlist = paths.into_iter().filter_map(tracks::normalize).collect();
        self.stop();
        self.ensure_tags(self.playlist.clone());
        self.playlist_file = Some(file);
        self.playlist_dirty = false;
        self.mark_config_dirty();
    }

    /// Clear the playlist for a fresh build (confirm dialog lives in the GUI).
    /// Unsets the tracked file — a new playlist must ask where it's saved.
    pub fn new_playlist(&mut self) {
        self.stop();
        self.playlist.clear();
        self.playlist_file = None;
        self.playlist_dirty = false;
        self.mark_config_dirty();
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

    /// List a directory and start tagging its audio files in the background.
    /// Persists the last browsed dir on the way.
    ///
    /// The split is the point: `LibraryState::open` owns *what the folder
    /// contains and how it sorts*, and returns the paths that need scanning.
    /// Starting that scan stays here, because the tag cache and the `TagReader`
    /// are the app's, shared with the other two panes.
    pub fn navigate_to(&mut self, dir: PathBuf) {
        let Some(scan) = self.library.open(dir, &self.tag_cache) else { return };
        self.ensure_tags(scan);
        self.mark_config_dirty();
    }

    /// Header click: pick a new column (ascending) or flip the active one and
    /// re-sort the current folder in place.
    pub fn set_library_sort(&mut self, key: usize) {
        if self.library.set_sort(key, &self.tag_cache) {
            self.mark_config_dirty();
        }
    }

    /// Ensure the given audio files have tag info in the cache.
    ///
    /// Which transport a track needs is not this function's business — the
    /// `TagReader` splits the batch and hands each half to the thread or the SMB
    /// worker. Both land in the same `tag_cache`, remote keyed by URI, so every
    /// pane fills in identically.
    ///
    /// Safe to call every frame: the reader skips cached tracks and the network
    /// side keeps its own in-flight set, so a running batch is not re-queued.
    pub fn ensure_tags(&mut self, paths: Vec<PathBuf>) {
        let cache = &self.tag_cache;
        let started = self.tracks.request(cache, &mut self.network, &paths);
        if started {
            self.ctx.request_repaint();
        }
    }

    /// Drain finished local tag results into the cache (called every frame).
    pub fn drain_tag_scan(&mut self) {
        let cache = &mut self.tag_cache;
        if self.tracks.drain_into(cache) {
            self.ctx.request_repaint();
        }
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
    /// The trap this exists to kill: opening a track id directly on a remote
    /// track is `File::open("smb://…")`, which always fails *quietly*, so a
    /// caller that skips this path breaks playback silently rather than loudly.
    /// Note also that a remote track is a *ready* track whenever its spool copy
    /// exists — which is why this is one entry point and not two.
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

    pub fn toggle_favorite(&mut self, dir: PathBuf) {
        self.library.toggle_favorite(dir);
        self.mark_config_dirty();
    }

    // ── SMB network ────────────────────────────────────────────────────────
    // State lives in `network::Network`; these are the persistence-aware seams
    // (config.json ownership stays in app.rs) + accessors for the GUI and
    // `update()`. Browsing/spool/creds/drain are methods on the network object.

    pub fn network(&self) -> &network::Network {
        &self.network
    }

    pub fn network_mut(&mut self) -> &mut network::Network {
        &mut self.network
    }

    /// Add a server (dedup by host; re-adding updates the username). Persists.
    pub fn add_network_server(&mut self, host: String, username: String) {
        self.network.add_server(host, username);
        self.mark_config_dirty();
    }

    /// Remove a saved server; persists. Passwords stay in the session map and
    /// are never written.
    pub fn remove_network_server(&mut self, host: &str) {
        self.network.remove_server(host);
        self.mark_config_dirty();
    }

    /// Session-memory password for a host, never persisted — set at add-time
    /// via the Library's add-server form, consumed per connect.
    pub fn set_network_password(&mut self, host: String, password: String) {
        self.network.set_password(host, password);
    }

    // Read-only getters

    pub fn current_path(&self) -> Option<&std::path::Path> { self.current_path.as_deref() }
    pub fn total_duration(&self) -> Option<Duration> { self.total_duration }
    pub fn volume(&self) -> f32 { self.volume }
    pub fn playlist(&self) -> &[PathBuf] { &self.playlist }
    pub fn current_index(&self) -> Option<usize> { self.current_index }
    pub fn shuffle(&self) -> bool { self.shuffle }
    pub fn repeat(&self) -> bool { self.repeat }
    /// Visualization buffer (shared with the tap source).
    pub fn viz(&self) -> &audio::viz::VizBuf { &self.viz }

    // ── Playback settings ────────────────────────────────────────────────
    //
    // The values live in `config::Prefs`; these are delegates, kept because the
    // GUI reaches state through `TPlayApp` and nothing else. The setters are
    // three lines rather than six because `Prefs` owns the clamping and reports
    // whether anything changed — so an unchanged value costs one `if` and never
    // marks the config dirty.

    /// Visualizer pane view — see `VizView::ALL`.
    pub fn viz_view(&self) -> VizView { self.prefs.viz_view() }

    pub fn set_viz_view(&mut self, view: VizView) {
        if self.prefs.set_viz_view(view) { self.mark_config_dirty(); }
    }

    /// Current balance (-1.0..=1.0).
    pub fn balance(&self) -> f32 { self.prefs.balance() }

    /// Set balance, live with no sink rebuild — `Prefs` holds the handle the
    /// audio source reads per frame, so there is no sink to rebuild here.
    pub fn set_balance(&mut self, v: f32) {
        if self.prefs.set_balance(v) { self.mark_config_dirty(); }
    }

    /// Whether to show remaining time instead of elapsed.
    pub fn remaining(&self) -> bool { self.prefs.remaining() }

    pub fn set_remaining(&mut self, remaining: bool) {
        if self.prefs.set_remaining(remaining) { self.mark_config_dirty(); }
    }

    /// Whether gapless playback is enabled.
    pub fn gapless(&self) -> bool { self.prefs.gapless() }

    pub fn toggle_gapless(&mut self) {
        self.prefs.toggle_gapless();
        self.mark_config_dirty();
    }

    /// Whether crossfade playback is enabled.
    pub fn crossfade(&self) -> bool { self.prefs.crossfade() }

    pub fn toggle_crossfade(&mut self) {
        self.prefs.toggle_crossfade();
        self.mark_config_dirty();
    }

    /// Crossfade duration in seconds.
    pub fn crossfade_secs(&self) -> f32 { self.prefs.crossfade_secs() }

    pub fn set_crossfade_secs(&mut self, secs: f32) {
        if self.prefs.set_crossfade_secs(secs) { self.mark_config_dirty(); }
    }

    pub fn library_dir(&self) -> &std::path::Path { self.library.dir() }
    pub fn library_entries(&self) -> &[library::Entry] { self.library.entries() }
    /// The whole tag cache, so a caller can sort a list of entries against it
    /// (`library::sort_entries`) rather than sorting entry-by-entry. Keys are
    /// track ids, so a remote one is an `smb://` URI.
    pub fn tag_cache(&self) -> &library::TagCache {
        &self.tag_cache
    }

    pub fn track_info(&self, path: &std::path::Path) -> Option<&library::TrackInfo> {
        self.tag_cache.get(path)
    }
    /// Whether any file in the browsed folder is still missing from the tag
    /// cache (i.e. its scan is pending or underway).
    ///
    /// Local by construction, not by filtering: `library_entries` is only ever
    /// filled by `navigate_to`, from `library::list_dir` on a `library_dir` that
    /// had to pass `dir.is_dir()` — and an `smb://` URI never does. So there is
    /// no remote entry here to skip, and the share browser computes its own
    /// count over its own entries.
    pub fn library_scanning(&self) -> bool {
        self.library
            .entries()
            .iter()
            .any(|e| !e.is_dir() && !library::is_playlist(e.path()) && !self.tag_cache.contains_key(e.path()))
    }
    pub fn favorite_dirs(&self) -> &[PathBuf] { self.library.favorites() }
    pub fn is_favorite(&self, dir: &std::path::Path) -> bool { self.library.is_favorite(dir) }

    /// Fixed user-folder shortcuts (Home + XDG user dirs) shown above the
    /// Favorites list in the Library pane. The logic is `library`'s — it never
    /// touched app state — so this delegate exists only to keep the GUI's
    /// uniform `app.*()` call shape.
    pub fn quick_folders(&self) -> Vec<(String, PathBuf)> {
        library::quick_folders()
    }

    pub fn show_hidden(&self) -> bool { self.library.show_hidden() }

    pub fn library_sort(&self) -> usize { self.library.sort() }
    pub fn library_sort_asc(&self) -> bool { self.library.sort_asc() }

    /// Toggle hidden-folder display and re-list the current dir so the change
    /// lands immediately (also persists it).
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

    pub fn playback_position(&self) -> f32 {
        // During crossfade/gapless the xf_sink plays the incoming track, whose
        // position starts at 0 and progresses normally.
        let (pos, total) = if let Some(xf) = &self.xf_sink {
            (xf.get_pos(), self.total_duration)
        } else {
            (self.sink.get_pos().saturating_add(self.position_offset), self.total_duration)
        };
        total
            .map(|d| (pos.as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    }

    pub fn playback_position_secs(&self) -> Duration {
        if let Some(xf) = &self.xf_sink {
            xf.get_pos()
        } else {
            self.sink.get_pos().saturating_add(self.position_offset)
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
}

impl eframe::App for TPlayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::gui::coordinator::update_ui(self, ctx);
        self.advance();
        self.drain_tag_scan();
        // Settings persist on a throttle, not on the setter that changed them —
        // see `config::should_flush`. Last in the frame, so a click that both
        // arms a dialog and moves a slider is already recorded.
        self.flush_config(ctx);
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
                        self.mark_config_dirty();
                        self.refresh_network_dir();
                    }
                    Err(e) => eprintln!("tplay: could not save playlist to {uri}: {e}"),
                },
                network::Event::Tagged(results) => {
                    // Same cache the local scan fills, keyed by URI, so rows
                    // switch from filename to tagged title on their own.
                    let cache = &mut self.tag_cache;
                    self.tracks.absorb(cache, results);
                    // Housekeeping after a batch, not per frame: this walks the
                    // spool dir, and tag spools are what fill it.
                    let budget = u64::from(self.spool_cache_mb).saturating_mul(1024 * 1024);
                    self.network.evict_unplayed(budget);
                }
            }
        }
        let waiting = self.network.busy();
        if waiting || (!self.sink.empty() && !self.sink.is_paused()) {
            ctx.request_repaint();
        }
    }
}