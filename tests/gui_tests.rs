//! GUI tests — theme application, dock layout (headless egui).

mod common;

use eframe::egui::{self, Color32, FontFamily};
use std::path::Path;
use tplay::app::Pane;
use tplay::audio::eq::EqShared;
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
