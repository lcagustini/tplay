//! GUI — app state and UI. Each logical row is a pane method; adding a pane
//! later means adding one method plus its call in `update`.

use crate::audio;
use eframe::egui;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
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
}

impl TPlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (_stream, stream_handle) =
            OutputStream::try_default().expect("No audio output device found");
        let sink = Sink::try_new(&stream_handle).expect("Failed to create audio sink");
        Self {
            _stream, stream_handle, sink,
            current_path: None,
            track_title: String::from("No track loaded"),
            total_duration: None,
            volume: 1.0,
            seek_normalized: 0.0,
            seek_target: None,
            seektable_ready: None,
        }
    }

    fn open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio", &["mp3", "wav", "ogg", "flac", "m4a"])
            .pick_file()
        {
            self.load_file(path);
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
            if ui.button("📂").clicked() { self.open_file(); }
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
            if ui.button("⏮").clicked() {
                let _ = self.sink.try_seek(Duration::ZERO);
                self.seek_normalized = 0.0;
                self.seek_target = None;
            }

            let is_paused = self.sink.is_paused();
            let is_empty  = self.sink.empty();
            if is_paused || is_empty {
                if ui.button("▶").clicked() {
                    if is_paused && !is_empty {
                        self.sink.play();
                    } else if let Some(path) = self.current_path.clone() {
                        self.load_file(path);
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

            ui.add_enabled(false, egui::Button::new("⏭"));

            ui.label("🔊");
            if ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false)).changed() {
                self.sink.set_volume(self.volume);
            }
        });
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
        });

        if !self.sink.empty() && !self.sink.is_paused() {
            ctx.request_repaint();
        }
    }
}
