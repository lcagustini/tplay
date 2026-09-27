//! The four settings/state owners that used to be bare `TPlayApp` fields:
//! `config::Prefs`, `audio::eq::EqSettings`, `gui::theme::ThemeState` and
//! `library::LibraryState`.
//!
//! They were moved out for cohesion — the presets sit beside the filters they
//! curve, the icon re-decode sits beside `load_icons` — and the testability
//! follows from it. `LibraryState` is the one that could not be exercised at all
//! before, because listing a folder also kicked off a tag scan, so the *state*
//! was welded to the *effect*.
//!
//! None of this needs an audio device or an eframe window, which is the point:
//! `Prefs` and `EqSettings` wrap `Arc<RwLock<_>>` handles that a `TPlayApp`
//! would otherwise have to hand out, and here they can be built directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[path = "common.rs"]
mod common;

use eframe::egui;
use tplay::audio::eq::{EqSettings, EQ_FREQUENCIES, EQ_PRESETS};
use tplay::config::{Config, Prefs, MAX_CROSSFADE_SECS};
use tplay::gui::theme::{ThemeState, Themes, DEFAULT_THEME_ID};
use tplay::library::{LibraryState, TagCache, TrackInfo};

// ── config::Prefs ────────────────────────────────────────────────────────────

fn prefs() -> Prefs {
    Prefs::from_config(&Config::default())
}

/// A repeated write of the same value is a no-op, and a real write is a
/// read-back. The setters no longer *report* a change — the config compare in
/// `TPlayApp::flush_config` is what decides that, by serializing the whole
/// config and comparing it to the copy on disk (pinned by
/// `equal_configs_serialize_identically` in `config_persistence.rs`).
#[test]
fn prefs_writes_are_idempotent() {
    let mut p = prefs();
    assert!(!p.remaining());
    p.set_remaining(true);
    assert!(p.remaining());
    p.set_remaining(true);
    assert!(
        p.remaining(),
        "writing the same value again changes nothing"
    );
    p.set_remaining(false);
    assert!(!p.remaining(), "flipping back is a write");

    let mut p = prefs();
    p.set_viz_view(tplay::app::VizView::Wave);
    assert_eq!(p.viz_view(), tplay::app::VizView::Wave);
}

#[test]
fn prefs_clamps_what_the_ui_offers() {
    let mut p = prefs();
    // Crossfade duration: 0..MAX, and a hand-edited config is clamped on load.
    p.set_crossfade_secs(-5.0);
    assert_eq!(
        p.crossfade_secs(),
        0.0,
        "a negative crossfade must clamp to 0"
    );
    p.set_crossfade_secs(900.0);
    assert_eq!(
        p.crossfade_secs(),
        MAX_CROSSFADE_SECS,
        "must clamp to the UI maximum"
    );
    p.set_crossfade_secs(3.0);
    assert_eq!(p.crossfade_secs(), 3.0);

    // A config.json edited to 900s must not reach the fade math.
    let edited = Config {
        crossfade_secs: 900.0,
        ..Default::default()
    };
    assert_eq!(
        Prefs::from_config(&edited).crossfade_secs(),
        MAX_CROSSFADE_SECS
    );

    // Balance: the full -1..=1 stereo range.
    p.set_balance(-9.0);
    assert_eq!(p.balance(), -1.0, "hard left");
    p.set_balance(9.0);
    assert_eq!(p.balance(), 1.0, "hard right");
    p.set_balance(0.0);
    assert_eq!(p.balance(), 0.0, "centre");
}

/// The balance handle the audio source holds is *the same* value the getter
/// reports — this is the "live, no sink rebuild" property, and the reason the
/// field is an `Arc` rather than a plain `f32`.
#[test]
fn prefs_balance_handle_is_the_live_value() {
    let mut p = prefs();
    let handle = p.balance_shared();
    p.set_balance(0.5);
    assert_eq!(
        *handle.read().unwrap(),
        0.5,
        "the source must see the new value"
    );
    *handle.write().unwrap() = -0.25; // as a source-side write would not do, but proves sharing
    assert_eq!(
        p.balance(),
        -0.25,
        "the getter and the handle are one value"
    );
}

/// Round-trip through `Config`: everything `from_config` reads must be a field
/// the app can write back, or a setting silently resets on the next save.
#[test]
fn prefs_round_trips_through_config() {
    let c = Config {
        viz_view: tplay::app::VizView::Wave,
        remaining: true,
        gapless: true,
        crossfade: true,
        crossfade_secs: 7.5,
        balance: -0.5,
        ..Default::default()
    };

    let p = Prefs::from_config(&c);
    assert_eq!(p.viz_view(), tplay::app::VizView::Wave);
    assert!(p.remaining() && p.gapless() && p.crossfade());
    assert_eq!(p.crossfade_secs(), 7.5);
    assert_eq!(p.balance(), -0.5);
}

#[test]
fn prefs_toggles_always_change() {
    let mut p = prefs();
    p.toggle_gapless();
    assert!(p.gapless());
    p.toggle_gapless();
    assert!(!p.gapless());
    p.toggle_crossfade();
    assert!(p.crossfade());
}

// ── audio::eq::EqSettings ────────────────────────────────────────────────────

#[test]
fn eq_settings_clamps_bands_to_the_ui_range() {
    let eq = EqSettings::new(false, [0.0; 10]);
    assert!(eq.set_band(0, 99.0));
    assert_eq!(eq.gains()[0], 12.0, "must clamp to +12 dB");
    assert!(eq.set_band(1, -99.0));
    assert_eq!(eq.gains()[1], -12.0, "must clamp to -12 dB");
}

#[test]
fn eq_settings_ignores_an_out_of_range_band() {
    let eq = EqSettings::new(false, [0.0; 10]);
    assert!(
        !eq.set_band(EQ_FREQUENCIES.len(), 3.0),
        "no such band, no change"
    );
    assert!(!eq.set_band(99, 3.0));
    assert_eq!(eq.gains(), [0.0; 10], "nothing was written");
}

#[test]
fn eq_settings_reports_no_change_for_the_same_gain() {
    let eq = EqSettings::new(false, [0.0; 10]);
    assert!(eq.set_band(3, 4.0));
    assert!(
        !eq.set_band(3, 4.0),
        "same gain again must not dirty the config"
    );
    assert!(eq.set_band(3, 4.001), "a real nudge counts");
}

#[test]
fn eq_settings_derives_the_preset_name_from_the_gains() {
    let eq = EqSettings::new(false, [0.0; 10]);
    assert_eq!(eq.preset(), Some("Flat"), "all-zero is the Flat preset");
    for (name, gains) in EQ_PRESETS {
        eq.set_preset(Some(name));
        assert_eq!(eq.gains(), gains, "{name} must apply its own curve");
        assert_eq!(eq.preset(), Some(name), "{name} must be recognised");
    }
    // A hand-tweaked curve is Custom, not silently the nearest preset.
    eq.set_band(0, 0.1);
    assert_eq!(eq.preset(), None);
}

#[test]
fn eq_settings_preset_none_and_unknown_names_are_no_ops() {
    let eq = EqSettings::new(false, [1.0; 10]);
    assert!(
        !eq.set_preset(None),
        "None is the ComboBox's Custom label, not an edit"
    );
    assert!(!eq.set_preset(Some("No Such Preset")));
    assert_eq!(eq.gains(), [1.0; 10], "gains untouched");
    // Re-applying the preset already loaded is not a change.
    eq.set_preset(Some("Flat"));
    assert!(!eq.set_preset(Some("Flat")));
}

/// The source and the settings must share one handle — two `Arc`s would mean the
/// EQ silently stops responding to the sliders.
#[test]
fn eq_settings_handle_is_shared_with_the_source() {
    let eq = EqSettings::new(false, [0.0; 10]);
    let handle = eq.shared();
    eq.set_band(5, 6.0);
    assert_eq!(handle.read().unwrap().gains[5], 6.0);
    handle.write().unwrap().enabled = true;
    assert!(
        eq.enabled(),
        "a write through the handle is visible to the settings"
    );
}

#[test]
fn eq_settings_toggle_flips_enabled() {
    let eq = EqSettings::new(false, [0.0; 10]);
    assert!(!eq.enabled());
    eq.toggle();
    assert!(eq.enabled());
    eq.toggle();
    assert!(!eq.enabled());
}

// ── gui::theme::ThemeState ──────────────────────────────────────────────────

/// A minimal but schema-valid theme.json. 14 palette tokens are required; the
/// layout block is optional.
fn write_theme(root: &std::path::Path, id: &str, accent: &str) {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("theme.json"),
        format!(
            r##"{{"id":"{id}","name":"{id}","base":"dark",
                 "palette":{{"bg":"#101010","panel_bg":"#181818",
                 "text_primary":"#e0e0e0","text_secondary":"#8c8c8c",
                 "accent":"{accent}","border":"#333333",
                 "progress_fill":"{accent}","slider_track":"#222222",
                 "slider_handle":"#999999","row_even":"#141414",
                 "row_odd":"#1c1c1c","btn_hover":"#282828",
                 "focus_ring":"{accent}"}}}}"##
        ),
    )
    .unwrap();
}

/// A fresh themes root **per test**. These used to share one
/// `…-themestate-<pid>` path, and since cargo runs test binaries' tests in
/// parallel they raced: one test's `remove_dir_all` deleted another's fixture
/// mid-run, which surfaced as a baffling "re-selecting the current theme
/// changes nothing" failure.
fn theme_fixture(name: &str) -> PathBuf {
    let root = common::test_dir(name);
    write_theme(&root, "alpha", "#2ea3f0");
    write_theme(&root, "beta", "#e0508a");
    root
}

/// A broken install (no themes on disk) must still yield a usable theme — the
/// hardcoded fallback is what stops a missing `themes/` from rendering nothing.
#[test]
fn theme_state_falls_back_when_the_id_is_unknown() {
    let root = theme_fixture("themestate-fallback");
    let ctx = egui::Context::default();
    let themes = Themes::load_from(std::slice::from_ref(&root));

    let known = ThemeState::load(&ctx, themes, "alpha");
    assert_eq!(known.current().id, "alpha");

    let bogus = ThemeState::load(&ctx, known_themes(&root), "nope");
    assert_eq!(
        bogus.current().id,
        DEFAULT_THEME_ID,
        "an unknown id must fall back to the default, not render nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

fn known_themes(root: &std::path::Path) -> Themes {
    Themes::load_from(std::slice::from_ref(&root.to_path_buf()))
}

/// Switching re-points the theme; re-selecting the current one is *not* a
/// change, because re-decoding every icon texture for nothing would be pure
/// waste — and it must not dirty the config either.
#[test]
fn theme_state_set_switches_once_and_is_idempotent() {
    let root = theme_fixture("themestate-set");
    let ctx = egui::Context::default();
    let mut ts = ThemeState::load(&ctx, known_themes(&root), "alpha");
    assert_eq!(ts.current().id, "alpha");

    assert!(
        !ts.set(&ctx, "alpha"),
        "re-selecting the current theme changes nothing"
    );
    assert!(ts.set(&ctx, "beta"), "a different theme is a change");
    assert_eq!(ts.current().id, "beta");
    assert!(!ts.set(&ctx, "beta"), "and is idempotent once applied");
    assert!(
        !ts.set(&ctx, "nope"),
        "an unknown id is refused, not a silent no-op"
    );
    assert_eq!(
        ts.current().id,
        "beta",
        "a refused switch must not move the theme"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn theme_state_lists_every_loadable_theme() {
    let root = theme_fixture("themestate-list");
    let ctx = egui::Context::default();
    let ts = ThemeState::load(&ctx, known_themes(&root), "alpha");
    let ids: Vec<&str> = ts.list().iter().map(|t| t.id.as_str()).collect();
    assert!(
        ids.contains(&"alpha") && ids.contains(&"beta"),
        "got {ids:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// These fixtures ship no icon files, so every slot rasterizes to `None` — which
/// is the documented "pane renders a unicode glyph" path. The point is that
/// asking for an icon out of bounds is `None` rather than a panic.
#[test]
fn theme_state_icons_are_none_without_files() {
    let root = theme_fixture("themestate-list");
    let ctx = egui::Context::default();
    let ts = ThemeState::load(&ctx, known_themes(&root), "alpha");
    for icon in tplay::gui::theme::Icon::ALL {
        assert!(
            ts.icon(icon).is_none(),
            "{:?} has no file in the fixture",
            icon
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

// ── library::LibraryState ────────────────────────────────────────────────────

fn tag_cache() -> TagCache {
    HashMap::new()
}

#[test]
fn library_state_open_lists_a_folder_and_asks_for_the_audio() {
    let dir = common::test_dir("libstate-open");
    std::fs::create_dir_all(dir.join("Rock")).unwrap();
    std::fs::write(dir.join("a.mp3"), b"x").unwrap();
    std::fs::write(dir.join("list.tplay"), b"{}").unwrap();

    let mut lib = LibraryState::new(dir.clone(), vec![], false);
    let scan = lib
        .open(dir.clone(), &tag_cache())
        .expect("a real dir must open");

    // The scan covers audio only: not the subfolder (a folder has no tags of
    // its own) and not the .tplay (a playlist file is not audio) — even though
    // the .tplay *is* a row, interleaved with the songs.
    assert_eq!(scan, vec![dir.join("a.mp3")]);
    assert_eq!(lib.dir(), dir);
    assert_eq!(lib.entries().len(), 3, "subfolder + mp3 + the .tplay row");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn library_state_open_refuses_a_non_directory() {
    let dir = common::test_dir("libstate-not-a-dir").join("a-file");
    std::fs::write(&dir, b"x").unwrap();
    let mut lib = LibraryState::new(dir.clone(), vec![], false);
    assert!(lib.open(dir.clone(), &tag_cache()).is_none());
    assert!(
        lib.entries().is_empty(),
        "a refused open must not touch the rows"
    );
    std::fs::remove_file(&dir).ok();
}

#[test]
fn library_state_hidden_folders_are_opt_in() {
    let dir = common::test_dir("libstate-hidden");
    std::fs::create_dir_all(dir.join(".config")).unwrap();
    std::fs::create_dir_all(dir.join("Music")).unwrap();

    let mut lib = LibraryState::new(dir.clone(), vec![], false);
    lib.open(dir.clone(), &tag_cache()).unwrap();
    assert_eq!(lib.entries().len(), 1, "dot-folders are hidden by default");

    assert!(lib.set_show_hidden(true), "a real change");
    assert!(
        !lib.set_show_hidden(true),
        "same value again is not a change"
    );
    lib.open(dir.clone(), &tag_cache()).unwrap();
    assert_eq!(lib.entries().len(), 2, "now the dot-folder is listed");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn library_state_sort_picks_then_flips() {
    let mut lib = LibraryState::new(PathBuf::from("/tmp"), vec![], false);
    let mut cache = tag_cache();
    cache.insert(
        PathBuf::from("/tmp/b.mp3"),
        TrackInfo {
            title: "B".into(),
            ..Default::default()
        },
    );
    cache.insert(
        PathBuf::from("/tmp/a.mp3"),
        TrackInfo {
            title: "A".into(),
            ..Default::default()
        },
    );

    // The default sort is already column 0 (Title), so selecting it *flips*
    // rather than "selects" — pick a different column to test that branch.
    assert_eq!(lib.sort(), 0, "Title is the default column");
    assert!(lib.sort_asc());

    assert!(lib.set_sort(1, &cache), "Artist is a valid column");
    assert_eq!(lib.sort(), 1);
    assert!(lib.sort_asc(), "a new column starts ascending");

    assert!(
        lib.set_sort(1, &cache),
        "clicking the active column is still a change"
    );
    assert!(!lib.sort_asc(), "and it flips the direction");

    assert!(
        !lib.set_sort(999, &cache),
        "an out-of-range column is refused"
    );
    assert_eq!(lib.sort(), 1, "and must not disturb the current sort");
}

#[test]
fn library_state_toggle_favorite_reports_the_new_state() {
    let mut lib = LibraryState::new(PathBuf::from("/tmp"), vec![], false);
    assert!(!lib.is_favorite(Path::new("/tmp/music")));
    assert!(
        lib.toggle_favorite(PathBuf::from("/tmp/music")),
        "now bookmarked"
    );
    assert!(lib.is_favorite(Path::new("/tmp/music")));
    assert_eq!(lib.favorites(), &[PathBuf::from("/tmp/music")]);

    assert!(
        !lib.toggle_favorite(PathBuf::from("/tmp/music")),
        "now un-bookmarked"
    );
    assert!(!lib.is_favorite(Path::new("/tmp/music")));
    assert!(lib.favorites().is_empty());
}
