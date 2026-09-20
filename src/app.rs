//! App state and logic — no UI code here.

use crate::audio;
use crate::gui::theme::{self, Theme, Themes};
use crate::library;
use eframe::egui;
use fastrand;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Docking panes (egui_dock) - used by GUI layer only
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane { NowPlaying, Playlist, Equalizer, Library }

impl Pane {
    pub const ALL: [Pane; 4] = [Pane::NowPlaying, Pane::Playlist, Pane::Equalizer, Pane::Library];
}

/// Equalizer presets — index 0 is Flat (the reset). A manually tweaked slider
/// switches the selection to `EQ_PRESET_CUSTOM`.
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
/// Sentinel index meaning "gains were hand-edited, not a named preset".
pub const EQ_PRESET_CUSTOM: usize = EQ_PRESETS.len();

pub struct TPlayApp {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Sink,

    current_path: Option<PathBuf>,
    total_duration: Option<Duration>,

    volume: f32,

    /// Holds the slider at the intended position until get_pos() catches up,
    /// preventing snap-back to 0 during a skip_duration seek.
    seek_target: Option<f32>,

    /// Set to Some(flag) while the background seektable builder is running.
    /// The flag flips to true when the thread finishes. None means the file
    /// either already had a seektable or is not a FLAC.
    seektable_ready: Option<Arc<AtomicBool>>,

    playlist: Vec<PathBuf>,
    /// Index into `playlist` of the track currently loaded, set when playback
    /// starts from the pane (row click, auto-advance). None = direct open,
    /// which breaks the sequential flow.
    current_index: Option<usize>,

    /// Shuffle mode: play tracks in random order
    shuffle: bool,
    /// Repeat mode: loop playlist (or random if shuffle also on)
    repeat: bool,

    /// Shuffled playlist indices. Regenerated when playlist changes or shuffle toggled.
    shuffle_order: Vec<usize>,
    /// Current position in shuffle_order.
    shuffle_pos: usize,

    /// Equalizer state
    eq_enabled: bool,
    /// 10-band gains in dB (-12 to +12), default 0
    eq_gains: [f32; 10],
    /// Active EQ settings shared with the running EqSource (live, no rebuild)
    eq_shared: Arc<Mutex<audio::eq::EqShared>>,
    /// Index into `EQ_PRESETS`, or `EQ_PRESET_CUSTOM` after a manual tweak.
    eq_preset: usize,


    /// Theme (loaded from themes/ dirs), applied to egui visuals by the GUI layer.
    theme: Arc<Theme>,
    /// All loadable themes (Skin dropdown + icon fallback).
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
    /// off; toggled from the ☰ menu, persisted in library.json.
    show_hidden: bool,
    /// Library list sort: index into `library::SORT_OPTIONS` (0 = Title, the
    /// default), plus direction. Set by header clicks.
    library_sort: usize,
    library_asc: bool,
}

impl TPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let (_stream, stream_handle) =
            OutputStream::try_default().expect("No audio output device found");
        let sink = Sink::try_new(&stream_handle).expect("Failed to create audio sink");

        let themes = Themes::load();
        let theme = themes
            .get(&theme::selection())
            .cloned()
            .unwrap_or_else(|| themes.default().clone());
        let icons = theme::load_icons(&ctx, &themes, &theme);

        let mut app = Self {
            _stream,
            stream_handle,
            sink,
            current_path: None,
            total_duration: None,
            volume: 1.0,
            seek_target: None,
            seektable_ready: None,
            playlist: Vec::new(),
            current_index: None,
            shuffle: false,
            repeat: false,
            shuffle_order: Vec::new(),
            shuffle_pos: 0,
            eq_enabled: false,
            eq_gains: [0.0; 10],
            eq_shared: Arc::new(Mutex::new(audio::eq::EqShared {
                gains: [0.0; 10],
                enabled: false,
            })),
            eq_preset: 0,
            theme,
            themes,
            icons,
            ctx,
            library_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            library_entries: Vec::new(),
            tag_cache: HashMap::new(),
            tag_scan_rx: None,
            favorite_dirs: Vec::new(),
            show_hidden: false,
            library_sort: 0,
            library_asc: true,
        };
        app.load_eq();
        app.sync_eq_shared();
        app.load_library();
        app
    }

    pub(crate) fn audio_dialog() -> rfd::FileDialog {
        rfd::FileDialog::new().add_filter("Audio", &["mp3", "wav", "ogg", "flac", "m4a"])
    }

    fn playlist_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("tplay").join("playlist.json"))
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
            let new_len = self.playlist.len();
            let old_len = new_len - added_count;
            let mut new_indices: Vec<usize> = (old_len..new_len).collect();
            for i in (1..new_indices.len()).rev() {
                let j = fastrand::usize(..=i);
                new_indices.swap(i, j);
            }
            let insert_at = self.shuffle_pos.min(self.shuffle_order.len());
            self.shuffle_order.splice(insert_at..insert_at, new_indices);
            // Tag the new tracks in the background (dedup handles repeats).
            self.ensure_tags(self.playlist.clone());
        }
    }

    fn load_file(&mut self, path: PathBuf) {
        self.seek_target     = None;
        self.seektable_ready = None;

        match Sink::try_new(&self.stream_handle) {
            Ok(s) => { self.sink = s; self.sink.set_volume(self.volume); }
            Err(e) => { eprintln!("tplay: sink error: {e}"); return; }
        }

        let is_flac = path.extension().and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("flac")).unwrap_or(false);

        if is_flac && !audio::flac_has_seektable(&path) {
            let ready = Arc::new(AtomicBool::new(false));
            self.seektable_ready = Some(Arc::clone(&ready));
            let path_bg = path.clone();
            std::thread::spawn(move || {
                let _ = audio::build_flac_seektable(&path_bg);
                ready.store(true, Ordering::Release);
            });
        }

        // Tag the loaded track up front so Now Playing shows title · artist
        // immediately instead of waiting on a scan (one file, negligible cost).
        if let Some(info) = library::read_info(&path) {
            self.tag_cache.insert(path.clone(), info);
        }

        match File::open(&path) {
            Ok(file) => match Decoder::new(BufReader::new(file)) {
                Ok(source) => {
                    self.total_duration = source.total_duration()
                        .or_else(|| audio::probe_duration(&path));
                    let source = source.convert_samples::<f32>();
                    let eq_source = audio::eq::EqSource::new(source, Arc::clone(&self.eq_shared));
                    self.sink.append(eq_source);
                    self.current_path = Some(path);
                }
                Err(e) => {
                    eprintln!("tplay: decode error: {e}");
                    self.current_path = None;
                    self.total_duration = None;
                }
            },
            Err(e) => {
                eprintln!("tplay: open error: {e}");
                self.current_path = None;
                self.total_duration = None;
            }
        }

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

        self.ensure_shuffle_order();

        if self.shuffle_pos >= self.shuffle_order.len() {
            if self.repeat {
                self.regenerate_shuffle_order();
                self.shuffle_pos = 0;
            } else {
                return None;
            }
        }

        let next = self.shuffle_order.get(self.shuffle_pos).copied();
        self.shuffle_pos += 1;
        next
    }

    fn ensure_shuffle_order(&mut self) {
        if self.shuffle_order.len() != self.playlist.len() {
            self.regenerate_shuffle_order();
            self.shuffle_pos = 0;
        }
    }

    fn regenerate_shuffle_order(&mut self) {
        let len = self.playlist.len();
        self.shuffle_order = (0..len).collect();
        for i in (1..len).rev() {
            let j = fastrand::usize(..=i);
            self.shuffle_order.swap(i, j);
        }
    }

    fn reset_shuffle(&mut self) {
        self.shuffle_order.clear();
        self.shuffle_pos = 0;
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

        if self.shuffle_pos > 1 {
            self.shuffle_pos -= 1;
            Some(self.shuffle_order[self.shuffle_pos - 1])
        } else if self.repeat && !self.shuffle_order.is_empty() {
            self.shuffle_pos = len;
            Some(self.shuffle_order[len - 1])
        } else {
            None
        }
    }

    pub fn seek(&mut self, progress: f32) {
        self.seek_target = Some(progress);

        let path = match self.current_path.clone() { Some(p) => p, None => return };
        let total_secs = match self.total_duration { Some(d) => d.as_secs_f32(), None => return };
        let target = Duration::from_secs_f32((progress * total_secs).max(0.0));

        let table_ready = self.seektable_ready
            .as_ref()
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(true);

        if table_ready && self.sink.try_seek(target).is_ok() {
            return;
        }

        let was_paused = self.sink.is_paused();

        let file   = match File::open(&path)             { Ok(f) => f, Err(e) => { eprintln!("seek open: {e}");   return; } };
        let source = match Decoder::new(BufReader::new(file)) { Ok(s) => s, Err(e) => { eprintln!("seek decode: {e}"); return; } };
        let source = source.convert_samples::<f32>();

        match Sink::try_new(&self.stream_handle) {
            Ok(s) => self.sink = s,
            Err(e) => { eprintln!("seek sink: {e}"); return; }
        }
        self.sink.set_volume(self.volume);
        let eq_source = audio::eq::EqSource::new(source.skip_duration(target), Arc::clone(&self.eq_shared));
        self.sink.append(eq_source);
        if was_paused { self.sink.pause(); }
    }

    pub fn advance(&mut self) {
        if !self.sink.empty() || self.sink.is_paused() || self.current_path.is_none() {
            return;
        }
        if let Some(next) = self.next_track_index() {
            let path = self.playlist[next].clone();
            self.current_index = Some(next);
            self.load_file(path);
        }
    }

    pub fn fmt_duration(d: Duration) -> String {
        let s = d.as_secs();
        format!("{:02}:{:02}", s / 60, s % 60)
    }

    /// Public actions called by GUI layer

    pub fn play(&mut self) {
        if self.sink.is_paused() && !self.sink.empty() {
            self.sink.play();
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
            self.ensure_shuffle_order();
            let first_idx = self.shuffle_order[0];
            let path = self.playlist[first_idx].clone();
            self.current_index = Some(first_idx);
            self.shuffle_pos = 1;
            self.load_file(path);
        } else {
            let path = self.playlist[0].clone();
            self.current_index = Some(0);
            self.load_file(path);
        }
    }

    pub fn pause(&mut self) {
        self.sink.pause();
    }

    /// Stop playback and reset song + playlist state: unloads the current
    /// track and rewinds the play position, so the next Play starts from the
    /// top of the playlist (or shuffle order) instead of resuming.
    pub fn stop(&mut self) {
        let _ = self.sink.stop();
        self.current_path = None;
        self.current_index = None;
        self.total_duration = None;
        self.seek_target = None;
        self.seektable_ready = None;
        self.shuffle_pos = 0;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.sink.set_volume(volume);
    }

    /// Set gain for one EQ band (0-9), in dB (-12 to +12). Applies live to the
    /// running source — no sink rebuild, no audio restart. Marks the selection
    /// as Custom (no preset name applies anymore).
    pub fn set_eq_gain(&mut self, band: usize, gain_db: f32) {
        if band >= 10 { return; }
        self.eq_gains[band] = gain_db.clamp(-12.0, 12.0);
        self.eq_shared.lock().unwrap().gains[band] = self.eq_gains[band];
        self.eq_preset = EQ_PRESET_CUSTOM;
        self.save_eq();
    }

    /// Select an EQ preset by index (see `EQ_PRESETS`), or `EQ_PRESET_CUSTOM`.
    pub fn set_eq_preset(&mut self, idx: usize) {
        if idx > EQ_PRESET_CUSTOM { return; }
        self.eq_preset = idx;
        if idx < EQ_PRESETS.len() {
            self.eq_gains = EQ_PRESETS[idx].1;
            self.eq_shared.lock().unwrap().gains = self.eq_gains;
        }
        self.save_eq();
    }

    pub fn eq_preset(&self) -> usize { self.eq_preset }

    pub fn eq_preset_name(&self) -> &'static str {
        if self.eq_preset < EQ_PRESETS.len() {
            EQ_PRESETS[self.eq_preset].0
        } else {
            "Custom"
        }
    }

    /// Switch theme by id (from the Skin dropdown); persisted, applied the
    /// same frame by the GUI layer, icons re-decoded for the new palette.
    pub fn set_theme(&mut self, id: &str) {
        if let Some(theme) = self.themes.get(id) {
            self.theme = Arc::clone(theme);
            self.icons = theme::load_icons(&self.ctx, &self.themes, &self.theme);
            theme::save_selection(id);
        }
    }

    pub fn theme(&self) -> &Arc<Theme> {
        &self.theme
    }

    /// All loadable themes, for the Skin dropdown.
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
        self.eq_enabled = !self.eq_enabled;
        self.eq_shared.lock().unwrap().enabled = self.eq_enabled;
        self.save_eq();
    }

    /// Publish current EQ settings to the running source (used after loading).
    fn sync_eq_shared(&self) {
        let mut shared = self.eq_shared.lock().unwrap();
        shared.gains = self.eq_gains;
        shared.enabled = self.eq_enabled;
    }

    /// Path to EQ settings file
    fn eq_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("tplay").join("eq.json"))
    }

    /// Save EQ settings to disk
    fn save_eq(&self) {
        if let Some(path) = Self::eq_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            #[derive(Serialize)]
            struct EqData { enabled: bool, gains: [f32; 10], preset: usize }
            let data = EqData {
                enabled: self.eq_enabled,
                gains: self.eq_gains,
                preset: self.eq_preset,
            };
            if let Ok(json) = serde_json::to_string_pretty(&data) {
                let _ = std::fs::write(&path, json);
            }
        }
    }

    /// Load EQ settings from disk (old files without preset still load).
    fn load_eq(&mut self) {
        if let Some(path) = Self::eq_path() {
            if let Ok(json) = std::fs::read_to_string(&path) {
                #[derive(Deserialize)]
                struct EqData {
                    enabled: bool,
                    gains: [f32; 10],
                    preset: Option<usize>,
                }
                if let Ok(data) = serde_json::from_str::<EqData>(&json) {
                    self.eq_enabled = data.enabled;
                    self.eq_gains = data.gains;
                    self.eq_preset = data.preset.unwrap_or_else(|| {
                        EQ_PRESETS.iter()
                            .position(|(_, g)| *g == data.gains)
                            .unwrap_or(EQ_PRESET_CUSTOM)
                    });
                }
            }
        }
    }

    pub fn next_track(&mut self) {
        if let Some(next_idx) = self.next_track_index() {
            let path = self.playlist[next_idx].clone();
            self.current_index = Some(next_idx);
            self.load_file(path);
        }
    }

    pub fn prev_track(&mut self) {
        if let Some(prev_idx) = self.prev_track_index() {
            let path = self.playlist[prev_idx].clone();
            self.current_index = Some(prev_idx);
            self.load_file(path);
        }
    }

    pub fn remove_track(&mut self, index: usize) {
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
        self.regenerate_shuffle_order();
        self.shuffle_pos = 0;
    }

    pub fn move_track(&mut self, from: usize, to: usize) {
        if from != to {
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
            self.regenerate_shuffle_order();
            self.shuffle_pos = 0;
        }
    }

    pub fn play_track(&mut self, index: usize) {
        let path = self.playlist[index].clone();
        self.current_index = Some(index);
        self.reset_shuffle();
        self.load_file(path);
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.regenerate_shuffle_order();
        self.shuffle_pos = 0;
    }

    pub fn toggle_repeat(&mut self) {
        self.repeat = !self.repeat;
    }

    pub fn save_playlist(&self) {
        if let Some(path) = Self::playlist_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            #[derive(Serialize)]
            struct PlaylistData {
                paths: Vec<String>,
                shuffle: bool,
                repeat: bool,
            }
            let data = PlaylistData {
                paths: self.playlist.iter().filter_map(|p| p.to_str().map(|s| s.to_string())).collect(),
                shuffle: self.shuffle,
                repeat: self.repeat,
            };
            if let Ok(json) = serde_json::to_string_pretty(&data) {
                let _ = std::fs::write(&path, json);
            }
        }
    }

    pub fn load_playlist(&mut self) {
        if let Some(path) = Self::playlist_path() {
            if let Ok(json) = std::fs::read_to_string(&path) {
                #[derive(Deserialize)]
                struct PlaylistData {
                    paths: Vec<String>,
                    shuffle: Option<bool>,
                    repeat: Option<bool>,
                }
                if let Ok(data) = serde_json::from_str::<PlaylistData>(&json) {
                    self.playlist = data.paths.into_iter()
                        .filter_map(|s| PathBuf::from(s).canonicalize().ok())
                        .filter(|p| p.exists())
                        .collect();
                    self.shuffle = data.shuffle.unwrap_or(false);
                    self.repeat = data.repeat.unwrap_or(false);
                    self.current_index = None;
                    self.current_path = None;
                    self.regenerate_shuffle_order();
                    self.shuffle_pos = 0;
                    self.ensure_tags(self.playlist.clone());
                }
            }
        }
    }

    // ── Library ────────────────────────────────────────────────────────────

    /// Path to library settings (favorites + last browsed dir).
    fn library_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("tplay").join("library.json"))
    }

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
            .map(library::Entry::Dir)
            .chain(files.into_iter().map(library::Entry::File))
            .collect();
        self.apply_library_sort();
        self.ensure_tags(
            self.library_entries
                .iter()
                .filter(|e| !e.is_dir())
                .map(|e| e.path().to_path_buf())
                .collect(),
        );
        self.save_library();
    }

    /// Sort the browsed folder's rows by the active header sort. Missing
    /// tags sort last (empty artist/album/year/genre, missing duration);
    /// folders are untagged entries, so the tag columns sink them below the
    /// files.
    fn apply_library_sort(&mut self) {
        let key = self.library_sort;
        let asc = self.library_asc;
        let cache = &self.tag_cache;
        self.library_entries.sort_by(|a, b| {
            let ord = library::cmp_entries(a, b, key, cache.get(a.path()), cache.get(b.path()));
            if asc { ord } else { ord.reverse() }
        });
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

    pub fn toggle_favorite(&mut self, dir: PathBuf) {
        if let Some(i) = self.favorite_dirs.iter().position(|d| d == &dir) {
            self.favorite_dirs.remove(i);
        } else {
            self.favorite_dirs.push(dir);
        }
        self.save_library();
    }

    fn save_library(&self) {
        if let Some(path) = Self::library_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            #[derive(Serialize)]
            struct LibraryData {
                favorites: Vec<String>,
                last_dir: String,
                show_hidden: bool,
            }
            let data = LibraryData {
                favorites: self
                    .favorite_dirs
                    .iter()
                    .filter_map(|d| d.to_str().map(str::to_string))
                    .collect(),
                last_dir: self.library_dir.to_string_lossy().into_owned(),
                show_hidden: self.show_hidden,
            };
            if let Ok(json) = serde_json::to_string_pretty(&data) {
                let _ = std::fs::write(&path, json);
            }
        }
    }

    fn load_library(&mut self) {
        if let Some(path) = Self::library_path() {
            if let Ok(json) = std::fs::read_to_string(&path) {
                #[derive(Deserialize)]
                struct LibraryData {
                    favorites: Option<Vec<String>>,
                    last_dir: Option<String>,
                    show_hidden: Option<bool>,
                }
                if let Ok(data) = serde_json::from_str::<LibraryData>(&json) {
                    if let Some(favs) = data.favorites {
                        self.favorite_dirs = favs
                            .into_iter()
                            .map(PathBuf::from)
                            .filter(|d| d.is_dir())
                            .collect();
                    }
                    if let Some(last) = data.last_dir {
                        let p = PathBuf::from(last);
                        if p.is_dir() {
                            self.library_dir = p;
                        }
                    }
                    if let Some(show) = data.show_hidden {
                        self.show_hidden = show;
                    }
                }
            }
        }
    }

    // Read-only getters

    pub fn current_path(&self) -> Option<&std::path::Path> { self.current_path.as_deref() }
    pub fn total_duration(&self) -> Option<Duration> { self.total_duration }
    pub fn volume(&self) -> f32 { self.volume }
    pub fn playlist(&self) -> &[PathBuf] { &self.playlist }
    pub fn current_index(&self) -> Option<usize> { self.current_index }
    pub fn shuffle(&self) -> bool { self.shuffle }
    pub fn repeat(&self) -> bool { self.repeat }

    pub fn eq_enabled(&self) -> bool { self.eq_enabled }
    pub fn eq_gains(&self) -> &[f32; 10] { &self.eq_gains }

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
            .any(|e| !e.is_dir() && !self.tag_cache.contains_key(e.path()))
    }
    pub fn favorite_dirs(&self) -> &[PathBuf] { &self.favorite_dirs }
    pub fn is_favorite(&self, dir: &std::path::Path) -> bool {
        self.favorite_dirs.iter().any(|d| d == dir)
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
        self.shuffle_pos < self.shuffle_order.len() || self.repeat
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
        self.shuffle_pos > 1 || self.repeat
    }

    pub fn playback_position(&self) -> f32 {
        let pos = self.sink.get_pos();
        self.total_duration
            .map(|d| (pos.as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    }

    pub fn playback_position_secs(&self) -> Duration {
        self.sink.get_pos()
    }

    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    pub fn seek_target_reached(&self) -> Option<bool> {
        self.seek_target.map(|t| self.playback_position() >= t - 0.02)
    }

    pub fn clear_seek_target(&mut self) {
        self.seek_target = None;
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