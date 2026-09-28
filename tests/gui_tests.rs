//! GUI tests — theme application, dock layout (headless egui).

mod common;

use eframe::egui::{self, Color32, FontFamily};
use std::path::Path;
use tplay::app::{Pane, VizView};
use tplay::audio::eq::EqShared;
use tplay::audio::viz::{VizBuf, VIZ_BANDS};
use tplay::gui::panes::visualizer::views::SHADER_VIEWS;
use tplay::gui::theme::{rasterize_icon, Base, Icon, Layout, Themes, DEFAULT_THEME_ID};

#[test]
fn themes_loads_builtin_dark_theme() {
    let themes = Themes::load();
    assert!(
        !themes.list().is_empty(),
        "at least dark theme should exist"
    );
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
            FontFamily::Monospace | FontFamily::Proportional => {}
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
    let panes = [
        Pane::NowPlaying,
        Pane::Playlist,
        Pane::Equalizer,
        Pane::Library,
    ];
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

// ── Sidebar width containment ─────────────────────────────────────────

/// The Library sidebar must be immune to egui's `TextEdit` overflow allocation.
///
/// `TextEdit` deliberately grows its parent's `min_rect` by the text overflow
/// ("allocate additional space … so a ScrollArea can properly scroll to the
/// cursor"). The sidebar is a vertical-only ScrollArea, whose width is the
/// content width, so that growth used to (1) widen the column and (2) keep
/// moving its scrollbar while typing. `clip_text` alone does NOT stop it — it
/// pins the field rect, but the overflow still grows the parent.
///
/// The fix is two distinct egui behaviors: `allocate_space` reserves a rect
/// *and* advances the layout cursor (a bare `new_child` leaves the next row
/// drawn on top), while a raw `new_child` does *not* propagate its min_rect
/// upward (which is what contains the overflow).
#[test]
fn sidebar_column_and_scroll_content_ignore_textedit_overflow() {
    const SIDEBAR: f32 = 120.0;
    const FORM_W: f32 = 100.0;
    const FIELD_H: f32 = 18.0;
    const LONG: &str = "smb://192.168.15.59/newhd/some/deeply/nested/folder/name";

    #[derive(Clone, Copy)]
    enum Shape {
        /// `ui.vertical` + `set_min_width`: the original width bug.
        Vertical,
        /// `new_child` without advancing the cursor: the sibling overlaps.
        ChildNoAdvance,
        /// Sidebar fixed, but the form NOT wrapped: content still grows.
        UnwrappedForm,
        /// The fix: sidebar and form both in fixed-rect children.
        Fixed,
    }

    /// Returns (sidebar width, file-list min.x, scroll content width).
    fn layout(shape: Shape, text: &str) -> (f32, f32, f32) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(680.0, 460.0),
            )),
            ..Default::default()
        };
        let out = std::cell::Cell::new((0.0f32, 0.0f32, 0.0f32));
        let content_w = std::cell::Cell::new(0.0f32);
        let ctx = egui::Context::default();
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    let mut text = text.to_string();

                    // The scroll content, including the fixed-rect form.
                    let mut sidebar_body = |ui: &mut egui::Ui, wrap_form: bool| {
                        let scroll_h = (ui.available_height() - 8.0).max(40.0);
                        let sa = egui::ScrollArea::vertical()
                            .id_salt("places_favorites")
                            .auto_shrink([true, false])
                            .max_height(scroll_h)
                            .show(ui, |ui| {
                                ui.label("Network");
                                let gap = ui.spacing().item_spacing.y;
                                let form_h = 3.0 * (FIELD_H + gap) + ui.spacing().interact_size.y;
                                if wrap_form {
                                    let (_, frect) = ui.allocate_space(egui::vec2(FORM_W, form_h));
                                    let mut f = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(frect)
                                            .layout(egui::Layout::top_down(egui::Align::Min)),
                                    );
                                    f.vertical(|ui| {
                                        for _ in 0..3 {
                                            ui.add_sized(
                                                egui::vec2(FORM_W, FIELD_H),
                                                egui::TextEdit::singleline(&mut text)
                                                    .clip_text(true)
                                                    .desired_width(FORM_W),
                                            );
                                        }
                                    });
                                } else {
                                    for _ in 0..3 {
                                        ui.add_sized(
                                            egui::vec2(FORM_W, FIELD_H),
                                            egui::TextEdit::singleline(&mut text)
                                                .clip_text(true)
                                                .desired_width(FORM_W),
                                        );
                                    }
                                }
                            });
                        content_w.set(sa.content_size.x);
                    };

                    // The match *is* the sidebar width: every arm lays the
                    // column out differently and yields the width it ended up
                    // with. Written as an expression rather than a `let mut`
                    // assigned per arm, which needed an `#[allow]` for a
                    // never-read `0.0` initializer.
                    let sidebar_w = match shape {
                        Shape::Vertical => {
                            let inner = ui.vertical(|ui| {
                                ui.set_min_width(SIDEBAR);
                                sidebar_body(ui, false);
                            });
                            inner.response.rect.width()
                        }
                        Shape::ChildNoAdvance => {
                            let rect = egui::Rect::from_min_size(
                                ui.min_rect().min,
                                egui::vec2(SIDEBAR, ui.available_height()),
                            );
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            sidebar_body(&mut child, true);
                            rect.width()
                        }
                        Shape::UnwrappedForm | Shape::Fixed => {
                            let (_, rect) =
                                ui.allocate_space(egui::vec2(SIDEBAR, ui.available_height()));
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            sidebar_body(&mut child, matches!(shape, Shape::Fixed));
                            rect.width()
                        }
                    };
                    let files = ui.vertical(|ui| {
                        ui.label("files");
                    });
                    out.set((sidebar_w, files.response.rect.min.x, content_w.get()));
                });
            });
        });
        out.get()
    }

    // Premise A: `ui.vertical` sizes the column by its min_rect, which the
    // overflow grows — the original bug.
    let (vertical_w, _, _) = layout(Shape::Vertical, LONG);
    assert!(
        vertical_w > SIDEBAR + 20.0,
        "premise: ui.vertical sidebar should widen with long text, got {vertical_w}"
    );

    // Premise B: a bare `new_child` never advances the horizontal cursor, so the
    // file-list sibling lands at the same x and draws over the sidebar.
    let (_, overlap_x, _) = layout(Shape::ChildNoAdvance, LONG);
    assert!(
        overlap_x < SIDEBAR,
        "premise: new_child without allocate_space should overlap, sibling x={overlap_x}"
    );

    // Premise C: fixing only the column leaves the scroll content growing —
    // which is what kept dragging the scrollbar.
    let (_, _, unwrapped_content) = layout(Shape::UnwrappedForm, LONG);
    assert!(
        unwrapped_content > FORM_W + 20.0,
        "premise: an unwrapped form should still widen the scroll content, got {unwrapped_content}"
    );

    // The fix: column pinned, sibling beside it, and scroll content invariant.
    for text in ["", "192.168.15.59", "smb://192.168.15.59/newhd/", LONG] {
        let (w, files_x, content) = layout(Shape::Fixed, text);
        assert!(
            (w - SIDEBAR).abs() < 0.5,
            "sidebar must stay {SIDEBAR}px for {text:?}, got {w}"
        );
        assert!(
            files_x >= SIDEBAR - 0.5,
            "file list must sit beside the sidebar, not on top: x={files_x}"
        );
        assert!(
            content <= FORM_W + 0.5,
            "scroll content must not widen with {text:?}, got {content}"
        );
    }
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
    // Its own subdirectory of the shared per-pid root, never the root itself:
    // cargo runs a binary's tests in parallel, so a `remove_dir_all` on the root
    // deletes every other test's fixture mid-run — which showed up as a
    // `write_wav` ENOENT in an unrelated pane test. Same fix, and for the same
    // reason, as `settings_owners.rs`'s `theme_fixture`.
    let base = common::test_dir("theme-priority");
    let overrides = base.join("overrides");
    let bundled = base.join("bundled");
    write_theme(&overrides, "dark", "#ff0000");
    write_theme(&bundled, "dark", "#00ff00"); // must lose to overrides
    write_theme(&bundled, "mine", "#0000ff"); // no icons dir
    std::fs::create_dir_all(overrides.join("dark").join("icons")).unwrap();
    std::fs::write(
        overrides.join("dark").join("icons").join("play.svg"),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 20\"/>",
    )
    .unwrap();

    // Priority order: first dir in the slice wins on id clash.
    let themes = Themes::load_from(&[overrides.clone(), bundled]);
    assert_eq!(themes.list().len(), 2);
    assert_eq!(
        themes.default().palette.accent,
        Color32::from_rgb(0xff, 0, 0)
    );
    // "mine" has no icons of its own → falls back to the default theme's.
    let mine = themes.get("mine").unwrap();
    assert!(themes
        .icon_path(mine, Icon::Play)
        .unwrap()
        .ends_with("dark/icons/play.svg"));
    assert!(themes.icon_path(mine, Icon::Volume).is_none()); // dark has no volume.svg

    let _ = std::fs::remove_dir_all(&base);
}
/// Regression: the Library pane's right-hand header row (counts + Add All)
/// pushed the whole file list below the pane.
///
/// `Ui::with_layout` with a **horizontal** layout whose cross-axis align is
/// `Center`/`Max` hands its child a `min_rect` spanning the parent's whole
/// remaining height instead of the height actually used, and `scope_dyn` ends
/// with `advance_cursor_after_rect(child.min_rect())` — so the parent cursor
/// jumps by that entire span and `available_height()` collapses to 0. Measured
/// in a 460px window: `Center` consumed 430px of a 444px column, `Min` consumed
/// the 21px it used.
///
/// This asserts the *geometry the user sees* (is the list still on screen, and
/// did the header row leave room for it), not the align token, because the
/// token is an implementation detail and the geometry is the bug.
#[test]
fn right_to_left_center_does_not_swallow_the_column() {
    use eframe::egui::{pos2, vec2, Rect, Sense};

    /// Build one frame of `CentralPanel > horizontal_top > [sidebar, column]`
    /// with the header row in `align`, and report (header height consumed,
    /// scroll content top, window height).
    fn measure(align: egui::Align) -> (f32, f32, f32) {
        const WIN_H: f32 = 460.0;
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(680.0, WIN_H))),
            ..Default::default()
        };
        let mut consumed = 0.0;
        let mut scroll_top = f32::NAN;
        // `Context::run` returns a `#[must_use]` FullOutput; this test only cares
        // about the side effects on the Cells below.
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // Sidebar, exactly as the pane allocates it.
                    let (_, side) = ui.allocate_space(vec2(120.0, ui.available_height()));
                    let mut su = ui.new_child(egui::UiBuilder::new().max_rect(side));
                    su.vertical(|ui| {
                        for i in 0..12 {
                            ui.label(format!("place {i}"));
                        }
                    });

                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label("breadcrumb");
                        });
                        let before = ui.min_rect().height();
                        ui.with_layout(egui::Layout::right_to_left(align), |ui| {
                            // Drawn for its layout effect only; no click needed.
                            let _ = ui.button("Add All");
                            ui.label("12 tracks · 3 folders · 1 playlist");
                        });
                        consumed = ui.min_rect().height() - before;
                        ui.horizontal(|ui| {
                            ui.label("Search");
                            ui.add(
                                egui::TextEdit::singleline(&mut String::new()).desired_width(220.0),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Title");
                            ui.label("Artist");
                        });
                        let scroll_h = (ui.available_height() - 24.0).max(40.0);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(scroll_h)
                            .show(ui, |ui| {
                                let (r, _) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 20.0),
                                    Sense::hover(),
                                );
                                scroll_top = r.min.y;
                            });
                    });
                });
            });
        });
        (consumed, scroll_top, WIN_H)
    }

    // Premise: this really is a trap, and it is the align token that decides.
    // Without this, the real assertions below could pass for the wrong reason.
    let (center_h, center_top, win_h) = measure(egui::Align::Center);
    assert!(
        center_h > 200.0,
        "premise: right_to_left(Center) is expected to swallow the column, consumed {center_h:.1}px"
    );
    assert!(
        center_top > win_h,
        "premise: with Center the list lands below the pane (top {center_top:.1} > {win_h:.1})"
    );

    // The fix: Min consumes one row's worth, leaving the list on screen.
    let (min_h, min_top, win_h) = measure(egui::Align::Min);
    assert!(
        min_h < 40.0,
        "Align::Min must consume only the row it uses, got {min_h:.1}px"
    );
    assert!(
        min_top < win_h,
        "the file list must be inside the pane, got top {min_top:.1} vs window {win_h:.1}"
    );
    assert!(
        min_h < center_h,
        "Align::Min must consume strictly less than Center ({min_h:.1} vs {center_h:.1})"
    );
}

/// The Playlist pane's search filters rows without touching the list, so the
/// only logic worth testing is the match itself. `row_matches` is `pub` and
/// `Ui`-free for exactly this.
mod playlist_search {
    use super::Path;
    use tplay::gui::panes::playlist::row_matches;
    use tplay::library::TrackInfo;

    fn tagged(title: &str, artist: &str, album: &str) -> TrackInfo {
        TrackInfo {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            ..Default::default()
        }
    }

    #[test]
    fn an_empty_query_matches_every_row() {
        // The unfiltered path, run for every row on every frame.
        assert!(row_matches(Path::new("/m/a.mp3"), None, ""));
        assert!(row_matches(
            Path::new("/m/a.mp3"),
            Some(&tagged("Song", "Band", "Album")),
            ""
        ));
    }

    #[test]
    fn the_match_is_case_insensitive_over_title_artist_and_album() {
        // The contract: the caller trims and lowercases the query once per frame
        // (the Library's filter does the same), so case folding is this function's
        // job on the haystack — a user typing "ANTI" must find "Anti".
        let info = tagged("Ne‐Yo", "Rihanna", "Anti");
        for q in ["ne‐", "rih", "anti"] {
            assert!(row_matches(Path::new("/m/x.mp3"), Some(&info), q), "{q}");
        }
        assert!(!row_matches(Path::new("/m/x.mp3"), Some(&info), "drake"));
    }

    #[test]
    fn an_untagged_row_is_found_by_its_filename() {
        // The premise for the whole feature on a fresh playlist: the tag scan has
        // not run, so the only text a row has is its name.
        assert!(row_matches(
            Path::new("/music/Blue Monday.wav"),
            None,
            "blue"
        ));
        // The extension is part of the filename, so a format query works too.
        assert!(row_matches(
            Path::new("/music/Blue Monday.wav"),
            None,
            ".wav"
        ));
        assert!(!row_matches(
            Path::new("/music/Blue Monday.wav"),
            None,
            "sunday"
        ));
    }
}

/// Every icon slot in every bundled theme really rasterizes.
///
/// `load_icons` swallows a missing or unparseable file (`fs::read(..).ok()?`) and
/// the pane falls back to a glyph, which on this font is a tofu box. Nothing
/// else would notice, so the assets are checked here instead.
///
/// This replaces both halves of what `themes/generate_icons.py --check` used to
/// cover, and is strictly stronger on the direction it kept. The old check asked
/// whether a *mark* had a slot; this asks whether every slot has a file that
/// parses **and** produces the right pixels. The reverse direction — an SVG with
/// no `Icon` slot — is the loop below, and is the one that would otherwise be
/// invisible forever.
///
/// The alpha assertions are the load-bearing part. A game-icons SVG that kept its
/// `<path d="M0 0h512v512H0z"/>` background rectangle rasterizes to a *fully
/// opaque* 20×20 black square, and a file whose paths were all stripped
/// rasterizes to a *fully transparent* one. Both load without error and both
/// render as a solid block, so "it parsed" is not a sufficient check.
#[test]
fn every_icon_has_a_parseable_svg_source() {
    let themes = Themes::load();
    for id in ["dark", "retro", "neon"] {
        let theme = themes.get(id).unwrap_or_else(|| themes.default());
        for icon in Icon::ALL {
            let path = themes
                .icon_path(theme, icon)
                .unwrap_or_else(|| panic!("{id}: no file for slot {}", icon.index()));
            let px = icon.px();
            let image = rasterize_icon(&path, px).unwrap_or_else(|| {
                panic!(
                    "{id}: slot {} failed to rasterize from {}",
                    icon.index(),
                    path.display()
                )
            });
            let label = format!("{id}/{}", icon.file_name());

            assert_eq!(
                image.size,
                [px as usize, px as usize],
                "{label}: wrong size"
            );

            let alpha = |i: usize| image.pixels[i].a();
            let opaque = (0..image.pixels.len()).any(|i| alpha(i) > 200);
            let clear = (0..image.pixels.len()).any(|i| alpha(i) < 40);
            assert!(
                opaque,
                "{label}: nothing opaque — empty or all-transparent SVG"
            );
            assert!(
                clear,
                "{label}: fully opaque — the SVG still has a background rect"
            );
        }
    }
}

/// The reverse direction `every_icon_has_a_parseable_svg_source` cannot see: an
/// icon file with no `Icon` slot behind it. Nothing in the app reads it, so it
/// would sit in the tree forever looking maintained.
#[test]
fn no_icon_file_is_without_a_slot() {
    let slots: std::collections::HashSet<&str> = Icon::ALL.iter().map(|i| i.file_name()).collect();
    for id in ["dark", "retro", "neon"] {
        let dir = std::path::Path::new("themes").join(id).join("icons");
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                path.extension().is_some_and(|e| e == "svg"),
                "{id}: {name} is not an svg"
            );
            assert!(
                slots.contains(name.as_str()),
                "{id}/{name} has no `Icon` slot — add the variant to `Icon` AND a row to \
                 `ALL` AND a row to `DATA` in src/gui/theme.rs, all three, at the end"
            );
        }
    }
}

/// The one pane test that runs the real function instead of mirroring its
/// shape. The Playlist pane gives its ScrollArea everything `available_height()`
/// has left *after* the search row and the action row, and a `with_layout` under a
/// vertical parent can report a min_rect spanning the parent's whole remaining
/// height — the `right_to_left_center_does_not_swallow_the_column` trap, which
/// silently collapses the list to the `.max(40.0)` floor instead of erroring.
///
/// Measuring the pane's own `min_rect` catches that: a pane whose chrome behaved
/// claims the height it was given, and one whose chrome swallowed the column
/// comes back short by however much it ate.
///
/// Run at both window sizes because the action row **wraps** — the five controls
/// fit the default width on one line and the 320px minimum on two, so the small
/// window is the only case that exercises the wrap at all.
#[test]
fn the_playlist_pane_leaves_its_list_the_height_it_was_given() {
    use common::{test_dir, write_wav, TestApp};
    use eframe::egui::{pos2, vec2, Rect};
    use tplay::gui::panes::playlist::playlist_pane;
    use tplay::gui::theme::ThemeState;

    // The default window, then the minimum one (`main.rs`'s window options).
    for (win_w, win_h) in [(680.0f32, 460.0f32), (320.0, 160.0)] {
        let mut t = TestApp::new("playlist-pane-geometry");
        let dir = test_dir("playlist-pane-geometry");
        let mut tracks = Vec::new();
        for n in ["a.wav", "b.wav", "c.wav", "d.wav", "e.wav"] {
            let p = dir.join(n);
            write_wav(&p);
            tracks.push(p);
        }
        t.app.add_files(tracks);

        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(win_w, win_h))),
            ..Default::default()
        };
        let mut claimed = 0.0f32;
        let _ = ctx.run(raw, |ctx| {
            let themes = ThemeState::load(ctx, tplay::gui::theme::Themes::load(), "dark");
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                    playlist_pane(&mut t.app, &themes, ui);
                    claimed = ui.min_rect().height();
                });
            });
        });

        // 32px of slack is the CentralPanel's own margin plus the trailing
        // `add_space`, not a fudge: a working pane claims the rect it was given.
        assert!(
            claimed >= win_h - 32.0,
            "at {win_w}x{win_h} the pane must fill the window — a short claim means the \
             chrome ate the column and the list is on the 40px floor (claimed \
             {claimed:.1} of {win_h:.1})"
        );
        assert!(
            claimed <= win_h + 1.0,
            "at {win_w}x{win_h} the pane must not overflow its own rect either (claimed \
             {claimed:.1} of {win_h:.1})"
        );
    }
}

/// The Library pane draws one breadcrumb for both sources, so the two segment
/// builders plus `plan` (the `…` rule) are the only source-specific logic in
/// it. Both are pure, so none of this needs an `egui::Ui`.
mod breadcrumb {
    use super::Path;
    use tplay::gui::panes::library::header::{local_segs, plan, remote_segs, Action, Step};
    use tplay::network::NetworkBrowse;

    /// The two `always` values the pane passes, named.
    const LOCAL: usize = 1;
    const SHARE: usize = 3;

    fn browse(share: Option<&str>, rel: &str) -> NetworkBrowse {
        NetworkBrowse {
            host: "nas".into(),
            share: share.map(str::to_string),
            rel: rel.into(),
            ..Default::default()
        }
    }

    fn labels(segs: &[tplay::gui::panes::library::header::Seg]) -> Vec<&str> {
        segs.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn plan_collapses_the_middle_and_keeps_the_way_home() {
        // Local: only the filesystem root is exempt, so a 6-deep path shows the
        // root, one "…", and the last two levels.
        assert_eq!(
            plan(6, LOCAL),
            vec![Step::Seg(0), Step::Ellipsis, Step::Seg(4), Step::Seg(5)]
        );
        // A shallow path is never collapsed — every level is one click away.
        assert_eq!(
            plan(3, LOCAL),
            vec![Step::Seg(0), Step::Seg(1), Step::Seg(2)]
        );
        // Premise for the share case below: the local rule really would strand
        // a user, because it collapses the host and the share.
        assert_eq!(
            plan(7, LOCAL),
            vec![Step::Seg(0), Step::Ellipsis, Step::Seg(5), Step::Seg(6)],
            "local rule over a 7-segment share path"
        );
        // A share exempts Local / host / share, so a deep directory still has a
        // route to the share root — with no `..` row, that is the only way out.
        assert_eq!(
            plan(7, SHARE),
            vec![
                Step::Seg(0),
                Step::Seg(1),
                Step::Seg(2),
                Step::Ellipsis,
                Step::Seg(5),
                Step::Seg(6)
            ]
        );
        // The share-list stage is 2 segments — under `always`, so the "Local"
        // exit can never collapse away, and no "…" appears.
        assert_eq!(plan(2, SHARE), vec![Step::Seg(0), Step::Seg(1)]);
        // Degenerate: nothing to draw, and never a "…" for an empty list.
        assert!(plan(0, LOCAL).is_empty());
    }

    #[test]
    fn local_segs_walk_root_to_current() {
        let segs = local_segs(Path::new("/home/u/Music/Rock"));
        // Root's label is "/" rather than blank, so the first segment is never
        // an empty clickable box.
        assert_eq!(labels(&segs), ["/", "home", "u", "Music", "Rock"]);
        assert!(matches!(segs[0].action, Action::GoTo(_)));
        // Every segment hovers its full path: a capped segment can truncate to
        // nothing, so the tooltip is the only way to see where it points.
        assert_eq!(segs[3].hover.as_deref(), Some("/home/u/Music"));
        // The last segment is where we already are — drawn strong, not clicked.
        assert!(matches!(&segs[4].action, Action::GoTo(p) if p == Path::new("/home/u/Music/Rock")));
    }

    #[test]
    fn remote_segs_carry_a_full_uri_not_a_bare_name() {
        let segs = remote_segs(&browse(Some("music"), "Rock/Album"));
        assert_eq!(labels(&segs), ["Local", "nas", "music", "Rock", "Album"]);
        // The first three are the way *out* of the share, not into it.
        assert!(matches!(segs[0].action, Action::LeaveNetwork));
        assert!(matches!(&segs[1].action, Action::BrowseServer(h) if h == "nas"));
        // Every share/dir segment carries the walk *to* itself (cumulative, not
        // the segment's own name) and the share root carries an empty one.
        // Re-joining these instead of splitting is what produced a
        // PATH_NOT_FOUND (see `nav_uri_round_trips_but_renaming_one_does_not`).
        for (i, expect_rel) in ["", "Rock", "Rock/Album"].iter().enumerate() {
            match &segs[2 + i].action {
                Action::BrowseOpen { uri, share, rel } => {
                    assert_eq!(share, "music");
                    assert_eq!(rel, expect_rel);
                    assert!(
                        uri.starts_with("smb://nas/music"),
                        "segment {i} uri must be a full child URI, got {uri}"
                    );
                }
                other => panic!("segment {i} should be BrowseOpen, got {other:?}"),
            }
        }
        // At the share-list stage there is no share yet: Local + host, and the
        // host goes back to the share listing rather than into a share.
        let listing = remote_segs(&browse(None, ""));
        assert_eq!(labels(&listing), ["Local", "nas"]);
        assert!(matches!(listing[1].action, Action::BrowseServer(_)));
    }

    #[test]
    fn a_share_root_has_no_empty_trailing_crumb() {
        // `rel` is "" at the share root, and `"".split('/')` yields one empty
        // item. Unfiltered, that became a fourth segment labelled "" which
        // `plan` then drew as the bold "you are here" crumb — a blank box at the
        // end of the breadcrumb, at the one level users sit at most.
        let root = remote_segs(&browse(Some("music"), ""));
        assert_eq!(labels(&root), ["Local", "nas", "music"]);
        // Nothing to collapse at three segments, and the share is the way out.
        assert_eq!(
            plan(root.len(), SHARE),
            vec![Step::Seg(0), Step::Seg(1), Step::Seg(2)]
        );
        // One level down is exactly one more crumb, and the share root stays
        // reachable from it (the `always = 3` exemption).
        let deeper = remote_segs(&browse(Some("music"), "Rock"));
        assert_eq!(labels(&deeper), ["Local", "nas", "music", "Rock"]);
        assert_eq!(
            plan(deeper.len(), SHARE),
            vec![Step::Seg(0), Step::Seg(1), Step::Seg(2), Step::Seg(3)]
        );
    }
}

/// The two views that fill a non-convex area, and the one property egui's
/// tessellator needs of them.
///
/// `epaint::tessellator::fill_closed_path` triangulates a closed path as a fan
/// from its **first point**, so a polygon with a reflex corner is not drawn
/// badly — it is drawn as a wedge of triangles spanning the whole shape. Radial
/// handed it a ring sector (convexity fails because the middle is cut out) and
/// Flame a ridgeline mass (fails wherever the contour dips). Both filled as
/// garbage, and the code said so in comments: "a simple (non-self-intersecting)
/// polygon, so egui's tessellator fills it correctly". Simple is not convex, and
/// the claim was the bug.
///
/// So this is the guard: what they hand the tessellator now is convex. Nothing
/// else about a fill is observable without a window.
mod view_fills {
    use super::egui;
    use tplay::gui::panes::visualizer::views::{flame, radial};

    const BANDS: usize = 32;

    fn pane() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))
    }

    /// Weak convexity: every corner turns the same way. A zero-area piece (a
    /// Flame step sitting on the floor) turns zero times and is still fine, so
    /// the check is "no sign disagreement", not "strictly convex".
    fn assert_convex(quad: &[egui::Pos2; 4], label: &str) {
        let turns: Vec<f32> = (0..4)
            .map(|i| {
                let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
                (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x)
            })
            .collect();
        let left = turns.iter().filter(|&&t| t < 0.0).count();
        let right = turns.iter().filter(|&&t| t > 0.0).count();
        assert!(
            left == 0 || right == 0,
            "{label}: a reflex corner in {quad:?} — turns {turns:?}"
        );
    }

    #[test]
    fn every_filled_piece_is_convex() {
        for &level in &[0.0, 0.15, 0.5, 0.85, 1.0] {
            let levels = vec![level; BANDS];
            for quad in radial::fill_quads(pane(), &levels) {
                assert_convex(&quad, &format!("radial @ {level}"));
            }
            for quad in flame::fill_quads(pane(), &levels) {
                assert_convex(&quad, &format!("flame @ {level}"));
            }
        }
    }

    /// A flat spectrum is the easy case — one trapezoid per view per level. This
    /// is the one that broke: a contour that alternates full and empty, so
    /// every step is a different height and the whole mass is a sawtooth.
    #[test]
    fn a_sawtooth_contour_fills_convex_pieces() {
        let sawtooth: Vec<f32> = (0..BANDS)
            .map(|i| if i % 2 == 0 { 0.95 } else { 0.05 })
            .collect();
        let quads = flame::fill_quads(pane(), &sawtooth);
        assert_eq!(quads.len(), BANDS - 1);
        for quad in &quads {
            assert_convex(quad, "flame sawtooth");
        }
    }

    /// The quad list is the drawing, so it has to have the right *count* too:
    /// one piece per arc step per band, one per contour step.
    #[test]
    fn the_decomposition_has_one_piece_per_step() {
        let levels = vec![0.5; BANDS];
        assert_eq!(
            radial::fill_quads(pane(), &levels).len(),
            BANDS * radial::SEGMENTS,
            "radial: one quad per arc step per band"
        );
        assert_eq!(
            flame::fill_quads(pane(), &levels).len(),
            BANDS - 1,
            "flame: one trapezoid per contour step"
        );
        // A one-point contour cannot close, so it has no steps and no pieces —
        // the guard against indexing it.
        assert!(flame::fill_quads(pane(), &[0.5]).is_empty());
        assert!(flame::fill_quads(pane(), &[]).is_empty());
    }

    /// Convex and the right count still leaves "the pieces are somewhere else"
    /// open, which is a different wrong shape rather than a different wrong
    /// tessellation. Both views paint `rect` and nothing else.
    #[test]
    fn the_pieces_stay_inside_the_pane() {
        let rect = pane();
        let levels = vec![1.0; BANDS];
        for (label, quads) in [
            ("radial", radial::fill_quads(rect, &levels)),
            ("flame", flame::fill_quads(rect, &levels)),
        ] {
            for quad in &quads {
                for p in quad {
                    assert!(
                        p.distance(rect.center()) <= rect.width().max(rect.height()),
                        "{label}: {p:?} is outside the pane"
                    );
                }
            }
        }
    }
}

/// The rule the two fill regressions both turned on, asserted at the level where
/// it can be seen at all.
///
/// `fill_closed_path` is a triangle fan from the first point, and it insets the
/// fill by half of its 1px feathering — so convexity *and* one `Shape::Path` per
/// filled area are both preconditions, and a multi-shape tiling leaves a visible
/// gap along every shared edge whether or not the pieces are convex. Neither
/// property is measurable from a headless context, and the wrong version of the
/// rule is what two views' comments asserted, so it is pinned in the source: no
/// view may build a **filled closed path** at all. Filled areas go through
/// `fill_quads` into one `epaint::Mesh`.
///
/// Per-*mark* primitives are deliberately not banned. `rect_filled` for a bar
/// column or a spectrogram cell, and `line_segment` for a graticule, are each
/// their own area with no shared edge to gap against; only tiling **one** area
/// from several shapes is what feathering breaks.
///
/// Same shape as `no_inline_tests.rs` and `the_layout_reads_live_inside_the_menu_closure`:
/// the defect is a placement, and only the call site can show it.
#[test]
fn no_view_fills_a_closed_path() {
    let dir = std::path::Path::new("src/gui/panes/visualizer/views");
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        // Strip comment lines, so a view may *explain* the rule it follows.
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in ["convex_polygon", "Shape::Path", "PathShape"] {
            assert!(
                !code.contains(banned),
                "{} builds a filled closed path (`{banned}`) — that fill is a triangle \
                 fan from its first point, and it is inset by half its 1px feathering, so \
                 a non-convex one spills and adjacent ones leave a gap. Use `fill_quads` \
                 + one `epaint::Mesh`.",
                path.display()
            );
        }
    }
}

/// The shader-drawn views: the four properties that hold them together, and the
/// three ways a new one could quietly break.
///
/// A shader view's whole interface is a `draw` fn and a shader string, identical
/// in shape to a CPU view's `draw`. That is what makes adding one the same six
/// steps as adding a CPU view — but it is also a property of *convention*, so
/// nothing but a test holds it: a view that reached around the harness, or
/// painted its own background, or named a colour, would compile, pass every
/// behavioural test, and look correct in every other respect.
///
/// The colour rule is the one with a real user-visible failure. A hardcoded hex
/// ignores `theme.json`, so that view is the only one that does not follow a
/// mid-session theme switch — and nothing else in the app can see that, which is
/// exactly why `contrast_tests.rs` and `recolor_icons.py --check` both exist as
/// separate gates for the same reason.
mod shader_views {
    use super::*;
    use tplay::gui::panes::visualizer::gpu;
    use tplay::gui::theme::Palette;

    fn src(relative: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A pane-sized rect and a real `VizBuf`, so `draw` runs its whole CPU half.
    fn pane() -> (egui::Rect, VizBuf) {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let viz = VizBuf::new();
        // A window rather than silence: a view that reads the buffer should see
        // a full-scale tone, so a smoothing key left unset is distinguishable
        // from one that is working.
        for _ in 0..4096 {
            viz.push(0.5);
        }
        (rect, viz)
    }

    type ShaderDraw = fn(&egui::Painter, egui::Rect, &VizBuf, &Palette);

    /// Run one shader view's real `draw` in a headless `Context` and hand back
    /// the primitives it produced.
    ///
    /// The callback body never runs — there is no GL context here — and that is
    /// the point. Everything a view does on the CPU (the band smoothing, the
    /// uniform packing, the single queued callback and the rect it names) all
    /// happens in `draw`, so the whole testable half of a shader view is
    /// reachable without a GPU. The half that is not reachable is the shader
    /// itself, and no amount of headless testing changes that.
    fn primitives(
        draw: Option<ShaderDraw>,
        viz: &VizBuf,
        rect: egui::Rect,
    ) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let palette = Themes::load()
            .get(DEFAULT_THEME_ID)
            .expect("the default theme is always present")
            .palette;
        ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(draw) = draw {
                    draw(ui.painter(), rect, viz, &palette);
                }
            });
        })
        .shapes
    }

    fn callbacks(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::PaintCallback> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Callback(cb) => Some(cb),
                _ => None,
            })
            .collect()
    }

    /// A view contributes exactly one callback, and it covers exactly the pane.
    ///
    /// One is what makes this cheap: the callback runs inside a paint, so a
    /// hundred of them would be a hundred fullscreen draws. Covering the pane is
    /// what makes it *visible* — a callback whose rect is the panel rather than
    /// the drawing area paints over the header, and a zero-sized one paints
    /// nothing at all, and neither shows up anywhere but here.
    #[test]
    fn every_shader_view_asks_for_exactly_one_callback() {
        assert!(
            !SHADER_VIEWS.is_empty(),
            "the table is empty, so this sweep would pass vacuously"
        );
        let (rect, viz) = pane();
        for view in SHADER_VIEWS {
            let shapes = primitives(Some(view.draw), &viz, rect);
            let cbs = callbacks(&shapes);
            assert_eq!(
                cbs.len(),
                1,
                "{}: a shader view must queue exactly one callback — the callback body \
                 runs inside the paint, so more than one is more than one fullscreen \
                 draw (got {} callbacks in {} primitives)",
                view.name,
                cbs.len(),
                shapes.len()
            );
            assert_eq!(
                cbs[0].rect, rect,
                "{}: the callback must cover the pane rect it was handed, or it paints \
                 over the header or nothing at all",
                view.name
            );
        }
    }

    /// Every shader reads the palette, so a theme switch reaches it.
    ///
    /// The complement to the hex check above, and the one that carries the weight:
    /// a hardcoded *literal* is a colour someone typed, but a shader that simply
    /// never mentions a palette uniform ignores the theme just as thoroughly and
    /// cannot be caught by pattern-matching literals at all. This is the property
    /// that says "palette-only" rather than "no hex".
    #[test]
    fn every_shader_reads_the_palette() {
        const PALETTE_UNIFORMS: [&str; 3] = ["u_bg", "u_accent", "u_progress_fill"];
        for (name, body) in bodies() {
            assert!(
                PALETTE_UNIFORMS.iter().any(|u| body.contains(u)),
                "{name}: no palette uniform is referenced, so this view cannot follow a \
                 mid-session theme switch — every CPU view paints from `palette`, and a \
                 shader is the one place a colour could be a literal instead"
            );
        }
    }

    /// A view adds nothing of its own beyond the callback.
    ///
    /// Counted against an empty run of the same panel, because `CentralPanel`
    /// emits a background of its own and that is not the view's doing. What this
    /// rules out is a view painting anything directly — a second background
    /// fill, a border, a placeholder for "no GL here" — which would either cover
    /// the shader or stand in for it, and in both cases the shader view would
    /// still look deliberate.
    #[test]
    fn a_shader_view_queues_a_callback_and_nothing_else() {
        let (rect, viz) = pane();
        let baseline = primitives(None, &viz, rect).len();
        for view in SHADER_VIEWS {
            let shapes = primitives(Some(view.draw), &viz, rect);
            assert_eq!(
                shapes.len(),
                baseline + 1,
                "{}: a shader view must add the callback and nothing else — {} shapes \
                 beyond the panel's own {} means it painted something itself",
                view.name,
                shapes.len() - baseline,
                baseline
            );
        }
    }

    /// No shader hardcodes a colour.
    ///
    /// Every CPU view paints from `palette`, so a mid-session theme switch moves
    /// all of them at once (`theme::apply` runs every frame). A hex literal in a
    /// shader is the one view that does not follow — and nothing else in the app
    /// can see it, which is why this is a separate gate for the same reason
    /// `contrast_tests.rs` and `recolor_icons.py --check` are.
    ///
    /// No shader hardcodes a colour.
    ///
    /// Every CPU view paints from `palette`, so a mid-session theme switch moves
    /// all of them at once (`theme::apply` runs every frame). A literal in a
    /// shader is the one view that does not follow — and nothing else in the app
    /// can see it, which is why this is a separate gate for the same reason
    /// `contrast_tests.rs` and `recolor_icons.py --check` are.
    ///
    /// The patterns are the two unambiguous spellings, `#rrggbb` and `rgb(...)`.
    /// A bare `vec3(0.2, 0.4, 0.9)` is deliberately **not** matched, and the
    /// reason is that it cannot be: the eight lattice corners of a value-noise
    /// function are written `vec3(0.0,0.0,0.0)` through `vec3(1.0,1.0,1.0)`, and
    /// no textual rule separates those from a colour. Guessing here would either
    /// flag every noise function or miss every dark colour, so the honest
    /// position is that this catches the obvious spellings and a hand-written
    /// `vec3` colour is caught by review and by `contrast_tests.rs`.
    #[test]
    fn no_shader_hardcodes_a_colour() {
        for (name, body) in bodies() {
            for (n, line) in body.lines().enumerate() {
                let code = line.trim_start().trim_start_matches("//");
                for (what, hit) in [
                    ("a hex literal", hex_in(code)),
                    ("an rgb() literal", code.contains("rgb(")),
                ] {
                    assert!(
                        !hit,
                        "{}:{} has {what} — `{line}`\nA shader colour must come from the \
                         palette uniform, or this view ignores a mid-session theme switch.",
                        name,
                        n + 1
                    );
                }
            }
        }
    }

    /// Every GLSL body the app ships, as `(view name, source)`.
    ///
    /// Flattened from the table's `frags` rather than reading one `frag` per view:
    /// `Trails` runs two programs, and a sweep that only saw the first would leave
    /// the second — which is half of what that view draws — unchecked.
    fn bodies() -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        for view in SHADER_VIEWS {
            for body in view.frags {
                out.push((view.name, *body));
            }
        }
        out
    }

    /// `#abc` or `#aabbcc`, and not a `#` in a preprocessor line or a comment.
    fn hex_in(line: &str) -> bool {
        if line.trim_start().starts_with('#') && !line.contains(' ') {
            return false; // `#version`, `#define`
        }
        let bytes = line.as_bytes();
        bytes.iter().enumerate().any(|(at, &b)| {
            b == b'#'
                && bytes
                    .get(at + 1..at + 7)
                    .is_some_and(|rest| rest.iter().all(u8::is_ascii_hexdigit))
                && bytes.get(at + 7).is_none_or(|c| !c.is_ascii_hexdigit())
        })
    }

    /// The harness names no view.
    ///
    /// "A new visualization never means editing the harness" is the load-bearing
    /// claim of the whole design, and it is invisible in every other way: a
    /// harness that grew one view's uniform or special-cased one view's shader
    /// would compile, pass every behavioural test, and render correctly. So the
    /// only thing that can see it is the harness's own source.
    ///
    /// The `SHADER_VIEWS`/`Uniforms` names are the permitted vocabulary — the
    /// harness is *supposed* to know what a shader view is. What it may not do
    /// is name one.
    #[test]
    fn the_gl_harness_never_names_a_view() {
        let text = src("src/gui/panes/visualizer/gpu.rs");
        // Strip comment lines, so the module docs may discuss views by name.
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for name in VizView::ALL.iter().map(|v| v.name()) {
            assert!(
                !code.contains(name),
                "gpu.rs names the view `{name}` — the harness is handed a shader and a \
                 rect and must not know which look it is drawing. A view-specific \
                 uniform or special case here is what this test exists to stop."
            );
        }
    }

    /// Every shader view is reachable, and the table has nothing unreachable.
    ///
    /// Three ways to get this wrong, all silent: a table row whose `draw` is never
    /// called (the view exists and is invisible), a `VizView` variant that draws
    /// nothing, and a name in one list that is not in the other.
    #[test]
    fn the_shader_table_and_the_view_match_agree() {
        let names: Vec<&str> = VizView::ALL.iter().map(|v| v.name()).collect();
        let table: Vec<&str> = SHADER_VIEWS.iter().map(|v| v.name).collect();
        for name in &table {
            assert!(
                names.contains(name),
                "SHADER_VIEWS has `{name}`, which is not a VizView name — the dispatch \
                 looks the row up by `VizView::name()`, so this row is unreachable"
            );
        }
        // The match arm that dispatches them. Not a name check: a shader view is
        // reached through the table, so no view name appears in the pane at all.
        // What has to be there is the fall-through arm, and it is the *only* thing
        // that can route a table row — a match with seven direct calls and no
        // fall-through compiles, and every shader view renders nothing.
        let pane = src("src/gui/panes/visualizer.rs");
        let code: String = pane
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("SHADER_VIEWS"),
            "visualizer.rs has no arm dispatching through SHADER_VIEWS, so every shader \
             view in the table is unreachable — the match is exhaustive over the seven \
             CPU views and silently draws nothing for the rest"
        );
    }

    /// Every view has its own egui-memory key.
    ///
    /// A smoothing buffer shared between two views means switching from one to
    /// the other starts the new view off holding the old view's history, and
    /// switching back finds a buffer neither of them fully owns. Nothing shows
    /// that: both views animate, and the artifact is one frame of the wrong
    /// values at the moment of the switch.
    #[test]
    fn every_view_keys_its_own_memory() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gui/panes/visualizer/views");
        let mut seen: Vec<(String, String)> = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let owner = path.file_stem().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap();
            // Only the quoted form counts: an unquoted mention is prose in a
            // comment, and a key is always a string literal. The match text is
            // the prefix *including* its opening quote, so the key is read from
            // just past it — taking the match itself would yield the same
            // truncated string for every key in the tree.
            const PREFIX: &str = "\"tplay.viz.";
            for (at, _) in text.match_indices(PREFIX) {
                let key: String = text[at + PREFIX.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
                    .collect();
                assert!(
                    !seen.iter().any(|(k, _)| *k == key),
                    "egui-memory key `{key}` is claimed by two views ({} and {owner}) — \
                     each view owns its own, or switching between them carries one's \
                     state into the other",
                    seen.iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, o)| o.as_str())
                        .unwrap_or("?")
                );
                seen.push((key, owner.clone()));
            }
        }
        assert!(
            !seen.is_empty(),
            "no view declares an egui-memory key, so this sweep is checking nothing"
        );
    }

    /// A view that reads a superset uniform is reading what the view packed.
    ///
    /// The general form of "does `u_modes` reach the GPU", and the reason a
    /// per-view test would be a maintenance tax: `u_modes` is only meaningful to
    /// a view whose shader uses it, and a shader that silently stopped using it
    /// would pass a test written against the *old* shader. Matching the shader's
    /// own reference against the view's packing means the test is about the
    /// contract rather than about one view's current source.
    ///
    /// `u_prev` is excluded on purpose: it is a sampler and the harness owns it,
    /// and no shipped view declares one.
    #[test]
    fn a_superset_uniform_reaches_a_view_that_uses_it() {
        for view in SHADER_VIEWS {
            if !view.frags.iter().any(|f| f.contains("u_modes")) {
                continue;
            }
            let text = src(&format!(
                "src/gui/panes/visualizer/views/{}.rs",
                view.name.to_lowercase().replace(' ', "")
            ));
            assert!(
                text.contains("uniforms.modes"),
                "{}: its shader reads `u_modes`, so it must pack the mode pair into them",
                view.name
            );
        }
    }

    /// A framebuffer target is never zero-sized, and always covers the pane.
    ///
    /// Both halves are silent failures. `texImage2D` and `glFramebufferTexture2D`
    /// both reject a zero dimension with `INVALID_VALUE`, and a driver that
    /// reports an error on a path nobody checks leaves nothing in any log — the
    /// view is simply blank. And a *smaller* target is not an error at all: the
    /// sampler stretches it, so the feedback view quietly renders at the wrong
    /// resolution instead of failing.
    ///
    /// The rounding is asserted as "at least the pane", not "equal to the pane",
    /// because `ceil` is the right direction: a target one pixel short is
    /// stretched, and one pixel over costs nothing.
    #[test]
    fn fbo_size_covers_the_pane_and_is_never_zero() {
        // Zero, negative and fractional sizes: a pane really does measure zero on
        // one side for a frame while a splitter is dragged, and that is the case
        // this exists for.
        for size in [
            (0.0, 300.0),
            (400.0, 0.0),
            (0.0, 0.0),
            (-4.0, 300.0),
            (400.0, -1.0),
        ] {
            let (w, h) = gpu::fbo_size(size);
            assert!(
                w >= 1 && h >= 1,
                "fbo_size({size:?}) = ({w}, {h}) — a zero dimension is INVALID_VALUE \
                 and the view is blank with nothing in the log"
            );
        }
        // A fractional pane, which a HiDPI display always is.
        let (w, h) = gpu::fbo_size((600.4, 300.2));
        assert!(w >= 601 && h >= 301, "({w}, {h}) must round up, never down");
        // Exactly integral still covers.
        assert_eq!(gpu::fbo_size((400.0, 300.0)), (400, 300));
    }

    /// A feedback view's decay is a function of elapsed time, not of frames.
    ///
    /// This is the whole reason `feedback_for_a_dt` is a function at all. A
    /// per-frame multiplier is a fixed fraction per frame, so a trail computed as
    /// `frame * 0.95` is 20 frames of history at 60 Hz and 40 at 30 Hz — twice as
    /// long — and nothing about the result looks wrong. A screenshot cannot see
    /// it, a frame counter cannot see it, and it is the only thing that decides
    /// whether the trail means the same duration on any machine.
    ///
    /// Asserted as a *product over a fixed wall-clock duration* rather than as a
    /// per-frame comparison, because that is the property: the same number of
    /// milliseconds of decay must leave the same amount behind.
    #[test]
    fn feedback_for_a_dt_is_frame_rate_independent() {
        const HOLD: f32 = 1.6;
        /// How much survives half a second of decay, at each rate.
        fn survivors_per_half_second(rate: f32) -> f32 {
            let dt = 1.0 / rate;
            let mut kept = 1.0f32;
            // 0.5s at `rate` fps, by repeated application — which is exactly what
            // the shader accumulates over.
            for _ in 0..(0.5 * rate).round() as u32 {
                kept *= gpu::feedback_for_a_dt(dt, HOLD);
            }
            kept
        }
        let at_30 = survivors_per_half_second(30.0);
        let at_60 = survivors_per_half_second(60.0);
        let at_144 = survivors_per_half_second(144.0);
        // 1e-4 is 0.01% — three orders of magnitude tighter than the frame-rate
        // gap being ruled out, and loose enough for float rounding over 500
        // multiplications.
        assert!(
            (at_30 - at_60).abs() < 1e-4 && (at_60 - at_144).abs() < 1e-4,
            "half a second of decay must leave the same amount at any rate — \
             30fps {at_30:.6}, 60fps {at_60:.6}, 144fps {at_144:.6}"
        );

        // And the constant is the time constant, so `HOLD_SECS` of decay is
        // 1/e of the frame by construction rather than by a fitted number.
        let after_hold = gpu::feedback_for_a_dt(HOLD, HOLD);
        assert!(
            (after_hold - std::f32::consts::E.min(1.0 / std::f32::consts::E)).abs() < 1e-5,
            "after one HOLD_SECS, {after_hold} of the frame should remain"
        );
    }

    /// A degenerate frame time cannot resurrect a frame that should be gone.
    ///
    /// `dt` is read from egui's input, so on the first frame of a session it can
    /// be zero, and a predicted value is never negative in a way the app intends.
    /// `(dt / hold).exp()` with `dt == 0` is exactly 1.0 — the *whole* previous
    /// frame kept, which is the permanent smear the decay exists to prevent. The
    /// guard is what stops a hitch from turning the view into a frozen frame.
    #[test]
    fn feedback_refuses_a_degenerate_dt() {
        const HOLD: f32 = 1.6;
        for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                gpu::feedback_for_a_dt(dt, HOLD),
                0.0,
                "dt {dt} must decay to nothing, not keep the previous frame"
            );
        }
        // A real dt never returns a full-strength frame either.
        assert!(gpu::feedback_for_a_dt(1e-9, HOLD) < 1.0);
    }

    /// Every GLSL function is declared before the first use of it.
    ///
    /// **This is the one shader defect a compiler catches and CI never will**, and
    /// it shipped in two of the four views on the day they were written: `noise3`
    /// called `hash13` from below it, and `fbm` called `noise2` from below it.
    /// GLSL has no forward declarations, so both are hard compile errors — on
    /// every driver, every machine, every run — and the headless tests cannot see
    /// either. This is the same gap as everything else about shader compilation,
    /// except that this one has a *cheap deterministic answer* and needs no
    /// context to get it.
    ///
    /// The rule is that a function's name must appear as a definition before any
    /// use, so each definition's first mention in the whole source must be itself.
    /// Comment lines are skipped, so a comment may legitimately mention a
    /// function before it exists.
    #[test]
    fn every_glsl_function_is_declared_before_use() {
        for (view, body) in bodies() {
            for (line, name, at) in glsl_definitions(body) {
                assert_eq!(
                    body.find(&format!("{name}(")),
                    Some(at),
                    "{}:{} defines `{name}`, which GLSL requires to appear before its first \
                     use. GLSL has no forward declarations, so this is a hard compile error on \
                     every driver, and the headless tests cannot see it.",
                    view,
                    line
                );
            }
        }
    }

    /// Every GLSL function definition in `src`, as `(line number, name, byte offset)`.
    ///
    /// Recognised by a **return type**, not by a `(` and a `{`: `for (int i = 0;
    /// i < 4; i++) {` matches that shape and is not a definition. The offset is
    /// returned rather than recomputed by a second helper, because two parsers of
    /// the same source are two answers and this test compares them.
    fn glsl_definitions(src: &str) -> Vec<(usize, String, usize)> {
        const TYPES: [&str; 10] = [
            "float", "int", "uint", "bool", "void", "vec2", "vec3", "vec4", "mat3", "mat4",
        ];
        let mut out = Vec::new();
        let mut at = 0usize;
        for (n, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            let code = trimmed.trim_start_matches("//");
            let mut words = code.split_whitespace();
            if TYPES.contains(&words.next().unwrap_or_default()) {
                if let Some(head) = words.next() {
                    let name = head.split('(').next().unwrap_or_default();
                    let is_name = !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if is_name
                        && head.contains('(')
                        && code[code.find('(').unwrap_or(0)..].contains(") {")
                    {
                        // The offset of the *name*, not of the line: that is what
                        // `find("name(")` reports, and the two have to be the same
                        // number for the comparison to mean anything.
                        let within = code.find(name).unwrap_or(0);
                        out.push((
                            n + 1,
                            name.to_owned(),
                            at + (line.len() - trimmed.len()) + within,
                        ));
                    }
                }
            }
            at += line.len() + 1;
        }
        out
    }

    /// A theme switch reaches the uniforms.
    ///
    /// The colour rule is only real if the palette actually arrives, and the
    /// direction that matters is the one a screenshot cannot show: a view drawn
    /// under `dark` must differ from the same view under `retro`, or it is
    /// ignoring the theme whatever its shader says.
    #[test]
    fn a_shader_view_reads_its_palette() {
        let (rect, viz) = pane();
        let mut seen: Vec<[f32; 4]> = Vec::new();
        for id in ["dark", "retro", "neon"] {
            let themes = Themes::load();
            let theme = themes.get(id).expect("a bundled theme");
            let u = gpu::Uniforms::pack(
                &viz,
                [-60.0; VIZ_BANDS],
                &theme.palette,
                rect,
                &egui::Context::default(),
            );
            assert_ne!(
                u.accent, [0.0; 4],
                "{id}: the accent uniform is black, so a shader using it would draw nothing"
            );
            assert!(
                !seen.contains(&u.accent),
                "{id}: the same accent as another bundled theme — the pack is not reading \
                 the palette it was handed"
            );
            seen.push(u.accent);
        }
    }
}
