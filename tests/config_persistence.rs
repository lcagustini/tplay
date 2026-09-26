//! Config persistence — `~/.config/tplay/config.json` shape (serde
//! round-trip, missing-field defaults) and the write throttle
//! (`config::should_flush`). `TPlayApp::save_config` writes this file; the
//! on-disk contract and the decision of *when* to write are what's pinned here.

use tplay::app::{Config, EqData, LibraryData, VizView};
use tplay::config::{should_flush, CONFIG_SAVE_DEBOUNCE_SECS};
use tplay::network::ServerCfg;
use std::path::PathBuf;

// ── The write throttle ──────────────────────────────────────────────────────
//
// Every settings setter used to write config.json immediately, and the EQ
// band's 10 vertical sliders plus volume and balance call their setter on every
// frame of a drag (`resp.changed()`), so a drag wrote the whole file ~60×/sec
// on the UI thread. Setters now mark it dirty and one flush per frame decides.

#[test]
fn a_clean_config_is_never_written() {
    // Even at the far end of a debounce window, and even while closing: nothing
    // changed, so there is nothing to write.
    assert!(!should_flush(false, 100.0, 0.0, false));
    assert!(!should_flush(false, 100.0, 0.0, true));
}

#[test]
fn a_freshly_changed_config_waits_for_the_window() {
    // Changed 0.1s after the last write: inside the debounce.
    assert!(!should_flush(true, 0.1, 0.0, false));
    // Exactly on the boundary counts as due — otherwise the last change before
    // a quiet period could sit unwritten.
    assert!(should_flush(true, CONFIG_SAVE_DEBOUNCE_SECS, 0.0, false));
    assert!(should_flush(true, 10.0, 0.0, false));
}

#[test]
fn closing_overrides_the_debounce() {
    // The whole point of the debounce is that it can cost up to one window of
    // settings on a hard kill. The close flush buys that back, so a change made
    // a millisecond ago must still be written.
    assert!(should_flush(true, 0.001, 0.0, true));
    // And a *clean* config still isn't rewritten just because the window closed.
    assert!(!should_flush(false, 0.001, 0.0, true));
}

/// The property the throttle exists for: a continuous drag cannot write more
/// often than one debounce window, however many frames it spans. 2s of 60Hz
/// frames coalesces into a handful of writes, not 120.
#[test]
fn a_drag_cannot_write_more_often_than_the_window() {
    let mut last = f64::NEG_INFINITY;
    let mut gaps: Vec<f64> = Vec::new();
    let mut writes = 0;
    for frame in 0..120 {
        let now = frame as f64 / 60.0; // ~2s of dragging
        if should_flush(true, now, last, false) {
            if writes > 0 {
                gaps.push(now - last);
            }
            last = now;
            writes += 1;
        }
    }
    assert!(writes >= 3, "2s of dragging must still get written: {writes} writes");
    assert!(writes < 10, "120 frames must coalesce into a handful of writes, got {writes}");
    assert!(
        gaps.iter().all(|g| *g >= CONFIG_SAVE_DEBOUNCE_SECS),
        "every write must be at least one window after the last: {gaps:?}"
    );
}

#[test]
fn config_round_trip_preserves_every_field() {
    let c = Config {
        theme: "neon".into(),
        eq: EqData { enabled: true, gains: [1.0; 10] },
        shuffle: true,
        repeat: true,
        viz_view: VizView::Wave,
        volume: 0.5,
        buffer_size: 16384,
        spool_cache_mb: 8192,
        last_playlist: Some("/music/chill.tplay".into()),
        library: LibraryData {
            favorites: vec!["/music/favs".into()],
            last_dir: "/music".into(),
            show_hidden: true,
        },
        balance: 0.25,
        remaining: true,
        gapless: true,
        crossfade: true,
        crossfade_secs: 5.0,
        servers: vec![
            ServerCfg { host: "192.168.1.50".into(), username: "lucas".into() },
            ServerCfg { host: "box".into(), username: String::new() },
        ],
    };

    let json = serde_json::to_string(&c).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(back.theme, "neon");
    assert!(back.eq.enabled);
    assert_eq!(back.eq.gains, [1.0; 10]);
    assert!(back.shuffle);
    assert!(back.repeat);
    assert_eq!(back.viz_view, VizView::Wave);
    assert_eq!(back.volume, 0.5);
    assert_eq!(back.buffer_size, 16384);
    assert_eq!(back.spool_cache_mb, 8192);
    assert_eq!(back.last_playlist.as_deref(), Some("/music/chill.tplay"));
    assert_eq!(back.library.favorites, vec![PathBuf::from("/music/favs")]);
    assert_eq!(back.library.last_dir, "/music");
    assert!(back.library.show_hidden);
    assert_eq!(back.balance, 0.25);
    assert!(back.remaining);
    assert!(back.gapless);
    assert!(back.crossfade);
    assert_eq!(back.crossfade_secs, 5.0);
    assert_eq!(
        back.servers,
        vec![
            ServerCfg { host: "192.168.1.50".into(), username: "lucas".into() },
            ServerCfg { host: "box".into(), username: String::new() },
        ]
    );
}

#[test]
fn config_missing_fields_fall_back_to_defaults() {
    // Only `theme` is mandatory; a config written by an older version (or a
    // hand-edited one) must still load with sane defaults.
    let json = r#"{"theme":"dark"}"#;
    let c: Config = serde_json::from_str(json).unwrap();

    assert_eq!(c.theme, "dark");
    assert!(!c.eq.enabled);
    assert_eq!(c.eq.gains, [0.0; 10]);
    assert!(!c.shuffle);
    assert!(!c.repeat);
    assert_eq!(c.viz_view, VizView::Bars); // default view
    assert_eq!(c.volume, 1.0); // default_volume
    assert_eq!(c.buffer_size, 8192); // default_buffer_size
    assert_eq!(c.spool_cache_mb, 2048); // default_spool_cache_mb
    assert_eq!(c.last_playlist, None);
    assert!(c.library.favorites.is_empty());
    assert_eq!(c.library.last_dir, "");
    assert!(!c.library.show_hidden);
    assert_eq!(c.balance, 0.0);
    assert!(!c.remaining);
    assert!(!c.gapless);
    assert!(!c.crossfade);
    assert_eq!(c.crossfade_secs, 3.0); // default_crossfade_secs
    assert!(c.servers.is_empty()); // no servers saved → guest browsing only
}

#[test]
fn config_keeps_unknown_theme_id() {
    // Theme resolution (falling back to dark on an unknown id) is app-level;
    // the config file itself never validates the value.
    let json = r#"{"theme":"does-not-exist"}"#;
    let c: Config = serde_json::from_str(json).unwrap();
    assert_eq!(c.theme, "does-not-exist");
}

#[test]
fn config_json_shape_has_expected_keys() {
    let c = Config::default();
    let v: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();

    assert!(v.get("theme").is_some());
    assert!(v.get("shuffle").is_some());
    assert!(v.get("spool_cache_mb").is_some());
    assert!(v.get("repeat").is_some());
    assert!(v.get("viz_view").is_some());
    assert!(v.get("volume").is_some());
    assert!(v.get("buffer_size").is_some());
    assert!(v.get("last_playlist").is_some());
    let eq = v.get("eq").expect("eq object");
    assert!(eq.get("enabled").is_some());
    assert_eq!(eq.get("gains").and_then(|g| g.as_array()).map(Vec::len), Some(10));
    let lib = v.get("library").expect("library object");
    assert!(lib.get("favorites").is_some());
    assert!(lib.get("last_dir").is_some());
    assert!(lib.get("show_hidden").is_some());
    assert!(v.get("balance").is_some());
    assert!(v.get("remaining").is_some());
    assert!(v.get("gapless").is_some());
    assert!(v.get("crossfade").is_some());
    assert!(v.get("crossfade_secs").is_some());
    assert!(v.get("servers").is_some()); // SMB server list (host + username only)
}