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
