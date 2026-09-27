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
    // Its own directory, not the shared per-pid root: cargo runs a binary's
    // tests in parallel, and a `remove_dir_all` on the root deletes every other
    // test's fixture mid-run. Same fix as `settings_owners.rs`'s `theme_fixture`.
    let base = std::env::temp_dir()
        .join(format!("tplay-test-{}", std::process::id()))
        .join("layout-tokens");
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

/// The shape that made `icon_button`'s two trailing `bool`s unreadable must not
/// come back at a call site.
///
/// Every lit mode toggle read `true, app.shuffle()` — two bare `bool`s at the end
/// of a five-argument call, where a stray `true` and the state beside it are
/// indistinguishable and a swap is invisible. `icon_toggle` takes one, so a new
/// toggle has to reach for it.
///
/// What makes this worth a test is that the mistake is invisible in every other
/// way: it compiles, clippy is silent, and the button lights when the mode is
/// off. Only the call site can see it, which is the same reason
/// `the_layout_reads_live_inside_the_menu_closure` reads the source.
#[test]
fn no_icon_button_call_site_passes_two_bare_bools() {
    let root =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("src/gui");
    let mut offenders = Vec::new();
    let mut checked = 0;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                // Byte offsets are absolute, so a line number and a range check
                // both mean what they say. Walking a shrinking slice instead
                // silently compared positions from two different strings.
                let toggle_at = text.find("pub fn icon_toggle");
                // The search starts *past* the match, not at it: `text[s..]`
                // begins with `pub fn ` itself, so searching from there finds the
                // function we already found and yields an empty range.
                let toggle_end = toggle_at
                    .map(|s| {
                        text[s + "pub fn icon_toggle".len()..]
                            .find("pub fn ")
                            .map_or(text.len(), |n| s + "pub fn icon_toggle".len() + n)
                    })
                    .unwrap_or(0);
                let mut at = 0;
                while let Some(found) = text[at..].find("icon_button(") {
                    let call_at = at + found;
                    let open = call_at + "icon_button(".len();
                    let mut depth = 1;
                    let mut i = open;
                    let bytes = text.as_bytes();
                    while depth > 0 {
                        match bytes[i] {
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    let body = &text[open..i - 1];
                    at = i;
                    // The definition itself, and `icon_toggle`'s one forwarding
                    // call — which has the shape by definition, since that is the
                    // whole of what it does.
                    if body.contains("ui: &mut egui::Ui")
                        || toggle_at.is_some_and(|s| (s..toggle_end).contains(&call_at))
                    {
                        continue;
                    }
                    let line = text[..call_at].lines().count() + 1;
                    let args: Vec<&str> = body
                        .split(',')
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .collect();
                    checked += 1;
                    if args.len() == 6 && args[4] == "true" && args[5] != "false" {
                        offenders.push(format!(
                            "{}:{line} passes `true, {}` — use icon_toggle, which \
                             takes one bool",
                            path.display(),
                            args[5]
                        ));
                    }
                }
            }
        }
    }
    assert!(
        checked >= 15,
        "expected the whole gui tree, found {checked} calls"
    );
    assert!(
        offenders.is_empty(),
        "a lit toggle should read as one value:\n  {}",
        offenders.join("\n  ")
    );
}
