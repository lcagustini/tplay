//! Config persistence — `~/.config/tplay/config.json` shape (serde
//! round-trip, missing-field defaults). `TPlayApp::save_config` writes this
//! every change; the on-disk contract is what's pinned here.

use tplay::app::{Config, EqData, LibraryData, VizView};
use std::path::PathBuf;

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
    };

    let json = serde_json::to_string(&c).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(back.theme, "neon");
    assert_eq!(back.eq.enabled, true);
    assert_eq!(back.eq.gains, [1.0; 10]);
    assert_eq!(back.shuffle, true);
    assert_eq!(back.repeat, true);
    assert_eq!(back.viz_view, VizView::Wave);
    assert_eq!(back.volume, 0.5);
    assert_eq!(back.buffer_size, 16384);
    assert_eq!(back.last_playlist.as_deref(), Some("/music/chill.tplay"));
    assert_eq!(back.library.favorites, vec![PathBuf::from("/music/favs")]);
    assert_eq!(back.library.last_dir, "/music");
    assert_eq!(back.library.show_hidden, true);
    assert_eq!(back.balance, 0.25);
    assert!(back.remaining);
    assert!(back.gapless);
    assert!(back.crossfade);
    assert_eq!(back.crossfade_secs, 5.0);
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
    assert_eq!(c.last_playlist, None);
    assert!(c.library.favorites.is_empty());
    assert_eq!(c.library.last_dir, "");
    assert!(!c.library.show_hidden);
    assert_eq!(c.balance, 0.0);
    assert!(!c.remaining);
    assert!(!c.gapless);
    assert!(!c.crossfade);
    assert_eq!(c.crossfade_secs, 3.0); // default_crossfade_secs
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
}