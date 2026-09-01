#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// ── Duration probe ─────────────────────────────────────────────────────────────

fn probe_duration(path: &std::path::Path) -> Option<Duration> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let track = probed.format.default_track()?;
    let tb       = track.codec_params.time_base?;
    let n_frames = track.codec_params.n_frames?;
    let t = tb.calc_time(n_frames);
    Some(Duration::from_secs_f64(t.seconds as f64 + t.frac))
}

// ── FLAC seektable builder ─────────────────────────────────────────────────────

/// Returns true if the FLAC file already has a non-empty SEEKTABLE block.
/// Reads only the metadata section — a handful of bytes at most.
fn flac_has_seektable(path: &std::path::Path) -> bool {
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return false };
    let mut b4 = [0u8; 4];
    if f.read_exact(&mut b4).is_err() || &b4 != b"fLaC" { return false; }
    loop {
        if f.read_exact(&mut b4).is_err() { return false; }
        let is_last = b4[0] & 0x80 != 0;
        let btype   = b4[0] & 0x7F;
        let blen    = u32::from_be_bytes([0, b4[1], b4[2], b4[3]]) as i64;
        if btype == 3 && blen > 0 { return true; }
        if is_last { return false; }
        if f.seek(SeekFrom::Current(blen)).is_err() { return false; }
    }
}

/// Scans every audio packet (frame-demux only, no decoding) to collect accurate
/// byte offsets, then writes a SEEKTABLE block into the file's PADDING block.
///
/// This is the accurate version that walks packet headers — takes ~1 s on a
/// large FLAC but is called from a background thread so it never blocks the UI.
/// On subsequent loads `flac_has_seektable` returns true and this is skipped.
fn build_flac_seektable(path: &std::path::Path) -> io::Result<()> {
    if !path.extension().and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flac")).unwrap_or(false)
    {
        return Ok(());
    }
    if flac_has_seektable(path) { return Ok(()); }

    // ── Phase 1: read STREAMINFO and find PADDING block ────────────────────
    let (sample_rate, pad_hdr_pos, pad_data_len, pad_is_last) = {
        let mut f = File::open(path)?;
        let mut b4 = [0u8; 4];
        f.read_exact(&mut b4)?;
        if &b4 != b"fLaC" { return Ok(()); }

        let mut sample_rate = 0u32;
        let mut pad: Option<(u64, u64, bool)> = None;

        loop {
            let hdr_pos = f.stream_position()?;
            f.read_exact(&mut b4)?;
            let is_last = b4[0] & 0x80 != 0;
            let btype   = b4[0] & 0x7F;
            let blen    = u32::from_be_bytes([0, b4[1], b4[2], b4[3]]) as u64;
            match btype {
                0 => {
                    let mut si = vec![0u8; blen as usize];
                    f.read_exact(&mut si)?;
                    if si.len() >= 13 {
                        sample_rate = ((si[10] as u32) << 12)
                            | ((si[11] as u32) << 4)
                            | ((si[12] as u32) >> 4);
                    }
                }
                1 if blen >= 18 && pad.is_none() => {
                    pad = Some((hdr_pos, blen, is_last));
                    f.seek(SeekFrom::Current(blen as i64))?;
                }
                3 => return Ok(()), // seektable already present
                _ => { f.seek(SeekFrom::Current(blen as i64))?; }
            }
            if is_last { break; }
        }

        match pad {
            Some((pos, len, last)) if sample_rate > 0 => (sample_rate, pos, len, last),
            _ => return Ok(()),
        }
    };

    // ── Phase 2: walk audio packets to collect accurate byte offsets ────────
    //
    // symphonia's packet demuxer reads just the frame headers (a few bytes each)
    // without decoding any audio. We accumulate the encoded size of each packet
    // to track the exact byte offset of each frame within the audio stream.
    let file = File::open(path)?;
    let mss  = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let mut fmt = probed.format;

    let n_slots = (pad_data_len / 18) as usize;
    if n_slots == 0 { return Ok(()); }

    let interval  = sample_rate as u64; // one seekpoint per second
    let mut points: Vec<(u64, u64, u16)> = Vec::new(); // (sample, byte_off, frame_samples)
    let mut byte_off: u64 = 0;
    let mut next_target: u64 = 0;

    loop {
        match fmt.next_packet() {
            Ok(pk) => {
                if pk.ts >= next_target && points.len() < n_slots {
                    points.push((pk.ts, byte_off, pk.dur.min(u16::MAX as u64) as u16));
                    next_target = (pk.ts / interval + 1) * interval;
                }
                byte_off += pk.data.len() as u64;
            }
            Err(_) => break,
        }
    }
    if points.is_empty() { return Ok(()); }

    // ── Phase 3: write SEEKTABLE over the PADDING block ────────────────────
    //
    // SEEKTABLE entries are 18 bytes. The PADDING may not divide evenly by 18;
    // any leftover ≥ 4 bytes becomes a small trailing PADDING block.
    // If the leftover is 1-3 bytes (too small for a block header) we sacrifice
    // one slot and combine those bytes into a valid trailing PADDING block.
    let remainder = pad_data_len % 18;
    let used_slots: usize = if remainder > 0 && remainder < 4 {
        ((pad_data_len / 18).saturating_sub(1)) as usize
    } else {
        (pad_data_len / 18) as usize
    };
    let leftover: u64 = if remainder == 0 { 0 }
        else if remainder >= 4 { remainder }
        else { remainder + 18 };

    let seektable_data_len = (used_slots as u32) * 18;
    let actual_points = points.len().min(used_slots);

    let mut wf = std::fs::OpenOptions::new().write(true).open(path)?;
    wf.seek(SeekFrom::Start(pad_hdr_pos))?;

    // SEEKTABLE block header (type = 3)
    let st_last: u8 = if leftover == 0 && pad_is_last { 0x80 } else { 0 };
    wf.write_all(&[
        st_last | 3,
        (seektable_data_len >> 16) as u8,
        (seektable_data_len >>  8) as u8,
         seektable_data_len        as u8,
    ])?;

    for &(sn, so, fs) in &points[..actual_points] {
        let mut e = [0u8; 18];
        e[ 0.. 8].copy_from_slice(&sn.to_be_bytes());
        e[ 8..16].copy_from_slice(&so.to_be_bytes());
        e[16..18].copy_from_slice(&fs.to_be_bytes());
        wf.write_all(&e)?;
    }
    // Fill remaining slots with placeholder entries
    let mut ph = [0u8; 18];
    ph[..8].fill(0xFF);
    for _ in actual_points..used_slots {
        wf.write_all(&ph)?;
    }

    // Trailing PADDING block for any leftover bytes
    if leftover >= 4 {
        let pd_data = leftover - 4;
        let pd_last: u8 = if pad_is_last { 0x80 } else { 0 };
        wf.write_all(&[
            pd_last | 1,
            (pd_data >> 16) as u8,
            (pd_data >>  8) as u8,
             pd_data        as u8,
        ])?;
        wf.write_all(&vec![0u8; pd_data as usize])?;
    }

    Ok(())
}

// ── Main ───────────────────────────────────────────────────────────────────────

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPlay")
            .with_inner_size([320.0, 130.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "TPlay",
        native_options,
        Box::new(|cc| Ok(Box::new(TPlayApp::new(cc)))),
    )
}

// ── App state ──────────────────────────────────────────────────────────────────

struct TPlayApp {
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
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

        if is_flac && !flac_has_seektable(&path) {
            let ready = Arc::new(AtomicBool::new(false));
            self.seektable_ready = Some(Arc::clone(&ready));
            let path_bg = path.clone();
            std::thread::spawn(move || {
                let _ = build_flac_seektable(&path_bg);
                ready.store(true, Ordering::Release);
            });
        }

        match File::open(&path) {
            Ok(file) => match Decoder::new(BufReader::new(file)) {
                Ok(source) => {
                    self.total_duration = source.total_duration()
                        .or_else(|| probe_duration(&path));
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
}

// ── UI ────────────────────────────────────────────────────────────────────────

impl eframe::App for TPlayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📂").clicked() { self.open_file(); }
                ui.label(&self.track_title);
            });

            ui.separator();

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

            ui.separator();

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
        });

        if !self.sink.empty() && !self.sink.is_paused() {
            ctx.request_repaint();
        }
    }
}
