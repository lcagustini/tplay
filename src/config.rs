//! The app's one settings file — `~/.config/tplay/config.json`.
//!
//! This module owns the on-disk shape and nothing else: the structs, their
//! serde defaults, and the read/write. It has no idea what a "theme" or a
//! "shuffle" *does* — `TPlayApp` builds a `Config` from its own state and
//! hands it here. That one-way dependency is the point: the fields are
//! `pub` because they document the format the tests pin, not because the app
//! reads them back through this module.

use crate::network;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const FILE: &str = "config.json";

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
    /// Output buffer size in frames — larger = more underrun slack, more latency.
    #[serde(default = "default_buffer_size")]
    pub buffer_size: u32,
    /// Ceiling in megabytes for the SMB spool cache. Tagging a share downloads
    /// whole files, so without a bound the cache (which has no other eviction)
    /// would grow without limit. Only files playback has never used are
    /// evicted; a played track is never a candidate.
    #[serde(default = "default_spool_cache_mb")]
    pub spool_cache_mb: u32,
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
    /// Saved SMB servers (host + username; passwords are session-memory only).
    #[serde(default)]
    pub servers: Vec<network::ServerCfg>,
}

fn default_volume() -> f32 { 1.0 }
fn default_crossfade_secs() -> f32 { 3.0 }
fn default_buffer_size() -> u32 { 8192 }
/// 2 GiB of never-played spool. Sized for a laptop with room to spare; raise
/// `spool_cache_mb` in config.json to keep more of a browsed share on disk.
fn default_spool_cache_mb() -> u32 { 2048 }

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

// ── The live settings ───────────────────────────────────────────────────────

/// Longest crossfade the UI offers, in seconds (`config.json` values above it
/// are clamped on load rather than honoured).
pub const MAX_CROSSFADE_SECS: f32 = 10.0;

/// The user-tunable playback settings, as one owned group.
///
/// This is the live counterpart of the matching `Config` fields: `from_config`
/// reads them at startup, `write_into` fills them at save time. It exists
/// because `TPlayApp` had **12** near-identical setters — six of them the same
/// "if unchanged, return; set; mark dirty" shape — and the clamping lived in
/// each one separately. Setters here clamp and report whether anything actually
/// changed, so a caller can skip the dirty-marking entirely when nothing moved.
pub struct Prefs {
    pub viz_view: VizView,
    pub remaining: bool,
    pub gapless: bool,
    pub crossfade: bool,
    pub crossfade_secs: f32,
    /// Balance (L/R). An `Arc` because `BalanceSource` reads it on every audio
    /// frame from the audio thread while the GUI writes it — the same live-shared
    /// shape as `EqShared`. Not a plain `f32` for exactly that reason.
    balance: Arc<RwLock<f32>>,
}

impl Prefs {
    pub fn from_config(c: &Config) -> Self {
        Self {
            viz_view: c.viz_view,
            remaining: c.remaining,
            gapless: c.gapless,
            crossfade: c.crossfade,
            // Clamp on load: a hand-edited config.json must not be able to put
            // a 900-second crossfade (or a negative one) into the fade math.
            crossfade_secs: c.crossfade_secs.clamp(0.0, MAX_CROSSFADE_SECS),
            balance: Arc::new(RwLock::new(c.balance)),
        }
    }

    pub fn viz_view(&self) -> VizView { self.viz_view }

    pub fn set_viz_view(&mut self, v: VizView) -> bool {
        if self.viz_view == v { return false; }
        self.viz_view = v;
        true
    }

    pub fn remaining(&self) -> bool { self.remaining }

    pub fn set_remaining(&mut self, v: bool) -> bool {
        if self.remaining == v { return false; }
        self.remaining = v;
        true
    }

    pub fn gapless(&self) -> bool { self.gapless }

    /// Always changes, so it reports nothing — there is no "unchanged" case to
    /// skip a dirty-mark for.
    pub fn toggle_gapless(&mut self) { self.gapless = !self.gapless; }

    pub fn crossfade(&self) -> bool { self.crossfade }

    pub fn toggle_crossfade(&mut self) { self.crossfade = !self.crossfade; }

    pub fn crossfade_secs(&self) -> f32 { self.crossfade_secs }

    pub fn set_crossfade_secs(&mut self, secs: f32) -> bool {
        let clamped = secs.clamp(0.0, MAX_CROSSFADE_SECS);
        if (self.crossfade_secs - clamped).abs() < f32::EPSILON { return false; }
        self.crossfade_secs = clamped;
        true
    }

    pub fn balance(&self) -> f32 { *self.balance.read().unwrap() }

    pub fn set_balance(&mut self, v: f32) -> bool {
        let clamped = v.clamp(-1.0, 1.0);
        *self.balance.write().unwrap() = clamped;
        true
    }

    /// The handle `BalanceSource` holds. Cloned, not shared by reference — the
    /// source outlives any borrow of `self`.
    pub fn balance_shared(&self) -> Arc<RwLock<f32>> { Arc::clone(&self.balance) }
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(FILE))
}

/// Read config.json. A missing, unreadable or malformed file is a fresh
/// install, not an error — the app starts on defaults.
pub fn load() -> Config {
    path().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(config: &Config) {
    let Some(path) = path() else { return };
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(config) { let _ = std::fs::write(&path, json); }
}

// ── When to write ───────────────────────────────────────────────────────────

/// How long to let changes settle before writing config.json.
pub const CONFIG_SAVE_DEBOUNCE_SECS: f64 = 0.5;

/// Whether config.json should be written right now.
///
/// The debounce exists because every settings setter used to write
/// immediately, and the widgets that call them — the EQ band's 10 vertical
/// sliders, volume, balance — fire `resp.changed()` on **every frame of a
/// drag**. So dragging one band serialized and `fs::write`d the whole file
/// ~60 times a second, synchronously, on the UI thread.
///
/// Pure and time-injected so the policy is testable without an eframe
/// `Context`: `now` and `last_save` are both seconds on the same clock
/// (`ctx.input(|i| i.time)`), and `closing` is `close_requested()`.
///
/// The `closing` branch is the counterpart to the debounce — it costs at most
/// `debounce` seconds of settings on a hard kill, which the close flush buys
/// back. This is the same shape `gui/coordinator.rs` uses for dock_layout.json.
pub fn should_flush(dirty: bool, now: f64, last_save: f64, closing: bool) -> bool {
    if !dirty {
        return false;
    }
    if closing {
        return true;
    }
    now - last_save >= CONFIG_SAVE_DEBOUNCE_SECS
}
