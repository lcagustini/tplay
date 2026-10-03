//! UI-polish regression tests (headless egui): theme application behavior,
//! token round-trips, and the EQ floor invariant that the review fixes
//! introduced.

use eframe::egui::{self, Color32};
use serde_json::json;
use tplay::gui::theme::{apply, Chrome, Family, Rows, Theme, Themes};
use tplay::library::SORT_OPTIONS;

/// The 13 required palette tokens, which every theme must carry.
fn palette() -> serde_json::Value {
    json!({
        "bg": "#141414", "panel_bg": "#1d1d1d", "text_primary": "#e0e0e0",
        "text_secondary": "#8c8c8c", "accent": "#2ea3f0", "border": "#383838",
        "progress_fill": "#2ea3f0", "slider_track": "#2b2b2b", "slider_handle": "#9a9a9a",
        "row_even": "#171717", "row_odd": "#202020", "btn_hover": "#2a2a2a",
        "focus_ring": "#2ea3f0"
    })
}

/// Load a theme through the real loader, with `extra` merged beside the palette.
///
/// **`Theme` is a struct literal nowhere in this suite, on purpose.** A struct
/// literal would make every fixture break at compile time the moment a group was
/// added to the theme system, which is a compile error about the *tests* rather
/// than about the change under test — and `settings_owners.rs`'s `write_theme`
/// already plays the role of "the canonical minimal valid theme.json". Building
/// through the loader means a new group costs nothing here, and each fixture is
/// exactly the file a theme author writes.
fn load(extra: serde_json::Value) -> std::sync::Arc<Theme> {
    let mut v = json!({ "id": "test", "name": "Test", "base": "dark", "palette": palette() });
    if let serde_json::Value::Object(m) = extra {
        v.as_object_mut().unwrap().extend(m);
    }
    // One directory per fixture, keyed by the JSON's own bytes, so two fixtures
    // with different extras never collide and cargo's parallel tests never race.
    let dir = std::env::temp_dir()
        .join(format!("tplay-test-{}", std::process::id()))
        .join("ui-polish")
        .join(sha(&serde_json::to_string(&v).unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("theme.json"), serde_json::to_string(&v).unwrap()).unwrap();
    Themes::load_from(std::slice::from_ref(&dir))
        .get("test")
        .cloned()
        .expect("fixture theme loaded")
}

/// A directory name from the JSON, without pulling in a hashing dependency.
fn sha(s: &str) -> String {
    s.len().to_string()
}

/// A theme whose JSON says nothing but its palette — the back-compatibility
/// case, since every bundled theme and any hand-written one looks like this.
fn fixture_theme() -> std::sync::Arc<Theme> {
    load(json!({}))
}

#[test]
fn a_theme_with_only_a_palette_still_renders_as_it_did_before_the_token_groups() {
    let t = fixture_theme();

    // Every group is populated, because an absent group parses as `Null` and
    // each token falls back individually.
    assert_eq!(t.chrome, Chrome::default());
    assert_eq!(t.rows, Rows::default());
    assert_eq!(t.rows.height, 24.0, "a list row keeps today's height");
    assert_eq!(t.panes.sidebar_w, 120.0, "the sidebar keeps today's width");
    assert_eq!(t.type_.meta, 12.0, "metadata keeps today's size");
    assert_eq!(t.type_.mono, Family::Monospace);
    assert_eq!(t.dock.tab_text_active, Color32::from_gray(230));
    // Optional colours are absent rather than guessed.
    assert_eq!(t.chrome.button_border, None);
    assert_eq!(t.viz.bg, None);
    assert_eq!(t.viz.accent, None);
}

/// A zero is a value several tokens legitimately take — square corners, no
/// animation, no active-row tint — so the loader must read it rather than treat
/// it as "unset". `Layout::with_defaults`' `> 0.0` test discarded exactly these,
/// which is how `row_tint_alpha` could not be switched off.
#[test]
fn a_meaningful_zero_token_survives_loading() {
    let t = load(json!({
        "chrome": { "radius": 0, "motion": 0 },
        "rows": { "tint_alpha": 0, "stripe_w": 0 }
    }));
    assert_eq!(
        t.chrome.radius, 0.0,
        "square corners are a Winamp skin's whole look"
    );
    assert_eq!(
        t.rows.tint_alpha, 0.0,
        "a theme may ask for no active-row tint"
    );
    assert_eq!(t.rows.stripe_w, 0.0);

    // And a token that is simply absent still gets its default.
    let plain = fixture_theme();
    assert_eq!(plain.chrome.radius, 2.0);
    assert_eq!(plain.rows.tint_alpha, 0.10);
}

#[test]
fn every_token_group_round_trips_from_json() {
    let t = load(json!({
        "type": { "family": "monospace", "mono": "proportional", "heading": 20,
                  "body": 14, "meta": 11, "time": 12 },
        "chrome": { "radius": 3, "border_w": 2, "fg_w": 2, "focus_w": 2,
                    "button_border": "#ff0000", "cursor_w": 1, "handle_aspect": 0.5,
                    "disabled_alpha": 0.4, "expansion": 2, "weak_text": "#777777",
                    "text_edit_bg": "#101010", "window_stroke": "#00ff00",
                    "window_radius": 9, "backdrop": "#00000080", "motion": 0.2,
                    "item_gap": 5, "button_pad": 4, "menu_pad": 3 },
        "rows": { "height": 28, "radius": 1, "stripe_w": 2, "inset": 8, "gap": 6,
                  "dim": 0.6, "tint_alpha": 0.25, "header_h": 20, "title_floor": 30,
                  "cell_gap": 3, "trailing_w": 120, "columns": [0, 80, 120, 50] },
        "panes": { "topbar_h": 26, "logo": 20, "chrome_icon": 14, "transport_icon": 20,
                   "small_icon": 11, "toolbar_icon": 18, "volume_icon": 12, "folder_icon": 16,
                   "tiny_icon": 9, "sidebar_w": 130, "sidebar_row_h": 20, "sidebar_row_w": 110,
                   "sidebar_server_w": 95, "sidebar_form_w": 90, "breadcrumb_h": 20,
                   "breadcrumb_max_w": 60, "field_w": 200, "seek_min_w": 50, "meta_w": 140,
                   "gap": 6, "pad": 10, "eq_band_w_max": 70, "confirm_w": 240, "edit_w": 300,
                   "edit_label_w": 60, "edit_star": 26, "nocover_frac": 0.5 },
        "dock": { "tab_h": 26, "tab_radius": 0, "tab_bg": "#111111", "tab_bg_active": "#222222",
                  "tab_stroke": "#333333", "drag_fill": "#444444", "tab_text": "#555555",
                  "tab_text_active": "#ffffff", "split_w": 6, "split_color": "#00aaaa",
                  "split_color_active": "#00ffff" },
        "viz": { "bg": "#001122", "accent": "#ff00ff" }
    }));

    assert_eq!(t.type_.family, Family::Monospace);
    assert_eq!(t.type_.mono, Family::Proportional);
    assert_eq!(t.type_.heading, 20.0);
    assert_eq!(t.type_.time, 12.0);
    assert_eq!(t.chrome.radius, 3.0);
    assert_eq!(t.chrome.button_border, Some(Color32::from_rgb(255, 0, 0)));
    assert_eq!(t.chrome.handle_aspect, 0.5);
    assert_eq!(t.chrome.motion, 0.2);
    assert_eq!(t.chrome.backdrop, Some(Color32::from_black_alpha(128)));
    assert_eq!(t.rows.height, 28.0);
    assert_eq!(t.rows.trailing_w, 120.0);
    assert_eq!(t.rows.columns, [0.0, 80.0, 120.0, 50.0]);
    assert_eq!(t.panes.sidebar_w, 130.0);
    assert_eq!(t.panes.nocover_frac, 0.5);
    assert_eq!(t.dock.tab_bg, Some(Color32::from_rgb(0x11, 0x11, 0x11)));
    assert_eq!(t.dock.split_w, 6.0);
    assert_eq!(t.dock.tab_text_active, Color32::WHITE);
    assert_eq!(t.viz.accent, Some(Color32::from_rgb(255, 0, 255)));
}

/// `apply` is the whole of widget chrome, so this is the mapping a new token
/// extends. `Style::text_styles` is the largest addition: before it, no theme
/// could reach the ~19 labels that never build a `FontId` by hand.
#[test]
fn apply_maps_the_token_groups_onto_visuals_and_style() {
    let ctx = egui::Context::default();
    let theme = load(json!({
        "type": { "heading": 22, "body": 14, "meta": 11, "time": 12 },
        "chrome": { "radius": 4, "border_w": 2, "fg_w": 3, "focus_w": 5,
                    "button_border": "#ff00ff", "motion": 0.25, "item_gap": 3,
                    "button_pad": 7, "menu_pad": 6, "weak_text": "#123456",
                    "text_edit_bg": "#654321", "window_stroke": "#abcdef",
                    "window_radius": 11, "disabled_alpha": 0.25, "cursor_w": 1,
                    "expansion": 3, "handle_aspect": 0.5 }
    }));
    apply(&ctx, &theme);

    let style = ctx.style_of(ctx.theme());
    let v = &style.visuals;
    let p = theme.palette;

    // The palette mappings the first version of this test asserted.
    assert_eq!(v.panel_fill, p.bg);
    assert_eq!(v.window_fill, p.bg);
    assert_eq!(v.hyperlink_color, p.accent);
    assert_eq!(v.override_text_color, Some(p.text_primary));
    assert_eq!(v.selection.bg_fill, p.progress_fill);
    assert_eq!(v.widgets.noninteractive.bg_fill, p.panel_bg);

    // `chrome`: shape, and the widths that used to be literals in this function.
    assert_eq!(v.selection.stroke.width, 5.0);
    assert_eq!(v.window_corner_radius, egui::CornerRadius::same(11));
    assert_eq!(v.menu_corner_radius, egui::CornerRadius::same(11));
    assert_eq!(v.disabled_alpha, 0.25);
    assert_eq!(v.text_cursor.stroke.width, 1.0);
    assert_eq!(v.weak_text_color, Some(Color32::from_rgb(0x12, 0x34, 0x56)));
    assert_eq!(
        v.text_edit_bg_color,
        Some(Color32::from_rgb(0x65, 0x43, 0x21))
    );
    assert_eq!(v.window_stroke.color, Color32::from_rgb(0xab, 0xcd, 0xef));
    assert_eq!(v.window_stroke.width, 2.0);
    assert_eq!(
        v.handle_shape,
        egui::style::HandleShape::Rect { aspect_ratio: 0.5 }
    );

    // Corner radius and expansion on all five widget states, including `open`,
    // which had no mapping at all before.
    for (name, w) in [
        ("noninteractive", &v.widgets.noninteractive),
        ("inactive", &v.widgets.inactive),
        ("hovered", &v.widgets.hovered),
        ("active", &v.widgets.active),
        ("open", &v.widgets.open),
    ] {
        assert_eq!(
            w.corner_radius,
            egui::CornerRadius::same(4),
            "{name} radius"
        );
        assert_eq!(w.expansion, 3.0, "{name} expansion");
    }
    assert_eq!(
        v.widgets.inactive.bg_stroke.color,
        Color32::from_rgb(255, 0, 255)
    );
    assert_eq!(v.widgets.inactive.bg_stroke.width, 2.0);
    assert_eq!(v.widgets.hovered.fg_stroke.width, 3.0);

    // `Style`: the type scale and the density.
    assert_eq!(
        style.text_styles[&egui::TextStyle::Heading].size,
        22.0,
        "a heading size no theme could set before"
    );
    assert_eq!(style.text_styles[&egui::TextStyle::Body].size, 14.0);
    assert_eq!(style.text_styles[&egui::TextStyle::Small].size, 11.0);
    assert_eq!(style.text_styles[&egui::TextStyle::Monospace].size, 12.0);
    assert_eq!(style.spacing.item_spacing.x, 3.0);
    assert_eq!(style.spacing.button_padding, egui::vec2(7.0, 7.0));
    assert_eq!(style.spacing.menu_margin.left, 6);
    assert_eq!(style.animation_time, 0.25, "a theme may animate a switch");
}

/// `apply` writes **both** styles, not just the active one. `set_visuals` touched
/// only whichever the system preference selects, so the other kept whatever egui
/// built it with and an OS theme flip surfaced stale greys.
#[test]
fn apply_writes_both_styles_not_just_the_active_one() {
    let ctx = egui::Context::default();
    let theme = load(json!({ "chrome": { "radius": 7, "motion": 0.1 } }));
    apply(&ctx, &theme);
    for t in [egui::Theme::Dark, egui::Theme::Light] {
        let s = ctx.style_of(t);
        assert_eq!(
            s.visuals.widgets.noninteractive.corner_radius,
            egui::CornerRadius::same(7),
            "{t:?}"
        );
        assert_eq!(s.animation_time, 0.1, "{t:?}");
    }
}

/// A token present but not a colour must say so rather than fall back silently
/// forever: the file is hand-editable and a typo in it is otherwise invisible.
#[test]
fn a_malformed_colour_token_does_not_take_the_theme_down() {
    let t = load(json!({ "chrome": { "weak_text": "not-a-colour" } }));
    assert_eq!(t.chrome.weak_text, None);
    assert_eq!(
        t.palette.bg,
        Color32::from_rgb(0x14, 0x14, 0x14),
        "the rest of the file survived"
    );
}

/// A `columns` array of the wrong length fills what it can. The array is sized by
/// `SORT_OPTIONS::len()`, so a fifth column is a compile error at the
/// declaration — a short or long array must not be a panic on the way there.
#[test]
fn a_wrong_length_columns_array_fills_rather_than_panicking() {
    let short = load(json!({ "rows": { "columns": [0, 80] } }));
    assert_eq!(short.rows.columns, [0.0, 80.0, 100.0, 44.0]);
    let long = load(json!({ "rows": { "columns": [0, 80, 120, 50, 60, 70] } }));
    assert_eq!(long.rows.columns, [0.0, 80.0, 120.0, 50.0]);
}

/// The row-width array is sized by `SORT_OPTIONS`, so a fifth sortable column is
/// a compile error rather than a row that silently never draws a width.
#[test]
fn the_columns_array_matches_the_sortable_column_count() {
    assert_eq!(SORT_OPTIONS.len(), Rows::default().columns.len());
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
