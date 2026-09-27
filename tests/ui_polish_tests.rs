//! UI-polish regression tests (headless egui): theme application behavior,
//! layout-token round-trips, and the EQ floor invariant that the review fixes
//! introduced.

use eframe::egui::{self, Color32, FontFamily};
use tplay::gui::theme::{apply, Base, Layout, Palette, Theme, Themes};

fn fixture_theme() -> Theme {
    Theme {
        id: "test".into(),
        name: "Test".into(),
        base: Base::Dark,
        metadata_font: FontFamily::Proportional,
        palette: Palette {
            bg: Color32::from_rgb(0x14, 0x14, 0x14),
            panel_bg: Color32::from_rgb(0x1d, 0x1d, 0x1d),
            text_primary: Color32::from_rgb(0xe0, 0xe0, 0xe0),
            text_secondary: Color32::from_rgb(0x8c, 0x8c, 0x8c),
            accent: Color32::from_rgb(0x2e, 0xa3, 0xf0),
            border: Color32::from_rgb(0x38, 0x38, 0x38),
            progress_fill: Color32::from_rgb(0x2e, 0xa3, 0xf0),
            slider_track: Color32::from_rgb(0x2b, 0x2b, 0x2b),
            slider_handle: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            row_even: Color32::from_rgb(0x17, 0x17, 0x17),
            row_odd: Color32::from_rgb(0x20, 0x20, 0x20),
            btn_hover: Color32::from_rgb(0x2a, 0x2a, 0x2a),
            focus_ring: Color32::from_rgb(0x2e, 0xa3, 0xf0),
        },
        layout: Layout::default().with_defaults(),
        icons_dir: None,
    }
}

#[test]
fn apply_zeroes_animation_time() {
    let ctx = egui::Context::default();
    let theme = fixture_theme();

    // egui's default animation_time is ~0.083s; a nonzero value would smear
    // every widget's colors across frames on a mid-session theme switch.
    assert!(ctx.style().animation_time > 0.0);
    apply(&ctx, &theme);
    assert_eq!(ctx.style().animation_time, 0.0);
}

#[test]
fn apply_maps_palette_tokens_to_visuals() {
    let ctx = egui::Context::default();
    let theme = fixture_theme();
    apply(&ctx, &theme);

    let v = &ctx.style().visuals;
    assert_eq!(v.panel_fill, theme.palette.bg);
    assert_eq!(v.window_fill, theme.palette.bg);
    assert_eq!(v.hyperlink_color, theme.palette.accent);
    assert_eq!(v.override_text_color, Some(theme.palette.text_primary));
    assert_eq!(v.selection.bg_fill, theme.palette.progress_fill);
    assert_eq!(v.widgets.noninteractive.bg_fill, theme.palette.panel_bg);
}

/// A theme.json with the full new layout block must round-trip through
/// load_from with the JSON values winning over code defaults.
#[test]
fn new_layout_tokens_round_trip_from_json() {
    let base = std::env::temp_dir().join(format!("tplay-test-{}", std::process::id()));
    let dir = base.join("tokens");
    std::fs::create_dir_all(dir.join("tok")).unwrap();
    std::fs::write(
        dir.join("tok").join("theme.json"),
        r##"{"id":"tok","name":"Tok","base":"dark",
             "palette":{"bg":"#101010","panel_bg":"#181818",
             "text_primary":"#e0e0e0","text_secondary":"#8c8c8c",
             "accent":"#2ea3f0","border":"#333333",
             "progress_fill":"#2ea3f0","slider_track":"#222222",
             "slider_handle":"#999999","row_even":"#141414",
             "row_odd":"#1c1c1c","btn_hover":"#282828",
             "focus_ring":"#2ea3f0"},
             "layout":{"eq_band_w_min":24,"text_meta":11,"text_time":14,"row_tint_alpha":0.2}}"##,
    )
    .unwrap();

    let themes = Themes::load_from(std::slice::from_ref(&dir));
    let t = themes.get("tok").expect("fixture theme loaded");
    assert_eq!(t.layout.eq_band_w_min, 24.0);
    assert_eq!(t.layout.text_meta, 11.0);
    assert_eq!(t.layout.text_time, 14.0);
    assert_eq!(t.layout.row_tint_alpha, 0.2);

    // Omitted tokens fall back to with_defaults.
    let t2 = themes.get("tok").unwrap();
    assert_eq!(t2.layout.with_defaults().eq_slider_min_h, 60.0);

    let _ = std::fs::remove_dir_all(&base);
}

/// The coordinator floors the EQ pane's width at 10 × eq_band_w_min; with the
/// default that floor must be below the 320px minimum window so the band row
/// never clips.
#[test]
fn default_eq_floor_is_below_minimum_window() {
    let layout = Layout::default().with_defaults();
    assert!(10.0 * layout.eq_band_w_min <= 320.0);
    assert_eq!(10.0 * layout.eq_band_w_min, 300.0);
}
