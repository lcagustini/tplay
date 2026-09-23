//! GUI tests — theme application, dock layout (headless egui).

use tplay::gui::theme::{Themes, Base, Layout, Icon, DEFAULT_THEME_ID};
use tplay::app::Pane;
use tplay::audio::eq::EqShared;
use eframe::egui::{self, FontFamily, Color32};
use std::path::Path;

#[test]
fn themes_loads_builtin_dark_theme() {
    let themes = Themes::load();
    assert!(!themes.list().is_empty(), "at least dark theme should exist");
    let dark = themes.get("dark").expect("dark theme missing");
    assert_eq!(dark.id, "dark");
    assert_eq!(dark.base, Base::Dark);
}

#[test]
fn theme_palette_has_all_required_tokens() {
    let themes = Themes::load();
    let dark = themes.get("dark").unwrap();

    // Check all 14 palette tokens exist (as Color32, not strings)
    let p = &dark.palette;
    // These are Color32 values, just verify they're not default
    assert_ne!(p.bg, egui::Color32::default());
    assert_ne!(p.panel_bg, egui::Color32::default());
    assert_ne!(p.text_primary, egui::Color32::default());
    assert_ne!(p.text_secondary, egui::Color32::default());
    assert_ne!(p.accent, egui::Color32::default());
    assert_ne!(p.border, egui::Color32::default());
    assert_ne!(p.progress_fill, egui::Color32::default());
    assert_ne!(p.slider_track, egui::Color32::default());
    assert_ne!(p.slider_handle, egui::Color32::default());
    assert_ne!(p.focus_ring, egui::Color32::default());
    assert_ne!(p.row_even, egui::Color32::default());
    assert_ne!(p.row_odd, egui::Color32::default());
    assert_ne!(p.btn_hover, egui::Color32::default());
}

#[test]
fn theme_metadata_font_values() {
    let themes = Themes::load();
    for theme in themes.list() {
        match theme.metadata_font {
            FontFamily::Monospace | FontFamily::Proportional => {},
            _ => panic!("unknown metadata_font variant"),
        }
    }
}

#[test]
fn coordinator_applies_theme_visuals() {
    let ctx = egui::Context::default();
    let themes = Themes::load();
    let theme = themes.get("dark").unwrap();
    
    // This should not panic
    tplay::gui::theme::apply(&ctx, theme);
    
    // Visuals should be modified
    let visuals = ctx.style().visuals.clone();
    assert_ne!(visuals.panel_fill, egui::Color32::default());
}

#[test]
fn pane_enum_serializes() {
    use serde_json;
    let panes = [Pane::NowPlaying, Pane::Playlist, Pane::Equalizer, Pane::Library];
    for pane in panes {
        let json = serde_json::to_string(&pane).unwrap();
        let back: Pane = serde_json::from_str(&json).unwrap();
        assert_eq!(pane, back);
    }
}

#[test]
fn eq_shared_default_gains_are_zero() {
    let shared = EqShared::default();
    assert_eq!(shared.gains, [0.0; 10]);
    assert!(!shared.enabled);
}

#[test]
fn theme_layout_defaults() {
    let layout = Layout::default().with_defaults();
    assert_eq!(layout.eq_slider_min_h, 60.0);
    assert_eq!(layout.eq_slider_max_h, 220.0);
    assert_eq!(layout.eq_band_w_min, 30.0);
    assert_eq!(layout.eq_header_gap, 6.0);
    assert_eq!(layout.eq_band_gap, 2.0);
    assert_eq!(layout.text_meta, 12.0);
    assert_eq!(layout.text_time, 13.0);
    assert_eq!(layout.row_tint_alpha, 0.10);
}

#[test]
fn theme_base_enum() {
    // Test Base enum serialization
    use serde_json;
    let dark = Base::Dark;
    let light = Base::Light;
    let json_dark = serde_json::to_string(&dark).unwrap();
    let json_light = serde_json::to_string(&light).unwrap();
    assert_eq!(json_dark, "\"Dark\"");
    assert_eq!(json_light, "\"Light\"");
    let back_dark: Base = serde_json::from_str(&json_dark).unwrap();
    let back_light: Base = serde_json::from_str(&json_light).unwrap();
    assert_eq!(back_dark, Base::Dark);
    assert_eq!(back_light, Base::Light);
}

#[test]
fn default_theme_id_constant() {
    assert_eq!(DEFAULT_THEME_ID, "dark");
}

// ── Theme dir priority + icon fallback ────────────────────────────────

/// Fixture theme.json written into a temp dir tree.
fn write_theme(dir: &Path, id: &str, palette_accent: &str) {
    std::fs::create_dir_all(dir.join(id)).unwrap();
    std::fs::write(
        dir.join(id).join("theme.json"),
        format!(
            r##"{{"id":"{id}","name":"{id}","base":"dark",
             "palette":{{"bg":"#101010","panel_bg":"#181818",
             "text_primary":"#e0e0e0","text_secondary":"#8c8c8c",
             "accent":"{palette_accent}","border":"#333333",
             "progress_fill":"#2ea3f0","slider_track":"#222222",
             "slider_handle":"#999999","row_even":"#141414",
             "row_odd":"#1c1c1c","btn_hover":"#282828",
             "focus_ring":"#2ea3f0"}}}}"##,
        ),
    )
    .unwrap();
}

#[test]
fn higher_priority_dir_wins_and_icons_fall_back() {
    let base = std::env::temp_dir().join(format!("tplay-test-{}", std::process::id()));
    let overrides = base.join("overrides");
    let bundled = base.join("bundled");
    write_theme(&overrides, "dark", "#ff0000");
    write_theme(&bundled, "dark", "#00ff00"); // must lose to overrides
    write_theme(&bundled, "mine", "#0000ff"); // no icons dir
    std::fs::create_dir_all(overrides.join("dark").join("icons")).unwrap();
    std::fs::write(overrides.join("dark").join("icons").join("play.png"), b"png").unwrap();

    // Priority order: first dir in the slice wins on id clash.
    let themes = Themes::load_from(&[overrides.clone(), bundled]);
    assert_eq!(themes.list().len(), 2);
    assert_eq!(themes.default().palette.accent, Color32::from_rgb(0xff, 0, 0));
    // "mine" has no icons of its own → falls back to the default theme's.
    let mine = themes.get("mine").unwrap();
    assert!(themes.icon_path(mine, Icon::Play).unwrap().ends_with("dark/icons/play.png"));
    assert!(themes.icon_path(mine, Icon::Volume).is_none()); // dark has no volume.png

    let _ = std::fs::remove_dir_all(&base);
}