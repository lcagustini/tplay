//! GUI — app state and UI. Each logical row is a pane method; adding a pane
//! later means adding one method plus its call in `update`.

use crate::audio;
use eframe::egui;
use fastrand;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct TPlayApp {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Sink,

    current_path: Option<PathBuf>,
    track_title: String,
    total_duration: Option<Duration>,

    volume: f32,
    seek_normalized: f32,

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

    /// Index of the item being dragged, if any.
    drag_from: Option<usize>,
    /// Index of the item being hovered as a drop target.
    drag_hover: Option<usize>,
}

impl TPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (_stream, stream_handle) =
            OutputStream::try_default().expect("No audio output device found");
        let sink = Sink::try_new(&stream_handle).expect("Failed to create audio sink");
        let app = Self {
            _stream, stream_handle, sink,
            current_path: None,
            track_title: String::from("No track loaded"),
            total_duration: None,
            volume: 1.0,
            seek_normalized: 0.0,
            seek_target: None,
            seektable_ready: None,
            playlist: Vec::new(),
            current_index: None,
            shuffle: false,
            repeat: false,
            shuffle_order: Vec::new(),
            shuffle_pos: 0,
            drag_from: None,
            drag_hover: None,
        };
        app
    }

    fn audio_dialog() -> rfd::FileDialog {
        rfd::FileDialog::new().add_filter("Audio", &["mp3", "wav", "ogg", "flac", "m4a"])
    }

    fn playlist_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("tplay").join("playlist.json"))
    }

    fn save_playlist(&self) {
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

    fn load_playlist(&mut self) {
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

    fn open_file(&mut self) {
        if let Some(path) = Self::audio_dialog().pick_file() {
            self.add_to_playlist(vec![path]);
        }
    }

    fn add_to_playlist(&mut self, paths: Vec<PathBuf>) {
        let mut added_count = 0;
        for path in paths {
            if !self.playlist.iter().any(|p| p == &path) {
                self.playlist.push(path);
                added_count += 1;
            }
        }
        if added_count > 0 {
            // Add new songs to the unplayed portion of shuffle_order
            let new_len = self.playlist.len();
            let old_len = new_len - added_count;
            // Indices of new songs are at the end: old_len .. new_len
            // Insert them randomly into the unplayed portion (after shuffle_pos)
            let mut new_indices: Vec<usize> = (old_len..new_len).collect();
            // Fisher-Yates shuffle the new indices
            for i in (1..new_indices.len()).rev() {
                let j = fastrand::usize(..=i);
                new_indices.swap(i, j);
            }
            // Insert into shuffle_order after current position
            let insert_at = self.shuffle_pos.min(self.shuffle_order.len());
            self.shuffle_order.splice(insert_at..insert_at, new_indices);
        }
    }

    fn load_file(&mut self, path: PathBuf) {
        self.track_title = path
            .file_name().unwrap_or_default()
            .to_string_lossy().to_string();
        self.seek_normalized = 0.0;
        self.seek_target     = None;
        self.seektable_ready = None;

        // Drop the old Decoder by replacing the sink — releases the file handle
        // on Windows so the background thread can write the seektable.
        match Sink::try_new(&self.stream_handle) {
            Ok(s) => { self.sink = s; self.sink.set_volume(self.volume); }
            Err(e) => { self.track_title = format!("Sink error: {e}"); return; }
        }

        // For FLAC files without a seektable, kick off a background thread to
        // build one accurately (packet scan, ~1 s). try_seek() will use it as
        // soon as it's done; skip_duration() covers the gap until then.
        // Files that already have a seektable skip this entirely.
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

    /// Returns the next track index based on shuffle/repeat settings.
    /// Returns None if no next track is available (empty playlist, or end with no repeat).
    fn next_track_index(&mut self) -> Option<usize> {
        if self.playlist.is_empty() {
            return None;
        }
        let len = self.playlist.len();
        if len == 1 {
            return self.repeat.then_some(0);
        }

        if !self.shuffle {
            // Sequential modes
            return match (self.repeat, self.current_index) {
                (false, Some(i)) if i + 1 < len => Some(i + 1),
                (true, Some(i)) => Some((i + 1) % len),
                (true, None) => Some(0),
                _ => None,
            };
        }

        // Shuffle mode: use shuffle_order
        self.ensure_shuffle_order();

        if self.shuffle_pos >= self.shuffle_order.len() {
            // Reached end of shuffle cycle
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

    /// Ensure shuffle_order is valid for current playlist.
    /// Regenerates if empty or if playlist length changed.
    fn ensure_shuffle_order(&mut self) {
        if self.shuffle_order.len() != self.playlist.len() {
            self.regenerate_shuffle_order();
            self.shuffle_pos = 0;
        }
    }

    /// Generate a new random permutation of playlist indices.
    fn regenerate_shuffle_order(&mut self) {
        let len = self.playlist.len();
        self.shuffle_order = (0..len).collect();
        for i in (1..len).rev() {
            let j = fastrand::usize(..=i);
            self.shuffle_order.swap(i, j);
        }
    }

    /// Reset shuffle state (called when user clicks a track directly).
    fn reset_shuffle(&mut self) {
        self.shuffle_order.clear();
        self.shuffle_pos = 0;
    }

    /// Returns the previous track index based on shuffle/repeat settings.
    /// Returns None if no previous track is available.
    fn prev_track_index(&mut self) -> Option<usize> {
        if self.playlist.is_empty() {
            return None;
        }
        let len = self.playlist.len();
        if len == 1 {
            return self.repeat.then_some(0);
        }

        if !self.shuffle {
            // Sequential modes
            return match (self.repeat, self.current_index) {
                (false, Some(i)) if i > 0 => Some(i - 1),
                (true, Some(i)) => Some((i + len - 1) % len),
                (true, None) => Some(len - 1),
                _ => None,
            };
        }

        // Shuffle mode: go back in shuffle_order if possible
        if self.shuffle_pos > 1 {
            self.shuffle_pos -= 1;
            Some(self.shuffle_order[self.shuffle_pos - 1])
        } else if self.repeat && !self.shuffle_order.is_empty() {
            // At start of shuffle cycle, wrap to end if repeat
            self.shuffle_pos = len;
            Some(self.shuffle_order[len - 1])
        } else {
            None
        }
    }

    /// Check if there's a next track available (for button enabling).
    fn has_next_track(&self) -> bool {
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
        // Shuffle: check if we have more in current cycle, or repeat is on
        self.shuffle_pos < self.shuffle_order.len() || self.repeat
    }

    /// Check if there's a previous track available.
    fn has_prev_track(&self) -> bool {
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
        // Shuffle: check if we can go back in current cycle, or repeat is on
        self.shuffle_pos > 1 || self.repeat
    }

    fn fmt_duration(d: Duration) -> String {
        let s = d.as_secs();
        format!("{:02}:{:02}", s / 60, s % 60)
    }

    fn seek_to(&mut self, progress: f32) {
        self.seek_target = Some(progress);

        let path = match self.current_path.clone() { Some(p) => p, None => return };
        let total_secs = match self.total_duration { Some(d) => d.as_secs_f32(), None => return };
        let target = Duration::from_secs_f32((progress * total_secs).max(0.0));

        // Fast path: use try_seek only once the seektable is confirmed written.
        // seektable_ready == None means the file already had one (or isn't FLAC).
        let table_ready = self.seektable_ready
            .as_ref()
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(true);

        if table_ready && self.sink.try_seek(target).is_ok() {
            return;
        }

        // Slow path: seektable not ready yet (or format is unseekable).
        // Reload the file and use skip_duration() to burn through samples.
        // seek_target above keeps the slider pinned while this happens.
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

    // ── Panes ───────────────────────────────────────────────────────────────

    fn title_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(&self.track_title);
        });
    }

    fn seekbar_pane(&mut self, ui: &mut egui::Ui) {
        let pos = self.sink.get_pos();
        let total_secs = self.total_duration.map(|d| d.as_secs_f32());
        let actual_ratio = total_secs
            .map(|t| (pos.as_secs_f32() / t).clamp(0.0, 1.0))
            .unwrap_or(0.0);

        let pos_str   = Self::fmt_duration(pos);
        let total_str = self.total_duration
            .map(Self::fmt_duration)
            .unwrap_or_else(|| "--:--".into());

        ui.horizontal(|ui| {
            ui.label(pos_str);

            let bar = ui.add_enabled(
                total_secs.is_some(),
                egui::Slider::new(&mut self.seek_normalized, 0.0..=1.0).show_value(false),
            );

            if bar.dragged() {
                // hold
            } else if bar.drag_stopped() || bar.clicked() {
                self.seek_to(self.seek_normalized);
            } else {
                let target_reached = self.seek_target
                    .map(|t| actual_ratio >= t - 0.02)
                    .unwrap_or(true);
                if target_reached {
                    self.seek_target = None;
                    if total_secs.is_some() {
                        self.seek_normalized = actual_ratio;
                    }
                }
            }

            ui.label(total_str);
        });
    }

    fn transport_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Previous track button (⏮) - goes to previous in playlist/shuffle order
            let prev_enabled = self.has_prev_track();
            if ui.add_enabled(prev_enabled, egui::Button::new("⏮")).clicked() {
                if let Some(prev_idx) = self.prev_track_index() {
                    let path = self.playlist[prev_idx].clone();
                    self.current_index = Some(prev_idx);
                    self.load_file(path);
                }
            }

            let is_paused = self.sink.is_paused();
            let is_empty  = self.sink.empty();
            if is_paused || is_empty {
                if ui.button("▶").clicked() {
                    if is_paused && !is_empty {
                        self.sink.play();
                    } else if let Some(path) = self.current_path.clone() {
                        self.load_file(path);
                    } else if !self.playlist.is_empty() {
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
                    } else {
                        self.open_file();
                    }
                }
            } else if ui.button("⏸").clicked() {
                self.sink.pause();
            }

            if ui.button("⏹").clicked() {
                let _ = self.sink.try_seek(Duration::ZERO);
                self.sink.pause();
                self.seek_normalized = 0.0;
                self.seek_target = None;
            }

            let next_enabled = self.has_next_track();
            if ui.add_enabled(next_enabled && !self.playlist.is_empty(), egui::Button::new("⏭")).clicked() {
                if let Some(next_idx) = self.next_track_index() {
                    let path = self.playlist[next_idx].clone();
                    self.current_index = Some(next_idx);
                    self.load_file(path);
                }
            }

            ui.label("🔊");
            if ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false)).changed() {
                self.sink.set_volume(self.volume);
            }
        });
    }

    fn playlist_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Add Files").clicked() {
                let paths = Self::audio_dialog().pick_files();
                if let Some(paths) = paths {
                    self.add_to_playlist(paths);
                }
            }

            let shuffle_changed = ui.checkbox(&mut self.shuffle, "🔀 Shuffle").changed();
            ui.checkbox(&mut self.repeat, "🔁 Repeat");
            if shuffle_changed {
                self.regenerate_shuffle_order();
                self.shuffle_pos = 0;
            }

            if ui.button("Save Playlist").clicked() {
                self.save_playlist();
            }
            if ui.button("Load Playlist").clicked() {
                self.load_playlist();
            }
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut to_delete: Option<usize> = None;

                for i in 0..self.playlist.len() {
                    let is_current = Some(i) == self.current_index;
                    let name = self.playlist[i]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy().into_owned();
                    let track_num = format!("{}.", i + 1);

                    // We need the full row rect for drag hover detection, so build the row first
                    let row_response = ui.horizontal(|ui| {
                        // Track number (not draggable, just for display)
                        ui.label(track_num);

                        // Track name (click to play, drag to reorder)
                        let label_resp = ui.add(
                            egui::Label::new(egui::RichText::new(&name).color(
                                if is_current { ui.style().visuals.strong_text_color() } else { ui.style().visuals.text_color() }
                            ))
                            .selectable(is_current)
                            .sense(egui::Sense::click_and_drag()),
                        );

                        // Drag started on the label
                        if label_resp.drag_started() {
                            self.drag_from = Some(i);
                            self.drag_hover = None;
                        }

                        // Dragging - show visual feedback
                        if label_resp.dragged() {
                            ui.painter().text(
                                label_resp.rect.center(),
                                egui::Align2::CENTER_CENTER,
                                &name,
                                egui::FontId::proportional(14.0),
                                ui.style().visuals.hyperlink_color,
                            );
                        }

                        // Visual indicator for drop target (highlight the label)
                        if self.drag_hover == Some(i) || is_current {
                            // Re-paint with highlight
                            ui.painter().rect_filled(
                                label_resp.rect.expand(4.0),
                                4.0,
                                ui.style().visuals.hyperlink_color.gamma_multiply(0.15),
                            );
                        }

                        if label_resp.clicked() {
                            let path = self.playlist[i].clone();
                            self.current_index = Some(i);
                            self.reset_shuffle();
                            self.load_file(path);
                        }

                        // Delete button
                        if ui.button("✕").clicked() {
                            to_delete = Some(i);
                        }

                        label_resp
                    }).inner;

                    // Now we have the full row rect, do hover detection
                    let row_rect = row_response.rect;
                    if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                        if self.drag_from.is_some() && self.drag_from != Some(i) {
                            if row_rect.contains(pointer_pos) {
                                self.drag_hover = Some(i);
                            }
                        }
                    }

                    // Drag stopped - handle the move
                    if row_response.drag_stopped() {
                        if let (Some(from), Some(to)) = (self.drag_from, self.drag_hover) {
                            if from != to {
                                let item = self.playlist.remove(from);
                                let insert_at = to;
                                self.playlist.insert(insert_at, item);
                                // Update current_index
                                self.current_index = self.current_index.map(|ci| {
                                    if ci == from {
                                        insert_at
                                    } else if from < ci && ci <= insert_at {
                                        ci - 1
                                    } else if insert_at <= ci && ci < from {
                                        ci + 1
                                    } else {
                                        ci
                                    }
                                });
                                // Playlist order changed, regenerate shuffle
                                self.regenerate_shuffle_order();
                                self.shuffle_pos = 0;
                            }
                        }
                        self.drag_from = None;
                        self.drag_hover = None;
                    }
                }

                // Reset drag state if drag ended outside any item
                if self.drag_from.is_some() && ui.input(|i| i.pointer.any_released()) {
                    self.drag_from = None;
                    self.drag_hover = None;
                }

                if let Some(idx) = to_delete {
                    self.playlist.remove(idx);
                    self.current_index = self.current_index.and_then(|ci| {
                        if ci == idx {
                            None
                        } else if ci > idx {
                            Some(ci - 1)
                        } else {
                            Some(ci)
                        }
                    });
                    // If we deleted the currently playing track, clear current_path
                    if self.current_index.is_none() {
                        self.current_path = None;
                    }
                    // Playlist changed, regenerate shuffle
                    self.regenerate_shuffle_order();
                    self.shuffle_pos = 0;
                }
            });
    }

    /// Auto-advance to the next playlist track once the current one ends.
    fn advance_playlist(&mut self) {
        if !self.sink.empty() || self.sink.is_paused() || self.current_path.is_none() {
            return;
        }
        if let Some(next) = self.next_track_index() {
            let path = self.playlist[next].clone();
            self.current_index = Some(next);
            self.load_file(path);
        }
    }
}

impl eframe::App for TPlayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.title_pane(ui);
            ui.separator();
            self.seekbar_pane(ui);
            ui.separator();
            self.transport_pane(ui);
            ui.separator();
            self.playlist_pane(ui);
        });

        self.advance_playlist();

        if !self.sink.empty() && !self.sink.is_paused() {
            ctx.request_repaint();
        }
    }
}
