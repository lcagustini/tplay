//! The app's one settings file — `~/.config/tplay/config.json`, and the live
//! `Prefs` that mirrors its playback fields.
//!
//! The file half is one-way: `TPlayApp` builds a `Config` from its own state and
//! hands it over, and nothing reads a setting back *through* this module. It has
//! no idea what a "theme" or a "shuffle" *does*. The fields are `pub` because
//! they document the format the tests pin. The live half — [`Prefs`], the six
//! user-tunable playback settings — is the counterpart of the matching `Config`
//! fields, and owns their clamping.
//!
//! The last section is the **write policy**, which the other persisted file
//! shares: [`should_flush`] decides *whether*, [`Persisted`] holds the two values
//! the decision reads, and [`atomic_write`] is how a write reaches disk without
//! leaving a truncated file behind.

use crate::audio::eq::EQ_BANDS;
use crate::network;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

const FILE: &str = "config.json";

/// Visualizer views (serde'd into config.json `viz_view`). The pane matches on
/// this; the app only stores/serializes it — same shape as `Pane`.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum VizView {
    #[default]
    Bars,
    Wave,
    Radial,
    Spectrogram,
    Flame,
    Chladni,
    Nebula,
    Plasma,
    /// A feedback accumulation, and the view that keeps its own previous frame
    /// rather than only the current one. So does `Spectrogram`, which is why
    /// both need an owned render target.
    Trails,
}

/// Read a `VizView` by name, falling back to the default on one this build has
/// never heard of.
///
/// A derived `Deserialize` fails the **whole** `Config` on an unknown variant,
/// and `load_from`'s only answer to a parse error is to copy the file aside and
/// hand back `Config::default()`. So a `config.json` written by a build with one
/// more view — the downgrade case — or a typo in a file the docs call
/// hand-editable would cost the user their volume, EQ gains, saved SMB servers
/// and favourites to change which chart gets drawn. The round trip accepts
/// exactly the same names `Serialize` writes, so the on-disk format is unchanged
/// and the only thing that changes is that an unrecognised one stops being fatal.
///
/// The same reasoning put `library_db::Rule` in a struct of `Option`s. This is
/// the one other place the two answers disagreed.
fn de_viz_view<'de, D: serde::Deserializer<'de>>(d: D) -> Result<VizView, D::Error> {
    String::deserialize(d).map_or(Ok(VizView::default()), |name| {
        // **The dropdown's label first, then the Rust variant name.**
        //
        // These are two spellings of one value and they are not always the same
        // string: `VizView::name()` is what the picker shows, and a view spelled
        // for people does not match the derived `Serialize`, which writes the
        // variant name. Every shipped view currently agrees, and this is what
        // keeps the next one honest. This
        // function used to try only the variant spelling, so a hand-edited config
        // naming a view the way the UI spells it read as *unrecognised* and fell
        // back to the default. That is the exact failure `de_viz_view` exists to
        // prevent, arriving through the door it was built to guard — and it was
        // invisible until the visualizer's dispatch started keying on `name()` as
        // well, which is the only reason it was ever found.
        //
        // Both spellings are accepted, so no config either way stops loading, and
        // `Serialize` is left writing the variant name, so the on-disk format does
        // not move. `every_view_name_round_trips_through_the_config` keeps the two
        // in step.
        if let Some(v) = VizView::ALL.iter().copied().find(|v| v.name() == name) {
            return Ok(v);
        }
        // `from_value` rather than `from_str`: serde has already decoded the JSON
        // string, so the text still needs quoting applied before the variant name
        // is matched. This is the stdlib's own way to re-enter the derived
        // `Deserialize` with a value it already holds.
        Ok(serde_json::from_value(serde_json::Value::String(name)).unwrap_or_default())
    })
}

impl VizView {
    pub const ALL: [VizView; 9] = [
        VizView::Bars,
        VizView::Wave,
        VizView::Radial,
        VizView::Spectrogram,
        VizView::Flame,
        VizView::Chladni,
        VizView::Nebula,
        VizView::Plasma,
        VizView::Trails,
    ];

    /// Dropdown label in the visualizer pane header.
    pub fn name(self) -> &'static str {
        match self {
            VizView::Bars => "Bars",
            VizView::Wave => "Wave",
            VizView::Radial => "Radial",
            VizView::Spectrogram => "Spectrogram",
            VizView::Flame => "Flame",
            VizView::Chladni => "Chladni",
            VizView::Nebula => "Nebula",
            VizView::Plasma => "Plasma",
            VizView::Trails => "Trails",
        }
    }
}

/// Unified config — single JSON file. Dock layout stays separate.
///
/// `Default` is written out rather than derived, and must agree with the
/// `#[serde(default = …)]` attributes below: a *missing* file deserializes
/// through them, and this is what a missing or unreadable file falls back to.
/// Derived, the two disagreed and a fresh install got `volume: 0.0` — a silent
/// player — where an install with `{}` got 1.0.
#[derive(Serialize, Deserialize)]
pub struct Config {
    /// The only field that was ever required, and a missing one cost the whole
    /// file: `""` is not a theme id, and `ThemeState::load` falls back to the
    /// default theme for an unknown one, so "" is a GUI-free "unset".
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub eq: EqData,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default, deserialize_with = "de_viz_view")]
    pub viz_view: VizView,
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Output buffer size in frames — larger = more underrun slack, more latency.
    #[serde(default = "default_buffer_size")]
    pub buffer_size: u32,
    /// Spool cache ceiling in MB. Tagging a share downloads whole files and the
    /// cache has no other eviction, so without a bound it grows without limit.
    /// Only never-played files are evicted; a played track is never a candidate.
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

fn default_volume() -> f32 {
    1.0
}
fn default_crossfade_secs() -> f32 {
    3.0
}
fn default_buffer_size() -> u32 {
    8192
}
/// 2 GiB of never-played spool, sized for a laptop with room to spare. Raise
/// `spool_cache_mb` in config.json to keep more of a browsed share on disk.
fn default_spool_cache_mb() -> u32 {
    2048
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: String::new(),
            eq: EqData::default(),
            shuffle: false,
            repeat: false,
            viz_view: VizView::default(),
            volume: default_volume(),
            buffer_size: default_buffer_size(),
            spool_cache_mb: default_spool_cache_mb(),
            last_playlist: None,
            library: LibraryData::default(),
            balance: 0.0,
            remaining: false,
            gapless: false,
            crossfade: false,
            crossfade_secs: default_crossfade_secs(),
            servers: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct EqData {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gains")]
    pub gains: [f32; EQ_BANDS],
}

fn default_gains() -> [f32; EQ_BANDS] {
    [0.0; EQ_BANDS]
}

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
/// The live counterpart of the matching `Config` fields: `from_config` reads them
/// at startup, `TPlayApp::snapshot` reads them back at save time. The setters
/// here clamp, and whether a write is needed is asked once, by comparing the
/// whole config against the copy on disk.
pub struct Prefs {
    // Private like `balance`: every one of the six has a getter, a clamping
    // setter and a place in `snapshot`, so a public field would be a second way
    // in that skipped the clamp.
    viz_view: VizView,
    remaining: bool,
    gapless: bool,
    crossfade: bool,
    crossfade_secs: f32,
    /// Balance (L/R). An `Arc` because `BalanceSource` reads it per audio frame on
    /// the audio thread while the GUI writes it — the same live-shared shape as
    /// `EqShared`. Not a plain `f32` for exactly that reason.
    balance: Arc<RwLock<f32>>,
}

impl Prefs {
    pub fn from_config(c: &Config) -> Self {
        Self {
            viz_view: c.viz_view,
            remaining: c.remaining,
            gapless: c.gapless,
            crossfade: c.crossfade,
            // Clamp on load, so a hand-edited config.json cannot put a
            // 900-second (or negative) crossfade into the fade math.
            crossfade_secs: c.crossfade_secs.clamp(0.0, MAX_CROSSFADE_SECS),
            balance: Arc::new(RwLock::new(c.balance)),
        }
    }

    pub fn viz_view(&self) -> VizView {
        self.viz_view
    }

    pub fn set_viz_view(&mut self, v: VizView) {
        self.viz_view = v;
    }

    pub fn remaining(&self) -> bool {
        self.remaining
    }

    pub fn set_remaining(&mut self, v: bool) {
        self.remaining = v;
    }

    pub fn gapless(&self) -> bool {
        self.gapless
    }

    pub fn toggle_gapless(&mut self) {
        self.gapless = !self.gapless;
    }

    pub fn crossfade(&self) -> bool {
        self.crossfade
    }

    pub fn toggle_crossfade(&mut self) {
        self.crossfade = !self.crossfade;
    }

    pub fn crossfade_secs(&self) -> f32 {
        self.crossfade_secs
    }

    /// Clamped to what the UI offers, so a hand-edited value that survives
    /// `from_config`'s clamp cannot be re-widened from here.
    pub fn set_crossfade_secs(&mut self, secs: f32) {
        self.crossfade_secs = secs.clamp(0.0, MAX_CROSSFADE_SECS);
    }

    pub fn balance(&self) -> f32 {
        *self.balance.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_balance(&mut self, v: f32) {
        *self.balance.write().unwrap_or_else(PoisonError::into_inner) = v.clamp(-1.0, 1.0);
    }

    /// The handle `BalanceSource` holds. Cloned, not shared by reference — the
    /// source outlives any borrow of `self`.
    pub fn balance_shared(&self) -> Arc<RwLock<f32>> {
        Arc::clone(&self.balance)
    }
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(FILE))
}

/// Read config.json. A missing or unreadable file is a fresh install, not an
/// error — the app starts on defaults.
///
/// A *malformed* one is a different case: `TPlayApp::new` seeds its write
/// baseline from what this returns, so returning defaults here means the first
/// `flush_config` overwrites the user's settings. Keep the original before that
/// can happen.
pub fn load() -> Config {
    load_from(path().as_deref())
}

/// `load` against an explicit path, so the malformed-file case is testable
/// without writing to the user's real config.
pub fn load_from(p: Option<&Path>) -> Config {
    let Some(p) = p else { return Config::default() };
    let Ok(text) = std::fs::read_to_string(p) else {
        return Config::default();
    };
    match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            let kept = p.with_extension("json.broken");
            let _ = std::fs::write(&kept, &text);
            eprintln!(
                "tplay: {} is malformed ({e}); original kept at {}",
                p.display(),
                kept.display()
            );
            Config::default()
        }
    }
}

pub fn save(config: &Config) {
    let Some(path) = path() else { return };
    let Ok(json) = serde_json::to_string_pretty(config) else {
        return;
    };
    atomic_write(&path, &json);
}

// ── When to write ───────────────────────────────────────────────────────────

/// How long to let changes settle before writing config.json.
pub const CONFIG_SAVE_DEBOUNCE_SECS: f64 = 0.5;

/// Whether config.json should be written right now.
///
/// `changed` is "the config's content differs from the copy on disk", computed
/// by the caller (`TPlayApp::flush_config`) — not a hand-set flag, so no setter
/// has to remember to raise it. The debounce exists because the widgets behind
/// those setters — the EQ pane's 10 vertical sliders, volume, balance — fire
/// `resp.changed()` on **every frame of a drag**, and the file is ~500 bytes of
/// JSON; a changed-and-changed-back drag writes nothing at all.
///
/// Pure and time-injected so the policy is testable without an eframe `Context`:
/// `now` and `last_save` are both seconds on the same clock
/// (`ctx.input(|i| i.time)`), and `closing` is `close_requested()`.
///
/// `closing` is the counterpart to the debounce: it costs at most one debounce
/// window of settings on a hard kill, which the close flush buys back. Same
/// shape `gui/coordinator.rs` uses for dock_layout.json.
pub fn should_flush(changed: bool, now: f64, last_save: f64, closing: bool) -> bool {
    if !changed {
        return false;
    }
    if closing {
        return true;
    }
    now - last_save >= CONFIG_SAVE_DEBOUNCE_SECS
}

/// The write-throttle state one persisted file keeps: the content last written
/// and when it was written.
///
/// It exists because two files now keep that pair by hand — config.json and the
/// library database — and a third (`dock_layout.json`, in the GUI layer) keeps
/// the same idea in egui memory because the coordinator has no app state to hold
/// it. The *policy* stays in [`should_flush`], which is pure and tested on its
/// own; this only holds the two values the policy reads, so neither caller can
/// half-apply it.
pub struct Persisted {
    last: String,
    at: f64,
}

impl Persisted {
    /// Seed from what was just read, so the first write happens only if
    /// something has already changed.
    pub fn new(seed: &str) -> Self {
        Self {
            last: seed.to_owned(),
            at: 0.0,
        }
    }

    /// Whether it is worth *serializing* this frame.
    ///
    /// The debounce is asked here as well as at the write, because the point of
    /// asking is to avoid the work: `config.json` is ~500 bytes, so serializing
    /// it every frame to discover nothing changed is noise, but a
    /// `library.json` is the whole tag cache, so that same line becomes
    /// megabytes a second and grows with the library. Inside the window the
    /// answer cannot be "yes" anyway, so asking is pure waste. `closing` always
    /// asks, which is what buys back the change made inside the window.
    pub fn due(&self, now: f64, closing: bool) -> bool {
        closing || now - self.at >= CONFIG_SAVE_DEBOUNCE_SECS
    }

    /// Whether `content` differs from the last write and the debounce window has
    /// passed.
    pub fn wants_write(&self, content: &str, now: f64, closing: bool) -> bool {
        should_flush(content != self.last, now, self.at, closing)
    }

    /// Record a write. Called *before* the file is written, so a write that fails
    /// is not retried every frame for the rest of the session.
    pub fn note_written(&mut self, content: &str, now: f64) {
        self.last.clear();
        self.last.push_str(content);
        self.at = now;
    }
}

/// Write `content` to `path` atomically: into `<name>.part`, then renamed over the
/// target.
///
/// A plain `fs::write` truncates in place, so a process killed mid-write leaves a
/// file that reads back as *malformed* — and the recovery for a malformed file is
/// to reset everything (for config.json) or to start over (for the library
/// database, which would cost the user's play history). The `.broken` copy
/// rescues a file that was *already* damaged; this is what stops the next write
/// from being the one that damages it.
pub fn atomic_write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // `<name>.part` rather than a replaced extension, so the temporary file is
    // recognisably the same file rather than a second one: `config.json.part`.
    let mut partial = path.as_os_str().to_os_string();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    if std::fs::write(&partial, content).is_ok() && std::fs::rename(&partial, path).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&partial);
}
