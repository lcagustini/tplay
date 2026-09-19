//! App state and logic — no UI code here.

use crate::audio;
use eframe::egui;
use egui_dock::{DockState, NodeIndex};
use fastrand;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Docking panes (egui_dock)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane { NowPlaying, Playlist }

impl Pane {
    pub const ALL: [Pane; 2] = [Pane::NowPlaying, Pane::Playlist];
}

pub struct TPlayApp {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Sink,

    current_path: Option<PathBuf>,
    track_title: String,
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

    /// Docking state
    pub dock_state: DockState<Pane>,
    /// Which panes are currently open (docked)
    pub dock_open: HashSet<Pane>,
}

impl TPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (_stream, stream_handle) =
            OutputStream::try_default().expect("No audio output device found");
        let sink = Sink::try_new(&stream_handle).expect("Failed to create audio sink");

        let dock_state = {
            let mut d = DockState::new(vec![Pane::NowPlaying]);
            d.main_surface_mut().split_below(NodeIndex::root(), 0.25, vec![Pane::Playlist]);
            d
        };
        let dock_open = Pane::ALL.into_iter().collect();

        Self {
            _stream,
            stream_handle,
            sink,
            current_path: None,
            track_title: String::from("No track loaded"),
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
            dock_state,
            dock_open,
        }
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
        }
    }

    fn load_file(&mut self, path: PathBuf) {
        self.track_title = path
            .file_name().unwrap_or_default()
            .to_string_lossy().to_string();
        self.seek_target     = None;
        self.seektable_ready = None;

        match Sink::try_new(&self.stream_handle) {
            Ok(s) => { self.sink = s; self.sink.set_volume(self.volume); }
            Err(e) => { self.track_title = format!("Sink error: {e}"); return; }
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

        match File::open(&path) {
            Ok(file) => match Decoder::new(BufReader::new(file)) {
                Ok(source) => {
                    self.total_duration = source.total_duration()
                        .or_else(|| audio::probe_duration(&path));
                    self.sink.append(source);
                    self.current_path = Some(path);
                }
                Err(e) => {
                    self.track_title = format!("Decode error: {e}");
                    self.current_path = None;
                    self.total_duration = None;
                }
            },
            Err(e) => {
                self.track_title = format!("Open error: {e}");
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

        match Sink::try_new(&self.stream_handle) {
            Ok(s) => self.sink = s,
            Err(e) => { eprintln!("seek sink: {e}"); return; }
        }
        self.sink.set_volume(self.volume);
        self.sink.append(source.skip_duration(target));
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

    pub fn stop(&mut self) {
        let _ = self.sink.try_seek(Duration::ZERO);
        self.sink.pause();
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.sink.set_volume(volume);
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
                }
            }
        }
    }

    // Read-only getters

    pub fn track_title(&self) -> &str { &self.track_title }
    pub fn total_duration(&self) -> Option<Duration> { self.total_duration }
    pub fn volume(&self) -> f32 { self.volume }
    pub fn playlist(&self) -> &[PathBuf] { &self.playlist }
    pub fn current_index(&self) -> Option<usize> { self.current_index }
    pub fn shuffle(&self) -> bool { self.shuffle }
    pub fn repeat(&self) -> bool { self.repeat }

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
        if !self.sink.empty() && !self.sink.is_paused() {
            ctx.request_repaint();
        }
    }
}