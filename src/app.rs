//! App state and logic — no UI code here.

use crate::audio;
use crate::audio::transition;
use crate::gui::theme::{self, Theme, Themes};
use crate::library;
use eframe::egui;
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// How far before track end (seconds) to arm gapless/crossfade next track.
const PREROLL_SECS: f32 = 2.0;

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

/// Visualizer views (serde'd into config.json `viz_view`). The pane matches
/// on this; the app just stores/serializes it — same shape as `Pane`.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum VizView {
    #[default]
    Bars,
    Wave,
}

impl VizView {
    pub const ALL: [VizView; 2] = [VizView::Bars, VizView::Wave];

    /// Dropdown label in the visualizer pane header.
    pub fn name(self) -> &'static str {
        match self {
            VizView::Bars => "Bars",
            VizView::Wave => "Wave",
        }
    }
}

/// Unified config — single JSON file. Dock layout stays separate.
/// `pub` so the integration tests can pin the on-disk shape.
#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    pub theme: String,
    #[serde(default)]
    pub eq: EqData,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub viz_view: VizView,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub last_playlist: Option<String>,
    #[serde(default)]
    pub library: LibraryData,
    /// Balance (L/R) — -1 = full left, 0 = center, 1 = full right.
    #[serde(default)]
    pub balance: f32,
    /// Show remaining time instead of elapsed.
    #[serde(default)]
    pub remaining: bool,
    /// Gapless playback — pre-buffer the next track.
    #[serde(default)]
    pub gapless: bool,
    /// Crossfade playback — overlap tracks with a fade.
    #[serde(default)]
    pub crossfade: bool,
    /// Crossfade duration in seconds.
    #[serde(default = "default_crossfade_secs")]
    pub crossfade_secs: f32,
}

fn default_volume() -> f32 { 1.0 }
fn default_crossfade_secs() -> f32 { 3.0 }

#[derive(Serialize, Deserialize, Default)]
pub struct EqData {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gains")]
    pub gains: [f32; 10],
}

fn default_gains() -> [f32; 10] { [0.0; 10] }

#[derive(Serialize, Deserialize, Default)]
pub struct LibraryData {
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub last_dir: String,
    #[serde(default)]
    pub show_hidden: bool,
}

/// Generic config persistence: create dir, serialize/deserialize JSON.
fn config_path(name: &str) -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(name))
}

fn save_config<T: Serialize>(name: &str, data: &T) {
    if let Some(path) = config_path(name) {
        if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
        if let Ok(json) = serde_json::to_string_pretty(data) { let _ = fs::write(&path, json); }
    }
}

fn load_config<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    config_path(name).and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Equalizer presets — Flat is the reset. A manually tweaked slider
/// switches the selection to Custom (None).
/// Curves follow sfxengine.com/blog/best-equalizer-settings-for-music:
/// Flat ⇐ Flat, Rock ⇐ Rock/Metal, Pop ⇐ V-Shape, Jazz ⇐ Treble Boost,
/// Classical ⇐ gentle V-Shape, Electronic ⇐ Bass Boost, Vocal ⇐ Vocal Enhancement.
pub const EQ_PRESETS: [(&str, [f32; 10]); 7] = [
    ("Flat", [0.0; 10]),
    ("Rock", [2.0, 2.5, 3.0, -1.0, 0.0, 3.0, 2.0, 0.5, 0.5, 0.0]),
    ("Pop", [3.0, 2.5, 1.0, -0.5, -0.5, -1.5, 1.0, 2.0, 2.5, 2.0]),
    ("Jazz", [0.0, 0.5, 0.5, 0.0, 0.5, 1.0, 2.5, 2.0, 1.5, 1.0]),
    ("Classical", [2.5, 2.0, 0.5, 0.0, -0.5, -1.0, 0.5, 1.5, 2.0, 1.5]),
    ("Electronic", [4.0, 5.0, -2.0, -1.0, 0.0, 0.0, 0.5, 1.0, 1.0, 0.5]),
    ("Vocal", [0.0, -2.0, -1.0, 0.0, 0.5, 3.0, 1.5, -1.0, 0.0, 0.0]),
];

pub struct TPlayApp {
    /// Owns the cpal output stream + mixer; must outlive every Sink.
    output: OutputStream,
    sink: Sink,

    current_path: Option<PathBuf>,
    total_duration: Option<Duration>,

    volume: f32,

    /// Holds the slider at the intended position until get_pos() catches up,
    /// preventing snap-back to 0 during a skip_duration seek.
    seek_target: Option<f32>,

    /// The slow-path seek rebuilds the sink with a skip_duration(target) source;
    /// rodio's get_pos() only counts post-skip samples, so the fresh sink
    /// under-reports by exactly the skip amount. Compensate for it.
    position_offset: Duration,

    playlist: Vec<PathBuf>,
    /// Index into `playlist` of the track currently loaded, set when playback
    /// starts from the pane (row click, auto-advance). None = direct open,
    /// which breaks the sequential flow.
    current_index: Option<usize>,
    /// The `.tplay` file this playlist was saved to / loaded from. `Some` →
    /// Save Playlist overwrites it directly; `None` → Save opens the dialog.
    /// Cleared by New Playlist (a fresh playlist has no file to replace yet).
    playlist_file: Option<PathBuf>,
    /// Whether the in-memory playlist differs from its saved/loaded file —
    /// i.e. New/Load risk losing unsaved edits. Set on content changes,
    /// cleared on save/load/new (shuffle/repeat don't count; they're appwide).
    playlist_dirty: bool,

    /// Shuffle mode: play tracks in random order
    shuffle: bool,
    /// Repeat mode: loop playlist (or random if shuffle also on)
    repeat: bool,

    /// Indices already played in this shuffle cycle (history for prev/next).
    played: Vec<usize>,
    /// RNG state for shuffle (XorShift64).
    rng_state: u64,

    /// Equalizer state — single source of truth shared with EqSource.
    eq_shared: Arc<RwLock<audio::eq::EqShared>>,
    /// Visualization ring buffer — written by the tap source, read by the GUI.
    viz: audio::viz::VizBuf,
    /// Visualizer pane view (index into `VizView::ALL`). Persisted in config.json.
    viz_view: VizView,

    /// Balance (L/R) — shared with BalanceSource (live, like EqShared).
    balance: Arc<RwLock<f32>>,
    /// Show remaining time instead of elapsed.
    remaining: bool,
    /// Gapless playback — pre-buffer the next track.
    gapless: bool,
    /// Crossfade playback — overlap tracks with a fade.
    crossfade: bool,
    /// Crossfade duration in seconds.
    crossfade_secs: f32,
    /// Crossfade sink — incoming track during overlap. When Some, the current
    /// track plays in `sink` and the next track plays in `xf_sink`; volumes
    /// are faded (crossfade) or held (gapless) per frame in `advance()`.
    xf_sink: Option<Sink>,
    /// The outgoing track's total duration, captured at arm time. The arm
    /// flips `total_duration` to the incoming track (for the seek bar), so
    /// the fade math needs the outgoing total saved separately.
    xf_out_total: Option<Duration>,

    /// Theme (loaded from themes/ dirs), applied to egui visuals by the GUI layer.
    theme: Arc<Theme>,
    /// All loadable themes (Theme dropdown + icon fallback).
    themes: Themes,
    /// Current theme's icons, one `Option` per `Icon::ALL` slot (None → glyph).
    icons: Vec<Option<egui::TextureHandle>>,
    /// Needed to (re)load icon textures on theme switch.
    ctx: egui::Context,

    // ── Library pane ──────────────────────────────────────────────────────
    /// Directory currently browsed in the Library pane.
    library_dir: PathBuf,
    /// Rows of `library_dir`: subfolders + audio files in one list, sorted by
    /// `library_sort`. Folders are untagged — Title treats them by name, the
    /// tag columns sink them last.
    library_entries: Vec<library::Entry>,
    /// Shared tag/duration cache — any path ever scanned or played, used by
    /// the Library, Playlist, and Now Playing panes. Filenames stand in until
    /// a track's entry lands.
    tag_cache: HashMap<PathBuf, library::TrackInfo>,
    /// Receiver for the background tag scan. `Some` while a scan is running;
    /// a new request replaces it (one scan at a time).
    tag_scan_rx: Option<Receiver<(PathBuf, library::TrackInfo)>>,
    /// Bookmarked folders shown in the Library pane, persisted to disk.
    favorite_dirs: Vec<PathBuf>,
    /// Whether the Library lists dot-prefixed (hidden) subfolders. Default
    /// off; toggled from the ☰ menu, persisted in config.json.
    show_hidden: bool,
    /// Library list sort: index into `library::SORT_OPTIONS` (0 = Title, the
    /// default), plus direction. Set by header clicks.
    library_sort: usize,
    library_asc: bool,
}

impl TPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let output = OutputStreamBuilder::open_default_stream().expect("No audio output device found");
        let sink = Sink::connect_new(output.mixer());

        let themes = Themes::load();

        // Load unified config
        let config = load_config::<Config>("config.json").unwrap_or_default();
        let theme = themes
            .get(&config.theme)
            .cloned()
            .unwrap_or_else(|| themes.default().clone());
        let icons = theme::load_icons(&ctx, &themes, &theme);

        let mut app = Self {
            output,
            sink,
            current_path: None,
            total_duration: None,
            volume: config.volume,
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
            eq_shared: Arc::new(RwLock::new(audio::eq::EqShared {
                gains: config.eq.gains,
                enabled: config.eq.enabled,
            })),
            viz: audio::viz::VizBuf::new(),
            viz_view: config.viz_view,
            balance: Arc::new(RwLock::new(config.balance)),
            remaining: config.remaining,
            gapless: config.gapless,
            crossfade: config.crossfade,
            crossfade_secs: config.crossfade_secs,
            xf_sink: None,
            xf_out_total: None,
            theme,
            themes,
            icons,
            ctx,
            library_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            library_entries: Vec::new(),
            tag_cache: HashMap::new(),
            tag_scan_rx: None,
            favorite_dirs: config.library.favorites.into_iter().map(PathBuf::from).filter(|d| d.is_dir()).collect(),
            show_hidden: config.library.show_hidden,
            library_sort: 0,
            library_asc: true,
        };

        // Apply volume to sink
        app.sink.set_volume(app.volume);

        // Restore library directory
        let p = PathBuf::from(&config.library.last_dir);
        if p.is_dir() {
            app.library_dir = p;
        }

        // Restore last playlist if exists
        if let Some(pl_path) = config.last_playlist {
            let path = PathBuf::from(pl_path);
            if path.exists() {
                app.load_playlist_from(path);
            }
        }
        app.navigate_to(app.library_dir.clone());

        app
    }

    pub(crate) fn audio_dialog() -> rfd::FileDialog {
        rfd::FileDialog::new().add_filter("Audio", &["mp3", "wav", "ogg", "flac", "m4a"])
    }

    /// Native Yes/No confirm for actions that lose state (New Playlist, Load
    /// playlist, remove track). `at_risk` false → no dialog, caller proceeds.
    pub(crate) fn confirm(title: &str, description: &str, at_risk: bool) -> bool {
        if !at_risk {
            return true;
        }
        match rfd::MessageDialog::new()
            .set_title(title)
            .set_description(description)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
        {
            rfd::MessageDialogResult::Yes => true,
            _ => false,
        }
    }

    /// Save all settings to unified config.json
    fn save_config(&self) {
        let eq_shared = self.eq_shared.read().unwrap();
        let balance = self.balance.read().unwrap();
        let config = Config {
            theme: self.theme.id.clone(),
            eq: EqData {
                enabled: eq_shared.enabled,
                gains: eq_shared.gains,
            },
            shuffle: self.shuffle,
            repeat: self.repeat,
            viz_view: self.viz_view,
            volume: self.volume,
            last_playlist: self.playlist_file.as_ref().and_then(|p| p.to_str()).map(str::to_owned),
            library: LibraryData {
                favorites: self.favorite_dirs.iter().filter_map(|d| d.to_str().map(str::to_owned)).collect(),
                last_dir: self.library_dir.to_string_lossy().into_owned(),
                show_hidden: self.show_hidden,
            },
            balance: *balance,
            remaining: self.remaining,
            gapless: self.gapless,
            crossfade: self.crossfade,
            crossfade_secs: self.crossfade_secs,
        };
        save_config("config.json", &config);
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

    fn load_file(&mut self, path: PathBuf) {
        // Cancel any live crossfade when a new track is loaded explicitly.
        self.cancel_xf();

        self.seek_target     = None;
        self.position_offset = Duration::ZERO;
        self.viz.clear();

        self.sink = Sink::connect_new(self.output.mixer());
        self.sink.set_volume(self.volume);

        // Tag the loaded track up front so Now Playing shows title · artist
        // immediately instead of waiting on a scan (one file, negligible cost).
        if let Some(info) = library::read_info(&path) {
            self.tag_cache.insert(path.clone(), info);
        }

        // Open file once, create decoder, get total_duration.
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) => { eprintln!("tplay: open error: {e}"); self.current_path = None; self.total_duration = None; return; }
        };
        let decoder = match Decoder::try_from(file) {
            Ok(d) => d,
            Err(e) => { eprintln!("tplay: decode error: {e}"); self.current_path = None; self.total_duration = None; return; }
        };
        let total_duration = decoder.total_duration().or_else(|| audio::probe_duration(&path));
        self.total_duration = total_duration;

        // Always load the full track (no truncation, no pre-mix).
        // Crossfade/gapless are handled by the separate xf_sink in advance().
        let eq_source = audio::eq::EqSource::new(decoder, Arc::clone(&self.eq_shared));
        let tap_source = audio::viz::TapSource::new(eq_source, self.viz.clone());
        let balance_source = audio::balance::BalanceSource::new(tap_source, Arc::clone(&self.balance));
        self.sink.append(balance_source);
        self.current_path = Some(path);
    }

    /// Cancel any live crossfade/gapless: stop the xf sink and restore main volume.
    fn cancel_xf(&mut self) {
        if let Some(xf) = self.xf_sink.take() {
            let _ = xf.stop();
        }
        self.xf_out_total = None;
        // Restore main sink to full volume.
        self.sink.set_volume(self.volume);
    }

    fn next_track_index(&mut self) -> Option<usize> {
        if self.playlist.is_empty() {
            return None;
        }
        let len = self.playlist.len();
        if len == 1 {
            return self.repeat.then_some(0);
        }

        if !self.shuffle {
            return match (self.repeat, self.current_index) {
                (false, Some(i)) if i + 1 < len => Some(i + 1),
                (true, Some(i)) => Some((i + 1) % len),
                (true, None) => Some(0),
                _ => None,
            };
        }

        // Shuffle mode: pick random from unplayed
        let unplayed: Vec<usize> = (0..len).filter(|i| !self.played.contains(i)).collect();
        if unplayed.is_empty() {
            if self.repeat {
                self.played.clear();
                // Pick from all tracks
                let idx = rand_usize(&mut self.rng_state, len);
                self.played.push(idx);
                return Some(idx);
            }
            return None;
        }
        let idx = unplayed[rand_usize(&mut self.rng_state, unplayed.len())];
        self.played.push(idx);
        Some(idx)
    }

    fn reset_shuffle(&mut self) {
        self.played.clear();
    }

    fn prev_track_index(&mut self) -> Option<usize> {
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

        // Shuffle mode: pop from history
        if self.played.len() > 1 {
            self.played.pop();
            self.played.last().copied()
        } else if self.repeat && !self.played.is_empty() {
            // Loop back to last played
            self.played.last().copied()
        } else {
            None
        }
    }

    pub fn seek(&mut self, progress: f32) {
        self.seek_target = Some(progress);
        // Seeking cancels any live crossfade/gapless.
        self.cancel_xf();

        let path = match self.current_path.clone() { Some(p) => p, None => return };
        let total_secs = match self.total_duration { Some(d) => d.as_secs_f32(), None => return };
        let target = Duration::from_secs_f32((progress * total_secs).max(0.0));

        if self.sink.try_seek(target).is_ok() {
            // The seek landed in-place: TrackPosition now reports the new
            // position, so the slow path's skip offset no longer applies.
            self.position_offset = Duration::ZERO;
            return;
        }

        let was_paused = self.sink.is_paused();

        let file   = match File::open(&path)             { Ok(f) => f, Err(e) => { eprintln!("seek open: {e}");   return; } };
        let source = match Decoder::try_from(file)         { Ok(s) => s, Err(e) => { eprintln!("seek decode: {e}"); return; } };

        self.sink = Sink::connect_new(self.output.mixer());
        self.sink.set_volume(self.volume);
        // get_pos() on the fresh sink counts only post-skip samples; the
        // skipped `target` is the new position offset from here on.
        self.position_offset = target;
        // Use seek_or_skip (fast path for seekable formats, fallback to skip_duration)
        let seeked_source = transition::seek_or_skip(source, target);
        let eq_source = audio::eq::EqSource::new(seeked_source, Arc::clone(&self.eq_shared));
        let tap_source = audio::viz::TapSource::new(eq_source, self.viz.clone());
        let balance_source = audio::balance::BalanceSource::new(tap_source, Arc::clone(&self.balance));
        self.sink.append(balance_source);
        if was_paused { self.sink.pause(); }
    }

    pub fn advance(&mut self) {
        // 1) Settle a live xf: fade volumes toward the swap, promote when the
        // outgoing track has drained. `xf_out_total` is the OUTGOING track's
        // duration — `self.total_duration` was flipped to the incoming track
        // at arm time (the seek bar reads the incoming track during the fade).
        if let Some(xf) = &self.xf_sink {
            let pos = self.sink.get_pos().saturating_add(self.position_offset);
            let out_total = self.xf_out_total.unwrap_or_default();
            if self.sink.empty() {
                self.finish_xf();
                return;
            }
            let remaining = out_total.saturating_sub(pos);
            if self.crossfade {
                // Equal-power fade over the crossfade window. Probe drift can
                // push remaining past cf — the clamp keeps p in [0, 1].
                let p = (1.0 - remaining.as_secs_f32() / self.crossfade_secs).clamp(0.0, 1.0);
                let (out_gain, in_gain) = transition::fade_gains(p);
                self.sink.set_volume(self.volume * out_gain);
                xf.set_volume(self.volume * in_gain);
            } else {
                // Gapless: incoming sits silent until the swap — zero gap, no overlap.
                self.sink.set_volume(self.volume);
                xf.set_volume(0.0);
            }
            return;
        }

        // 2) No live xf — arm one when the current track nears its end.
        // Playing from playlist, unpaused, exactly one item queued (the
        // current track, nothing pre-buffered), and a mode enabled.
        if self.current_index.is_some()
            && self.current_path.is_some()
            && !self.sink.is_paused()
            && self.sink.len() == 1
            && (self.crossfade || self.gapless)
        {
            let total = match self.total_duration { Some(d) => d, None => return };
            let pos = self.sink.get_pos().saturating_add(self.position_offset);
            let remaining_secs = total.saturating_sub(pos).as_secs_f32();

            let cf = Duration::from_secs_f32(self.crossfade_secs);
            // Crossfade: arm within cf seconds of end (track longer than cf).
            // Gapless: arm within PREROLL_SECS (the silent pre-buffer hold).
            let should_arm = if self.crossfade {
                remaining_secs <= self.crossfade_secs && total > cf
            } else {
                remaining_secs <= PREROLL_SECS
            };

            if should_arm {
                if let Some(next_idx) = self.next_track_index() {
                    let next_path = self.playlist[next_idx].clone();
                    // A missing file can't be pre-buffered — skip the arm and
                    // let natural advance fail it gracefully via load_file.
                    if !next_path.is_file() {
                        return;
                    }
                    // A track shorter than the hold/fade window drains muted
                    // before the swap (gapless) or mid-fade (crossfade) and
                    // would be promoted empty — silently skipped. Prefer a
                    // natural-advance gap instead. Untagged tracks are allowed
                    // through (they're real music in practice).
                    let hold = if self.crossfade {
                        cf
                    } else {
                        Duration::from_secs_f32(remaining_secs)
                    };
                    if let Some(d) = self.tag_cache.get(&next_path).and_then(|i| i.duration) {
                        if d < hold {
                            return;
                        }
                    }
                    // Capture the outgoing duration for the fade math BEFORE
                    // flipping total_duration to the incoming track below.
                    self.xf_out_total = Some(total);
                    // Build the incoming track source (full track, buffered).
                    let xf_source = transition::build_gapless_next(
                        next_path.clone(),
                        Arc::clone(&self.eq_shared),
                        Arc::clone(&self.balance),
                        self.viz.clone(),
                    );
                    // Second sink on the same mixer — plays simultaneously.
                    let xf_sink = Sink::connect_new(self.output.mixer());
                    xf_sink.append(xf_source);
                    xf_sink.set_volume(0.0);
                    self.xf_sink = Some(xf_sink);
                    // Pre-flip the playlist metadata so Now Playing shows the new track.
                    self.current_index = Some(next_idx);
                    if let Some(info) = library::read_info(&next_path) {
                        self.total_duration = info.duration;
                        self.tag_cache.insert(next_path.clone(), info);
                    }
                    if self.total_duration.is_none() {
                        self.total_duration = audio::probe_duration(&next_path);
                    }
                    self.current_path = Some(next_path);
                    self.seek_target = None;
                    return;
                }
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

    /// Called when the outgoing track ends during a crossfade/gapless.
    /// Promotes the incoming sink to the main sink, restores full volume.
    fn finish_xf(&mut self) {
        if let Some(xf) = self.xf_sink.take() {
            let _ = self.sink.stop();
            // Promote the incoming sink; its position is self-relative.
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

    /// Public actions called by GUI layer

    pub fn play(&mut self) {
        if self.sink.is_paused() && !self.sink.empty() {
            self.sink.play();
            if let Some(xf) = &self.xf_sink {
                xf.play();
            }
        } else if let Some(path) = self.current_path.clone() {
            self.load_file(path);
        } else if !self.playlist.is_empty() {
            self.play_first_track();
        } else {
            if let Some(path) = Self::audio_dialog().pick_file() {
                self.add_files(vec![path]);
                self.play_first_track();
            }
        }
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

    /// Stop playback and reset song + playlist state: unloads the current
    /// track and rewinds the play position, so the next Play starts from the
    /// top of the playlist (or shuffle order) instead of resuming.
    pub fn stop(&mut self) {
        self.cancel_xf();
        let _ = self.sink.stop();
        self.current_path = None;
        self.current_index = None;
        self.total_duration = None;
        self.seek_target = None;
        self.position_offset = Duration::ZERO;
        self.viz.clear();
        self.reset_shuffle();
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.sink.set_volume(volume);
        // A live xf's volume is re-applied each frame in advance() scaled by
        // `self.volume`, so changing volume mid-fade lands on the next frame.
        self.save_config();
    }

    /// Set gain for one EQ band (0-9), in dB (-12 to +12). Applies live to the
    /// running source — no sink rebuild, no audio restart. Marks the selection
    /// as Custom (no preset name applies anymore).
    pub fn set_eq_gain(&mut self, band: usize, gain_db: f32) {
        if band >= 10 { return; }
        {
            let mut shared = self.eq_shared.write().unwrap();
            shared.gains[band] = gain_db.clamp(-12.0, 12.0);
        }
        self.save_config();
    }

    /// Select an EQ preset by name (see `EQ_PRESETS`), or None for custom.
    pub fn set_eq_preset(&mut self, name: Option<String>) {
        if let Some(name) = name.as_ref() {
            if let Some((_, gains)) = EQ_PRESETS.iter().find(|(n, _)| n == name) {
                self.eq_shared.write().unwrap().gains = *gains;
            }
        }
        self.save_config();
    }

    pub fn eq_enabled(&self) -> bool {
        self.eq_shared.read().unwrap().enabled
    }

    pub fn eq_gains(&self) -> [f32; 10] {
        self.eq_shared.read().unwrap().gains
    }

    pub fn eq_preset(&self) -> Option<&str> {
        let gains = self.eq_gains();
        EQ_PRESETS.iter()
            .find(|(_, g)| *g == gains)
            .map(|(n, _)| *n)
    }

    pub fn eq_preset_name(&self) -> &str {
        self.eq_preset().unwrap_or("Custom")
    }

    /// Switch theme by id (from the Theme dropdown); persisted, applied the
    /// same frame by the GUI layer, icons re-decoded for the new palette.
    pub fn set_theme(&mut self, id: &str) {
        if let Some(theme) = self.themes.get(id) {
            self.theme = Arc::clone(theme);
            self.icons = theme::load_icons(&self.ctx, &self.themes, &self.theme);
            self.save_config();
        }
    }

    pub fn theme(&self) -> &Arc<Theme> {
        &self.theme
    }

    /// All loadable themes, for the Theme dropdown.
    pub fn themes(&self) -> &[Arc<Theme>] {
        self.themes.list()
    }

    /// Texture for a pane icon in the current theme (falls back to the
    /// default theme's), or `None` → the pane renders a unicode glyph.
    pub fn theme_icon(&self, icon: theme::Icon) -> Option<&egui::TextureHandle> {
        self.icons.get(icon.index()).and_then(|t| t.as_ref())
    }

    /// Toggle EQ on/off. Applies live — the source starts/stops filtering in place.
    pub fn toggle_eq(&mut self) {
        {
            let mut shared = self.eq_shared.write().unwrap();
            shared.enabled = !shared.enabled;
        }
        self.save_config();
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
        self.save_config();
    }

    pub fn toggle_repeat(&mut self) {
        self.repeat = !self.repeat;
        self.save_config();
    }

    // ── Playlists — plain `.tplay` files on disk, found in the Library like
    //    any other file. Shuffle/repeat are appwide settings (config.json),
    //    never playlist content.

    /// Write the current playlist to a `.tplay` file (paths only) and record
    /// it as the file future saves overwrite without re-opening the dialog.
    pub fn save_playlist_to(&mut self, path: PathBuf) {
        if let Err(e) = library::write_playlist(&path, &self.playlist) {
            eprintln!("tplay: could not write playlist {}: {}", path.display(), e);
        }
        self.playlist_file = Some(path);
        self.playlist_dirty = false;
        self.save_config();
    }

    /// Replace the current playlist from a `.tplay` file. Tracks that no
    /// longer exist are dropped; unparseable files leave the playlist alone.
    pub fn load_playlist_from(&mut self, path: PathBuf) {
        let Some(paths) = library::read_playlist(&path) else { return };
        self.playlist = paths
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        self.stop();
        self.ensure_tags(self.playlist.clone());
        self.playlist_file = Some(path);
        self.playlist_dirty = false;
        self.save_config();
    }

    /// Clear the playlist for a fresh build (confirm dialog lives in the GUI).
    /// Unsets the tracked file — a new playlist must ask where it's saved.
    pub fn new_playlist(&mut self) {
        self.stop();
        self.playlist.clear();
        self.playlist_file = None;
        self.playlist_dirty = false;
        self.save_config();
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

    /// List the given directory and start tagging its audio files in the
    /// background. Persists the last browsed dir on the way.
    pub fn navigate_to(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            return;
        }
        self.library_dir = dir;
        let (dirs, files) = library::list_dir(&self.library_dir, self.show_hidden);
        self.library_entries = dirs
            .into_iter()
            .map(|p| library::Entry { path: p, is_dir: true })
            .chain(files.into_iter().map(|p| library::Entry { path: p, is_dir: false }))
            .collect();
        self.apply_library_sort();
        self.ensure_tags(
            self.library_entries
                .iter()
                .filter(|e| !e.is_dir() && !library::is_playlist(e.path()))
                .map(|e| e.path().to_path_buf())
                .collect(),
        );
        self.save_config();
    }

    /// Sort the browsed folder's rows by the active header sort. Missing
    /// tags sort last (empty artist/album/year/genre, missing duration);
    /// folders are untagged entries, so the tag columns sink them below the
    /// files.
    fn apply_library_sort(&mut self) {
        let key = self.library_sort;
        let asc = self.library_asc;
        let cache = &self.tag_cache;
        self.library_entries.sort_by_cached_key(|e| {
            let info = cache.get(e.path());
            library::sort_key(e, info, key)
        });
        if !asc {
            self.library_entries.reverse();
        }
    }

    /// Header click: pick a new column (ascending) or flip the active one and
    /// re-sort the current folder in place.
    pub fn set_library_sort(&mut self, key: usize) {
        if key >= library::SORT_OPTIONS.len() {
            return;
        }
        if self.library_sort == key {
            self.library_asc = !self.library_asc;
        } else {
            self.library_sort = key;
            self.library_asc = true;
        }
        self.apply_library_sort();
    }

    /// Ensure the given audio files have tag info in the cache: any path
    /// missing from `tag_cache` goes to a background `scan_files` thread.
    /// One scan runs at a time — a new request replaces the in-flight one
    /// (the cache is per-path, so a dropped scan simply restarts the next
    /// time its paths are requested; nothing is corrupted).
    pub fn ensure_tags(&mut self, paths: Vec<PathBuf>) {
        let missing: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| !self.tag_cache.contains_key(p))
            .collect();
        if missing.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || library::scan_files(missing, tx));
        self.tag_scan_rx = Some(rx);
        self.ctx.request_repaint();
    }

    /// Drain the background tag scan into the cache (called every frame).
    /// Drops the receiver once the thread finishes so "Scanning…" clears.
    pub fn drain_tag_scan(&mut self) {
        let mut ended = false;
        let mut new = false;
        if let Some(rx) = &self.tag_scan_rx {
            loop {
                match rx.try_recv() {
                    Ok((path, info)) => {
                        self.tag_cache.insert(path, info);
                        new = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        ended = true;
                        break;
                    }
                }
            }
        }
        if ended {
            self.tag_scan_rx = None;
        }
        if new || ended {
            self.ctx.request_repaint();
        }
    }

    /// Play a library file directly. `current_index = None` is the "direct
    /// open" semantics: the playlist's sequential flow isn't touched and
    /// auto-advance won't cascade off it.
    pub fn play_file(&mut self, path: PathBuf) {
        self.current_index = None;
        self.load_file(path);
    }

    /// Common playback start: set current_index and load the track.
    fn start(&mut self, idx: usize) {
        self.current_index = Some(idx);
        let path = self.playlist[idx].clone();
        self.load_file(path);
    }

    pub fn toggle_favorite(&mut self, dir: PathBuf) {
        if let Some(i) = self.favorite_dirs.iter().position(|d| d == &dir) {
            self.favorite_dirs.remove(i);
        } else {
            self.favorite_dirs.push(dir);
        }
        self.save_config();
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

    /// Visualizer pane view — see `VizView::ALL`.
    pub fn viz_view(&self) -> VizView { self.viz_view }

    /// Set the visualizer view; persists to config.json immediately.
    pub fn set_viz_view(&mut self, view: VizView) {
        if self.viz_view == view {
            return;
        }
        self.viz_view = view;
        self.save_config();
    }

    /// Current balance (-1.0..=1.0).
    pub fn balance(&self) -> f32 {
        *self.balance.read().unwrap()
    }

    /// Set balance; live, no sink rebuild. Clamped to [-1, 1].
    pub fn set_balance(&mut self, v: f32) {
        let clamped = v.clamp(-1.0, 1.0);
        *self.balance.write().unwrap() = clamped;
        self.save_config();
    }

    /// Whether to show remaining time instead of elapsed.
    pub fn remaining(&self) -> bool { self.remaining }

    /// Toggle remaining/elapsed mode; persists immediately.
    pub fn set_remaining(&mut self, remaining: bool) {
        if self.remaining == remaining {
            return;
        }
        self.remaining = remaining;
        self.save_config();
    }

    /// Whether gapless playback is enabled.
    pub fn gapless(&self) -> bool { self.gapless }

    /// Toggle gapless playback; persists immediately.
    pub fn toggle_gapless(&mut self) {
        self.gapless = !self.gapless;
        self.save_config();
    }

    /// Whether crossfade playback is enabled.
    pub fn crossfade(&self) -> bool { self.crossfade }

    /// Toggle crossfade playback; persists immediately.
    pub fn toggle_crossfade(&mut self) {
        self.crossfade = !self.crossfade;
        self.save_config();
    }

    /// Crossfade duration in seconds.
    pub fn crossfade_secs(&self) -> f32 { self.crossfade_secs }

    /// Set crossfade duration; persists immediately.
    pub fn set_crossfade_secs(&mut self, secs: f32) {
        let clamped = secs.clamp(0.0, 10.0);
        if (self.crossfade_secs - clamped).abs() < f32::EPSILON {
            return;
        }
        self.crossfade_secs = clamped;
        self.save_config();
    }

    pub fn library_dir(&self) -> &std::path::Path { &self.library_dir }
    pub fn library_entries(&self) -> &[library::Entry] { &self.library_entries }
    /// Tags/duration for any previously scanned or played track — the shared
    /// cache behind the Library, Playlist, and Now Playing panes.
    pub fn track_info(&self, path: &std::path::Path) -> Option<&library::TrackInfo> {
        self.tag_cache.get(path)
    }
    /// Whether any file in the browsed folder is still missing from the tag
    /// cache (i.e. its scan is pending or underway).
    pub fn library_scanning(&self) -> bool {
        self.library_entries
            .iter()
            .any(|e| !e.is_dir() && !library::is_playlist(e.path()) && !self.tag_cache.contains_key(e.path()))
    }
    pub fn favorite_dirs(&self) -> &[PathBuf] { &self.favorite_dirs }
    pub fn is_favorite(&self, dir: &std::path::Path) -> bool {
        self.favorite_dirs.iter().any(|d| d == dir)
    }

    /// Fixed user-folder shortcuts (Home + XDG user dirs) shown above the
    /// Favorites list in the Library pane. Missing dirs are skipped; dupes
    /// (e.g. Music == Home) are dropped.
    pub fn quick_folders(&self) -> Vec<(String, PathBuf)> {
        let mut v: Vec<(String, PathBuf)> = Vec::new();
        if let Some(h) = dirs::home_dir() {
            v.push(("Home".into(), h));
        }
        for (label, d) in [
            ("Music", dirs::audio_dir()),
            ("Downloads", dirs::download_dir()),
            ("Desktop", dirs::desktop_dir()),
        ] {
            if let Some(p) = d.filter(|p| p.is_dir()) {
                if !v.iter().any(|(_, e)| e == &p) {
                    v.push((label.into(), p));
                }
            }
        }
        v
    }

    pub fn show_hidden(&self) -> bool { self.show_hidden }

    pub fn library_sort(&self) -> usize { self.library_sort }
    pub fn library_sort_asc(&self) -> bool { self.library_asc }

    /// Toggle hidden-folder display in the Library and re-list the current
    /// dir so the change lands immediately (also persists it).
    pub fn set_show_hidden(&mut self, show: bool) {
        if self.show_hidden == show {
            return;
        }
        self.show_hidden = show;
        self.navigate_to(self.library_dir.clone());
    }

    pub fn has_next_track(&self) -> bool {
        if self.playlist.is_empty() {
            return false;
        }
        if self.playlist.len() == 1 {
            return self.repeat;
        }
        if !self.shuffle {
            return match (self.repeat, self.current_index) {
                (false, Some(i)) => i + 1 < self.playlist.len(),
                (true, _) => true,
                _ => false,
            };
        }
        let unplayed = (0..self.playlist.len()).filter(|i| !self.played.contains(i)).count();
        unplayed > 0 || self.repeat
    }

    pub fn has_prev_track(&self) -> bool {
        if self.playlist.is_empty() {
            return false;
        }
        if self.playlist.len() == 1 {
            return self.repeat;
        }
        if !self.shuffle {
            return match (self.repeat, self.current_index) {
                (false, Some(i)) => i > 0,
                (true, _) => true,
                _ => false,
            };
        }
        self.played.len() > 1 || self.repeat
    }

    pub fn playback_position(&self) -> f32 {
        // During crossfade/gapless, the xf_sink plays the incoming track.
        // Its position starts at 0 and progresses normally.
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
        if !self.sink.empty() && !self.sink.is_paused() {
            ctx.request_repaint();
        }
    }
}