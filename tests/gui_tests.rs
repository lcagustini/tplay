//! GUI tests — theme application, dock layout (headless egui).

mod common;

use eframe::egui::{self, Color32, FontFamily};
use std::path::Path;
use tplay::app::{Pane, VizView};
use tplay::audio;
use tplay::audio::eq::EqShared;
use tplay::audio::viz::{VizBuf, VIZ_BANDS, WAVE_BUCKETS};
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

/// Both lists emit their rows, and the culling still drops what is off-screen.
///
/// **This is the guard for the bug that emptied the Library and the Playlist at
/// once**, and it exists because nothing else could see it.
///
/// The row-culling gate compares the row's position against the scroll area's
/// clip, and it read that position from `ui.cursor().max.y`. Inside a
/// `ScrollArea::show` the content `Ui`'s cursor has **`Pos2::INF` as its min
/// corner** — that is what "no cursor position yet" looks like — so `.max.y` was
/// `inf` too, `top <= clip.max.y + row_h` was false for every row, and both panes
/// drew their chrome, counted their rows correctly, and painted **nothing**.
/// Silent, total, and on the first frame.
///
/// Three things had to be true for it to ship:
///
/// - **The data was fine.** The counts line read non-zero, so the folder really
///   was listed; the failure was purely downstream of a correct listing.
/// - **The existing pane test was blind to it *by construction*.**
///   `the_playlist_pane_leaves_its_list_the_height_it_was_given` asserts
///   `ui.min_rect()`, and the culled path still reserves each row's height with
///   `allocate_space` — reserving nothing is what would collapse the scrollbar.
///   So the one test covering this code asserted the exact property the bug
///   preserved. A `min_rect` assertion cannot catch a missing row, ever.
/// - **The gate was only reachable in a real `ScrollArea`.** A harness using
///   `allocate_new_ui` gives a finite cursor, so a test written that way passes
///   with the bug present. Hence the `DockArea` below: the panes are driven
///   through the same nesting the app uses, tab bodies and all.
///
/// The `Draw` count is checked against the rows that physically fit rather than
/// against the total, so this also pins the other half of the contract: the
/// culling is supposed to *skip* the 40 rows that are off-screen, and a "fix"
/// that simply deleted the gate would be caught by the upper bound here.
#[test]
fn both_lists_emit_the_rows_that_fit() {
    use common::{test_dir, write_wav, TestApp};
    use eframe::egui;
    use egui_dock::{DockState, NodeIndex};
    use tplay::app::Pane;
    use tplay::gui::theme::ThemeState;

    /// The row background `theme::row` fills, one per emitted row.
    fn drawn_rows(out: &egui::FullOutput, palette: &tplay::gui::theme::Palette) -> usize {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) => Some(r),
                _ => None,
            })
            .filter(|r| {
                (r.fill == palette.row_even || r.fill == palette.row_odd)
                    // The banded background spans the row; the row's own controls
                    // are narrow, and the search box and header reuse these tokens.
                    && r.rect.width() > 50.0
                    && r.rect.height() >= 20.0
            })
            .count()
    }

    struct Viewer<'a> {
        app: &'a mut tplay::app::TPlayApp,
        themes: &'a ThemeState,
    }
    impl egui_dock::TabViewer for Viewer<'_> {
        type Tab = Pane;
        fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
            format!("{tab:?}").into()
        }
        fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
            use tplay::gui::panes::{library, playlist};
            match tab {
                Pane::Library => library::library_pane(self.app, self.themes, ui),
                Pane::Playlist => playlist::playlist_pane(self.app, self.themes, ui),
                _ => {}
            }
        }
    }

    // Two panes stacked, so both get a real tab body: a 680x460 window is the
    // default, and 40 rows cannot fit in either half.
    let mut t = TestApp::new("lists-emit-rows");
    let dir = test_dir("lists-emit-rows");
    let mut tracks = Vec::new();
    for n in 0..40 {
        let p = dir.join(format!("{n}.wav"));
        write_wav(&p);
        tracks.push(p);
    }
    t.app.add_files(tracks);
    t.app.navigate_to(dir.clone());
    let rows_total = t.app.playlist().len();
    assert_eq!(rows_total, 40, "premise: 40 rows, more than fit");

    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(680.0, 460.0),
        )),
        ..Default::default()
    };
    let mut tree = DockState::new(vec![Pane::Library]);
    tree.main_surface_mut()
        .split_below(NodeIndex::root(), 0.5, vec![Pane::Playlist]);

    // Two frames: the first draws, the second proves it is not a one-frame state
    // that the next repaint silently undoes.
    let mut drawn = 0;
    for _ in 0..2 {
        let out = ctx.run(raw.clone(), |ctx| {
            let themes = ThemeState::load(ctx, tplay::gui::theme::Themes::load(), "dark");
            let mut viewer = Viewer {
                app: &mut t.app,
                themes: &themes,
            };
            egui::CentralPanel::default().show(ctx, |ui| {
                egui_dock::DockArea::new(&mut tree).show_inside(ui, &mut viewer);
            });
        });
        let themes = ThemeState::load(&ctx, tplay::gui::theme::Themes::load(), "dark");
        drawn = drawn_rows(&out, &themes.current().palette);
    }

    // The Library's 40 rows and the Playlist's 40 rows, in two half-height panes:
    // a dozen or so fit per pane, so the emitted count must be well clear of the
    // one non-row match. `> 1` rather than `> 0` because the Library's column
    // header reuses `row_odd` for its background — exactly one such rect, and it
    // is drawn whether or not the list works, so a `> 0` bound would pass with the
    // bug present. That is the whole reason this assertion is shaped the way it
    // is: measured, with the offender named.
    assert!(
        drawn > 1,
        "the panes listed {} library entries and {} playlist rows but emitted {drawn} \
         banded rects — only the Library's column header, so the culling gate \
         rejected every row. Counts and chrome still render, so this is invisible \
         except as an empty list.",
        t.app.library().entries().len(),
        rows_total
    );
    // ...and the upper bound, so deleting the gate to "fix" it is not a pass.
    assert!(
        drawn < rows_total,
        "all {rows_total} rows were emitted into two half-height panes, so the \
         off-screen culling is not happening (drawn {drawn})"
    );
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

    /// The waterfall's scroll shifts by **whole texels**, and samples accordingly.
    ///
    /// A scrolling history that resamples itself once a frame is a low-pass filter
    /// applied once per column, so a 256-column history is filtered 256 times and
    /// arrives uniformly smeared. The cause is invisible in every other way: the
    /// shader compiles, the callback count and rect are right, the program links,
    /// and the picture is *nearly* right — it is only soft, and only on the pane
    /// widths where the shift does not divide the target's width exactly, which
    /// reads as a property of the window rather than of the code.
    ///
    /// So the claim is about **which sampler the accumulate pass uses**: `texelFetch`
    /// addresses whole texels and `texture()` interpolates between them, and for a
    /// shift that is meant to be a copy of the neighbouring column only one of them
    /// is a copy. Asserted over the accumulate pass alone, because the present pass
    /// is required to interpolate — the quantised target is a little larger than
    /// the pane — so the two passes genuinely need different sampling and the rule
    /// is per-pass, not per-view.
    #[test]
    fn the_waterfall_scrolls_by_whole_texels() {
        let view = SHADER_VIEWS
            .iter()
            .find(|v| v.file == "spectrogram")
            .expect("the spectrogram is in the table");
        let accumulate = view.frags[0];
        assert!(
            accumulate.contains("texelFetch(u_prev"),
            "the spectrogram's accumulate pass must read its history with `texelFetch`, \
             which addresses whole texels. A `texture()` read at a fractional texel \
             offset interpolates between two neighbours, and doing that to the whole \
             history once a frame is a low-pass filter applied once per column — the \
             waterfall arrives blurred, and only on the pane widths whose target size \
             does not divide by the column count, so it reads as the window's fault."
        );
        assert!(
            !accumulate.contains("texture(u_prev"),
            "the spectrogram's accumulate pass must not sample its history through \
             `texture()` — see above. Every read of the feedback target there is a \
             texel copy, not an interpolation."
        );
        // The column count is derived from the target's own width, so it needs the
        // target's own size — which is neither the pane's nor `u_resolution`,
        // because `target_size` quantises it to a 64px grid. Guessing it from a
        // uniform is the mistake this replaced.
        assert!(
            accumulate.contains("textureSize(u_prev"),
            "the spectrogram's column width must come from the feedback target's own \
             size. `u_resolution` is the *pane* in physical pixels and the target is \
             quantised to a 64px grid, so a shift computed from `u_resolution` is \
             wrong by up to a grid cell and the column no longer lines up with the \
             texel it is copying."
        );
    }

    /// The waterfall's new column lands on the edge the scroll **vacates**.
    ///
    /// The two halves of a ring-buffer scroll have to agree: the shift consumes one
    /// edge and the new data overwrites it. When they disagree, nothing errors and
    /// nothing looks broken in the obvious sense — the new column is written on the
    /// edge the shift is *feeding from*, so it is overwritten again on the very next
    /// frame and never scrolls. What the user sees is a permanent bright stripe down
    /// the wrong side of the pane, plus a second copy of the same column at the
    /// opposite edge, and neither is a failure any existing check could name.
    ///
    /// **Checked against the shipped shader, not a copy of it.** An earlier version
    /// of this test simulated the scroll in Rust, which is a second implementation
    /// of the same two lines and would have passed with the shader reverted — the
    /// mirror trap this repo has already been bitten by once, in the shuffle
    /// picker. So both numbers are read out of the shader itself: the edge its
    /// fresh-column test names, and the sign of its shift. A simulation still runs
    /// below them, as the *premise* — it is what makes the pair a claim about a
    /// ring buffer rather than a string match — but it is the shader's arithmetic
    /// that is asserted.
    #[test]
    fn the_waterfalls_new_column_lands_where_the_scroll_vacated_it() {
        let body = SHADER_VIEWS
            .iter()
            .find(|v| v.file == "spectrogram")
            .and_then(|v| v.frags.first().copied())
            .expect("the spectrogram's accumulate pass is in the table");

        // The fresh column's guard, verbatim: `p.x >= sz.x - colw`. What matters
        // is that it names the **high** end of `p.x` and the same `colw` the shift
        // moves by — the two agreeing on one edge is the whole invariant.
        let fresh = body
            .lines()
            .find(|l| l.contains("? fresh : old"))
            .unwrap_or_else(|| panic!("the spectrogram no longer has a fresh/old choice:\n{body}"));
        assert!(
            fresh.contains("p.x >= sz.x - colw"),
            "the spectrogram's new column must land on the rightmost columns — the ones \
             its shift moves its neighbours out of. This guard is `{fresh}`. On any other \
             edge the new column is overwritten on the next frame and never scrolls, \
             which shows as a permanent bright bar down the wrong side of the pane."
        );

        // The shift reads its neighbour from the *low* side, which is the same
        // statement: `p + colw` steps towards the edge the new column takes, so a
        // column moves left and vacates the right. A `- colw` here with the guard
        // above would be a scroll that feeds from the column it is writing.
        let shift = body
            .lines()
            .find(|l| l.contains("texelFetch(u_prev"))
            .unwrap_or_else(|| panic!("the spectrogram no longer reads its history:\n{body}"));
        assert!(
            shift.contains("(p.x + colw) % sz.x"),
            "the spectrogram's scroll must read the column to the right — `p.x + colw` — so \
             that the picture moves left and vacates the rightmost columns, which is where \
             the new column goes. This line is `{shift}`."
        );

        // The premise: one period of the scroll, on the shader's own numbers, writes
        // every column exactly once. Without it the pair above is two string
        // patterns that happen to be spelled correctly; with it, it is a ring
        // buffer. `colw` 1, 2 and 4 are the ones a 64px-grid target can produce at
        // 256, 512 and 1024 columns, plus 3 for a target the size of a typical pane.
        for colw in [1usize, 2, 3, 4] {
            const COLUMNS: usize = 256;
            let mut history = [0u8; COLUMNS];
            for _ in 0..COLUMNS {
                let mut next = [0u8; COLUMNS];
                for (p, slot) in next.iter_mut().enumerate() {
                    *slot = if p >= COLUMNS - colw {
                        u8::MAX
                    } else {
                        history[(p + colw) % COLUMNS]
                    };
                }
                history = next;
            }
            assert!(
                history.iter().all(|v| *v == u8::MAX),
                "colw = {colw}: a full period of this scroll must consume every column \
                 exactly once, so no column is ever frozen and none is read twice. \
                 {history:?}"
            );
        }
    }

    /// The dB floor is **one number** across the DSP and every shader.
    ///
    /// `compute_bands` clamps into `DB_FLOOR..=0.0` and every shader maps that
    /// range up to `0..1` through the prelude's `level()`. The two ends are a
    /// boundary between halves that cannot see each other, and the mapping was a
    /// hand-written `float level(float d)` in **six** view bodies plus two inlined
    /// copies — all correct, all separate. That is fine until the floor moves, and
    /// then **nothing fails**: every shader still compiles, every value is still in
    /// range, and the symptom is a view that quietly compresses or clips its quiet
    /// end, differently in each one, on whichever side was not updated.
    ///
    /// So the number is interpolated into the prelude from [`DB_FLOOR`] and this
    /// test reads the *assembled* source — the same string the driver gets — rather
    /// than trusting that a view called the shared helper. Two directions, because
    /// either one alone is satisfiable by luck:
    ///
    /// 1. the prelude's `DB_FLOOR` is the DSP's, to the bit; and
    /// 2. no view defines a `level` of its own, and none writes the floor as a
    ///    literal — so there is exactly one place the number can be.
    #[test]
    fn the_db_floor_is_one_number_across_the_dsp_and_every_shader() {
        let assembled = gpu::fragment_source("float x = level(u_bands[0]);");
        let floor = format!("const float DB_FLOOR = {};", audio::viz::DB_FLOOR);
        assert!(
            assembled.contains(&floor),
            "the assembled prelude must carry the dB floor the DSP reports, spelled \
             `{floor}`. The two ends of this range are a boundary the halves cannot \
             see across: the DSP clamps into it and the shaders map out of it, so a \
             mismatch fails silently as a view that quietly compresses its quiet end."
        );
        // The span has to be derived, not typed: `(d + 60.0) / 60.0` assumes a
        // symmetric range, and a floor of -48 would silently be wrong.
        assert!(
            assembled.contains(&format!("const float DB_SPAN = {};", -audio::viz::DB_FLOOR)),
            "the prelude's DB_SPAN must be the width of the DSP's range, derived from \
             DB_FLOOR rather than typed — a literal assumes a symmetric range and is \
             wrong the moment the floor is not half of the ceiling."
        );

        assert!(
            !SHADER_VIEWS.is_empty(),
            "the table is empty, so this sweep would pass vacuously"
        );
        for view in SHADER_VIEWS {
            for body in view.frags {
                // Comments are skipped: a view may *name* the constant it no longer
                // writes down.
                let code: String = body
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    !code.contains("float level("),
                    "{}: defines its own `level`, and the prelude owns that function. \
                     A second copy is a second dB mapping, which is the thing this \
                     exists to prevent — and the prelude's is the one built from the \
                     DSP's floor.",
                    view.name
                );
                for (n, line) in code.lines().enumerate() {
                    assert!(
                        !line.contains("60.0"),
                        "{}:{} writes the dB floor as a literal — `{line}`. Use \
                         `level(d)`, or `DB_FLOOR` where the raw range is wanted, so \
                         the floor is written down once.",
                        view.name,
                        n + 1
                    );
                }
            }
        }

        // The Rust half, which is where the *other* nine copies lived: a view seeds
        // its smoothing buffer with `[DB_FLOOR; VIZ_BANDS]`, and a literal there is
        // the same silent drift — the buffer starts at a level `compute_bands` can
        // never produce, so the first frames of every view are wrong by an amount
        // nobody can see. Reading the sources is what catches it; the first version
        // of this test only read the GLSL and a retyped seed passed it.
        for view in SHADER_VIEWS {
            let code: String = view_source(view.file)
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("-60.0"),
                "{}: seeds or bounds something with a dB literal. `DB_FLOOR` is the \
                 one definition, and a view's first frame is exactly where a wrong \
                 floor shows with nothing else to compare it against.",
                view.name
            );
        }
    }

    /// The waterfall's two passes must flip the target's `y` **the same way**.
    ///
    /// A feedback round trip is two conversions of one coordinate, and they have to
    /// cancel: the accumulate pass writes the target's rows and the present pass
    /// reads them back. A framebuffer's rows run **bottom-up** (row 0 is at NDC
    /// y = −1) while `v_uv.y` is 0 at the pane's **top**, so getting the pair
    /// wrong does not merely turn the picture upside down — it mirrors the target
    /// on the way in and reads it back the same way round, and since the present
    /// pass *interpolates*, the low-pass of an image and its mirror **is**
    /// mirror-symmetric. So the pane looks like it is reflecting itself about the
    /// horizontal centre line, and the beat between consecutive mirrored frames is
    /// a regular pattern of vertical bars. Both symptoms, one cause.
    ///
    /// Nothing else in the repo can see this. The shaders compile, the callback
    /// count and rect are right, the program links, the column count is right, and
    /// the scroll genuinely happens — it is a picture that is *nearly* right and
    /// reflected, which is the worst shape of bug to diagnose from inside the code.
    ///
    /// `Trails` is immune, and the reason is worth having: its accumulate transform
    /// is a spin and a zoom about the pane's centre, so a whole-image `y` flip is
    /// one of that transform's own symmetries and cancels on its own. Only the
    /// waterfall carries **per-column** data, where a flip is visible.
    ///
    /// Asserted by reading the flip out of **both shipped strings** and requiring
    /// them to match. The simulation underneath is the premise that makes those two
    /// string checks mean something — it is the round trip, over a small target —
    /// and it is written so that flipping one pass alone breaks it, which is what
    /// makes "they must agree" a fact rather than a coincidence of two greps.
    #[test]
    fn the_waterfalls_two_passes_flip_y_together() {
        let view = SHADER_VIEWS
            .iter()
            .find(|v| v.file == "spectrogram")
            .expect("the spectrogram is in the table");
        assert_eq!(
            view.frags.len(),
            2,
            "the waterfall runs two programs — the accumulate must address whole texels and \
             the present must interpolate — so a single source cannot do both. Found {}.",
            view.frags.len()
        );
        let (accumulate, present) = (view.frags[0], view.frags[1]);

        // The accumulate's flip is a framebuffer-space row index; the present's is a
        // flipped sample. Read as "is this row mirrored", not as a spelling, so that
        // either form of the flip satisfies the rule and only a *missing* one — which
        // is the bug — fails it.
        let accumulate_flips = accumulate.contains("sz.y - 1 - int(");
        let present_flips = present.contains("1.0 - v_uv.y");
        assert!(
            accumulate_flips,
            "the waterfall's accumulate pass must address the target in *framebuffer* space: \
             a framebuffer's rows run bottom-up and `v_uv.y` is 0 at the pane's top, so a \
             read at `int(v_uv.y * sz.y)` and a write at the rasterised row land on \
             different rows. That mirrors the whole history on every frame."
        );
        assert_eq!(
            accumulate_flips, present_flips,
            "the waterfall's accumulate pass flips y ({accumulate_flips}) and its present \
             pass flips y ({present_flips}). They are two conversions of one coordinate and \
             must cancel: a target mirrored on the way in and read back the same way is not \
             upside down, it is *its own mirror image* softened by the present pass's \
             interpolation, with a vertical beat from the frames alternating. `Trails` does \
             not need the pair because its accumulate transform is symmetric under a \
             whole-image y flip — only the waterfall carries per-column data, where a flip \
             is visible."
        );

        // The premise: a `ROWS`-row target, each row holding its own index, through one
        // accumulate write and one present read. The write lands on the rasterised row
        // `w`; the read asks for row `w` back, unless the present's uv is top-down over
        // a bottom-up texture, in which case it asks for `ROWS - 1 - w`. So the round
        // trip is the identity exactly when the two flips agree — and this loop is
        // what the two string assertions above are standing on.
        const ROWS: usize = 9;
        let target: Vec<u8> = (0..ROWS).map(|r| r as u8).collect();
        for (label, p_flip) in [
            ("the present samples top-down", false),
            ("and the present samples bottom-up", true),
        ] {
            let shown: Vec<u8> = (0..ROWS)
                .map(|w| {
                    if p_flip {
                        target[w]
                    } else {
                        target[ROWS - 1 - w]
                    }
                })
                .collect();
            let identity = shown.iter().enumerate().all(|(r, v)| *v == r as u8);
            assert_eq!(
                identity, p_flip,
                "{label}: the row the accumulate wrote must come back as the row that was \
                 written, and this case says otherwise (rows {shown:?} over a {ROWS}-row \
                 target whose rows hold their own index). The accumulate pass always writes \
                 bottom-up, so the present pass has to read bottom-up too."
            );
        }
    }

    /// No shader indexes a band through an **angle**.
    ///
    /// The obvious way to say "the band at the angle this fragment sits at" is
    /// `atan(y, x)` scaled and floored into `u_bands`, and three views shipped
    /// exactly that. **`atan` has a branch cut**: it jumps from `+pi` to `-pi` along
    /// `x < 0` at `y == 0`, which is a fixed line up the middle of the pane's left
    /// side. So the band index steps there, and with it whatever the index drives —
    /// `Plasma`'s warp strength, `Trails`' ring brightness — steps with it.
    ///
    /// **No edge fade can hide this, and that is the reason it is a rule rather
    /// than a style note.** A fade works by taking a value to zero at a
    /// *boundary*; this is a break in the middle of the *function*, so the value
    /// either side of the cut is two different colours and the best a fade can do
    /// is smear between them. `Plasma` and `Trails` both showed it as a seam on the
    /// left, and in `Trails` it was worse than a seam: `fresh` is **added** to the
    /// buffer every frame, so the step was deposited as a permanent line.
    ///
    /// The replacement is the harness's `spectrum_at_bearing` — a projection onto
    /// the bearing's first harmonics, which is the same shape of read and is
    /// continuous and periodic by construction. Asserting the *forbidden* form is
    /// what makes it a rule: a new view that wants a radial spectrum read has one
    /// path to find, rather than one trap to rediscover. `Radial` is the case that
    /// proves the rule needs its exception stated — it *does* index a band by
    /// angle, because one band per angular sector is the whole picture, and its cut
    /// lands exactly on a segment gap the view already draws.
    #[test]
    fn a_shader_never_indexes_a_band_through_an_angle() {
        // Angular sectors are legitimate when the angle *is* the layout. `Radial`
        // draws one band per sector and leaves a gap at `within < 0.05`, so the
        // cut falls in a boundary it draws anyway — a break in the layout is not a
        // break in the picture. Named rather than pattern-matched, because a
        // pattern would have to guess at intent.
        const SECTORED: [&str; 1] = ["Radial"];
        for (name, body) in bodies() {
            if SECTORED.contains(&name) {
                continue;
            }
            // Comments are skipped, so a view may *name* the function it no longer
            // calls — which is how both seams are written down next to the fix.
            let code: String = body
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("atan("),
                "{name}: reads the spectrum through `atan`, whose branch cut along \
                 `x < 0` at `y == 0` steps the band index down the pane's left edge. \
                 The value either side of a cut is two different colours, so this \
                 cannot be faded out. Use `spectrum_at_bearing(p)` from the harness \
                 prelude, which is continuous and periodic by construction. A view \
                 whose *layout* is angular sectors may index a band directly — \
                 {SECTORED:?} is the one that does, and only because it draws a gap \
                 there anyway."
            );
        }
        // The replacement has to exist and be reachable, or "use the helper" is an
        // instruction to write a new one.
        assert!(
            gpu::fragment_source("float b = spectrum_at_bearing(p);")
                .contains("float spectrum_at_bearing(vec2 p)"),
            "a shader that asks for `spectrum_at_bearing` must get it, and get the \
             `u_bands` and `VIZ_BANDS` declarations it needs with it."
        );
    }

    /// The trail's edge fade is keyed on the fragment's **own** coordinate.
    ///
    /// `Trails` samples its previous frame through a shrink toward the centre, so
    /// the sample coordinate and the screen coordinate are nowhere near each other
    /// at the pane's edges: at `centred.x = -0.5` the sample lands at about `0.502`,
    /// the middle of the pane. The fade was computed from the *sample*, so it put a
    /// ramp in the middle of the picture and left the actual left edge at full
    /// weight — and the `clamp` then held that edge texel at full strength for the
    /// whole fade, which is a hard line and not a gradient.
    ///
    /// This is the same "which space is this coordinate in" trap as the waterfall's
    /// `y` flip, and the two are the only views that reach for a coordinate other
    /// than the fragment's own — which is why both are pinned.
    #[test]
    fn the_trails_fade_follows_the_fragment_not_the_sample() {
        let body = SHADER_VIEWS
            .iter()
            .find(|v| v.file == "trails")
            .and_then(|v| v.frags.first().copied())
            .expect("trails' accumulate pass is in the table");
        let fade = body
            .lines()
            .find(|l| l.contains("smoothstep"))
            .unwrap_or_else(|| panic!("trails no longer has an edge fade:\n{body}"));
        assert!(
            fade.contains(", uv)"),
            "the trail's edge fade must be keyed on `uv`, the fragment's own position: \
             `{fade}`. The trail shrinks toward the centre, so the sample coordinate \
             `prev` is near the middle of the pane at the pane's own edge — a fade on \
             it ramps through the middle of the picture and leaves the real edge at \
             full weight, which is a hard line and not the gradient it was meant to be."
        );

        // The premise: the sample coordinate at the pane's left edge is nowhere near
        // 0, so no fade keyed on it can coincide with the pane's edge. This is the
        // transform as the shader states it, at `centred.x = -0.5`.
        const SPIN: f32 = 0.004;
        const ZOOM: f32 = 0.0035;
        let centred = (-0.5f32, 0.0f32);
        let (s, c) = (SPIN.sin(), SPIN.cos());
        let spun = (centred.0 * c - centred.1 * s, centred.0 * s + centred.1 * c);
        let prev_x = (spun.0 * (1.0 - ZOOM) - centred.0) + 0.5;
        assert!(
            prev_x > 0.4,
            "the premise: at the pane's left edge the trail's sample lands at {prev_x}, \
             nowhere near 0. If this ever became ~0 the fade would coincide with the \
             edge and the check above would be reading a coincidence."
        );
    }

    /// One view's Rust source, by its `file` stem.
    ///
    /// A property about *where a name appears* is not measurable at runtime, so
    /// the sweeps that have one read the source. Keyed on the table's `file` field
    /// rather than guessing from the name — that guess is wrong the moment a UI
    /// label stops being a filename, and a wrong guess is an IO error rather than
    /// a wrong answer, which is the worst kind of failure for a test.
    fn view_source(file: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/gui/panes/visualizer/views")
            .join(format!("{file}.rs"));
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// Every GLSL body the app ships, as `(view name, source)`.
    ///
    /// Flattened from the table's `frags` rather than reading one `frag` per view:
    /// `Trails` runs two programs, and a sweep that only saw the first would leave
    /// the second — which is half of what that view draws — unchecked.
    fn bodies() -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        for view in SHADER_VIEWS {
            out.extend(view.frags.iter().map(|body| (view.name, *body)));
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

    /// Every view has a shader, and the table is exactly the views.
    ///
    /// **Both directions, and the reverse one is the one that bites.** Before
    /// every view was a shader, the compiler was what noticed a new `VizView`
    /// variant with no arm in the pane's `match` — the match was exhaustive over
    /// the enum, so a variant without an arm did not build. That safety went when
    /// the dispatch became a table lookup, which is the only reason this test has
    /// to check the *other* direction: a `VizView` the table does not name is a
    /// view that appears in the dropdown, is written to `config.json`, restores
    /// across sessions — and draws absolutely nothing. The old shape of this test
    /// only checked that the table had no unreachable rows, which is the harmless
    /// half.
    ///
    /// A row whose `draw` is never called is the mirror failure, and it is just
    /// as invisible: the view exists and is blank.
    #[test]
    fn every_view_has_a_shader_and_the_table_covers_them_all() {
        let names: Vec<&str> = VizView::ALL.iter().map(|v| v.name()).collect();
        let table: Vec<&str> = SHADER_VIEWS.iter().map(|v| v.name).collect();
        for name in &names {
            assert!(
                table.contains(name),
                "`{name}` is a selectable view and has no row in SHADER_VIEWS, so the \
                 pane's table lookup finds nothing and it draws a bare background. The \
                 dispatch is a lookup rather than an exhaustive match, so the compiler \
                 no longer catches this."
            );
        }
        for name in &table {
            assert!(
                names.contains(name),
                "SHADER_VIEWS has `{name}`, which is not a VizView name — the dispatch \
                 looks the row up by `VizView::name()`, so this row is unreachable"
            );
        }
        assert_eq!(
            table.len(),
            names.len(),
            "the table and VizView::ALL disagree on how many views there are"
        );
        // The arm that dispatches them. Not a name check: a view is reached
        // through the table, so no view name appears in the pane at all. What has
        // to be there is the table lookup itself — a pane that painted a
        // background and returned would compile, and every view would render
        // nothing.
        let pane = src("src/gui/panes/visualizer.rs");
        let code: String = pane
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("SHADER_VIEWS"),
            "visualizer.rs has no arm dispatching through SHADER_VIEWS, so every view \
             in the table is unreachable and the pane paints only its background"
        );
    }

    /// A view draws its shader and nothing else.
    ///
    /// The two guards that used to sit here were about *how* a view filled a
    /// shape: no closed paths, and every filled piece convex, because
    /// `epaint`'s closed-path fill is a triangle fan from its first vertex and
    /// insets the fill by half its feathering. Both were real defects — a ring
    /// sector and a ridgeline both shipped broken that way — and both cost a
    /// workaround: `fill_quads` decomposing into convex quads in two views, a
    /// 1 200-segment hand-built mesh in a third, and about a hundred lines of
    /// tests proving the workarounds correct.
    ///
    /// None of that can happen now, and this is the cheap guard that says so. A
    /// fragment shader rasterises a shape by deciding what colour each fragment
    /// is, so there is no fan, no inset, no shared edge between two adjacent
    /// pieces and no tessellator to get wrong. The class of defect is designed out
    /// rather than tested for, which is the better answer and the shorter one.
    ///
    /// **What is banned is drawing, not the painter.** Every view still takes a
    /// `&egui::Painter`, because that is how it queues its callback — so the check
    /// is on the primitive names, not on the receiver.
    #[test]
    fn no_view_paints_anything_with_egui() {
        const DRAWING: [&str; 9] = [
            "rect_filled",
            "rect_stroke",
            "line_segment",
            "circle_",
            "hline",
            "vline",
            "add_mesh",
            "Shape::mesh",
            "Shape::Path",
        ];
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gui/panes/visualizer/views");
        let mut files = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            files += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            // Strip comment lines, so a view may *explain* a rule it follows.
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for banned in DRAWING {
                assert!(
                    !code.contains(banned),
                    "{} draws with egui (`{banned}`). Every view is a fragment shader \
                     now: the pane hands it a callback and a rect, and the shader \
                     decides each fragment. Painting here would cover the shader, or \
                     stand in for it, and the view would still look deliberate.",
                    path.display()
                );
            }
        }
        assert_eq!(
            files,
            SHADER_VIEWS.len() + 1,
            "the views directory holds {files} files for {} table rows plus mod.rs — a \
             view file with no row, or a row with no file",
            SHADER_VIEWS.len()
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
    /// `u_modes` is the one uniform a *view* writes directly rather than having
    /// `pack` fill it, so it is the one a view can forget. Everything else is
    /// measured by `pack`, which cannot be forgotten because it has no per-view
    /// half — `u_hold` was the second, for a view that no longer exists.
    ///
    /// `u_prev` is excluded on purpose: it is a sampler and the harness owns it,
    /// and a view never writes one.
    #[test]
    fn a_superset_uniform_reaches_a_view_that_uses_it() {
        for view in SHADER_VIEWS {
            for uniform in ["u_modes"] {
                if !view.frags.iter().any(|f| f.contains(uniform)) {
                    continue;
                }
                let text = src(&format!("src/gui/panes/visualizer/views/{}.rs", view.file));
                let field = uniform.trim_start_matches("u_");
                assert!(
                    text.contains(&format!("uniforms.{field}")),
                    "{}: its shader reads `{uniform}`, so it must pack it into \
                     `uniforms.{field}` — a view-written uniform the view forgets to \
                     write reads as 0.0, which for this one is silence",
                    view.name
                );
            }
        }
    }

    /// A feedback target is never zero-sized, and survives sub-pixel jitter.
    ///
    /// Two failure modes, and the second is the one that bit.
    ///
    /// **Zero is a silent blank.** A pane legitimately measures zero on one side
    /// for a frame while a splitter is dragged, and GL's `texImage2D` and
    /// `glFramebufferTexture2D` both reject a zero dimension with
    /// `INVALID_VALUE` — which a driver reports on a path nobody checks, so the
    /// view is simply blank. `target_size` floors at a whole grid cell, so this is
    /// now structurally impossible rather than guarded against.
    ///
    /// **Jitter is a per-frame rebuild that leaks.** A dock's pane rect moves by
    /// fractions of a pixel as a splitter settles. An exact size check reads that
    /// as "the targets are the wrong size", so both targets are rebuilt — and each
    /// rebuild leaked the pair it replaced, because an `Fbo` cannot implement
    /// `Drop` (deleting a GL object needs a context). At ~3.5M pixels a pane on a
    /// HiDPI display that is tens of megabytes per rebuild, sixty times a second.
    /// The grid is what makes the size change only when the pane really did.
    #[test]
    fn a_feedback_target_is_quantised_and_never_zero() {
        let rect = |w: f32, h: f32| egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h));
        // A collapsed pane, on both axes, and negative for good measure.
        for r in [rect(0.0, 300.0), rect(400.0, 0.0), rect(0.0, 0.0)] {
            let (w, h) = gpu::target_size(r, 2.0);
            assert!(
                w >= 1 && h >= 1,
                "target_size({:?}) = ({w}, {h}) — a zero dimension is INVALID_VALUE and \
                 the view is blank with nothing in the log",
                r.size()
            );
        }
        // Jitter: the same pane measured a hundredth of a pixel apart must give
        // the same size, or every frame rebuilds and leaks the pair it replaced.
        let base = gpu::target_size(rect(400.0, 300.0), 2.0);
        for dw in [-0.01f32, -0.004, 0.004, 0.01, 0.4] {
            for dh in [-0.01f32, 0.01, 0.4] {
                let got = gpu::target_size(rect(400.0 + dw, 300.0 + dh), 2.0);
                assert_eq!(
                    got, base,
                    "a {dw}x{dh} px change moved the target from {base:?} to {got:?} — \
                     the pane jitters by fractions of a pixel, and every flip rebuilds \
                     both targets and leaks the pair it replaced"
                );
            }
        }
        // And it does track a real resize.
        assert_ne!(gpu::target_size(rect(800.0, 300.0), 2.0), base);
    }

    /// A feedback target is sized in **physical** pixels, not points.
    ///
    /// The pane rect is in points and the screen is in physical pixels; on a
    /// HiDPI display they differ by the scale factor. Sizing a render target from
    /// the rect alone therefore gives a target a quarter of the on-screen area at
    /// 2x, and the trail is drawn at a quarter of the resolution and stretched —
    /// which looks like a soft, low-resolution smear rather than like a bug.
    #[test]
    fn a_feedback_target_scales_with_the_display() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let at_1x = gpu::target_size(rect, 1.0);
        let at_2x = gpu::target_size(rect, 2.0);
        assert!(
            at_2x.0 >= at_1x.0 * 2 - 64 && at_2x.0 <= at_1x.0 * 2 + 64,
            "the same pane at 2x scale gave {at_1x:?} and {at_2x:?} — a render target \
             sized in points is a quarter of the on-screen area on a HiDPI display"
        );
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

    /// No shader reads `gl_FragCoord`.
    ///
    /// **This is the guard for the worst bug the pane has had, and it is a
    /// source-reading test because there is no other kind available.** Every one
    /// of the ten views divided `gl_FragCoord.xy` by `u_resolution`, as though
    /// `gl_FragCoord` were *pane*-relative with a top-left origin. It is neither:
    /// it is window-relative and **bottom-left origin**. So a view in a dock tab
    /// anywhere but the left edge of the window read part of its own pattern from
    /// outside itself, and the slice where the quotient passed 1.0 saturated —
    /// the bars and the wave froze on a vertical seam at a constant value, the
    /// rings drew off-centre because `0.5` is `pane_w / 2` of *window* x, the
    /// ridgeline was upside down, the nebula's raymarch camera started outside the
    /// volume so every fragment hit the early-out, and the waterfall's new column
    /// landed somewhere other than its edge.
    ///
    /// Nine of ten views, six distinct symptoms, and **every headless test passed
    /// the whole time.** The shaders compile, the callback count and rect are
    /// right, the program links, the driver reports no error — all of it correct,
    /// none of it about coordinate spaces. It took a person looking at the window,
    /// which is the instrument this project does not have.
    ///
    /// So the property is pinned in the source, the same shape as
    /// `the_gl_harness_never_names_a_view`: the uv comes from the vertex shader's
    /// `v_uv` varying, which the *viewport* maps onto the pane, so a view cannot
    /// get this wrong by accident and never needs to know where the pane is.
    #[test]
    fn no_shader_reads_gl_fragcoord() {
        // The vertex shader's half of the contract, read from the same string the
        // driver compiles. A `const` would be nicer but `vertex_source` allocates.
        let vert = gpu::vertex_source();
        assert!(
            vert.contains("out vec2 v_uv;")
                && vert.contains("v_uv = vec2(corner.x, 1.0 - corner.y);"),
            "the shared vertex shader stopped handing the fragment stage a \
             pane-relative `v_uv` (top-down). Every view gets its uv from it, so \
             this is the one thing that has to be true. Got: {vert}"
        );
        for (name, body) in bodies() {
            for (n, line) in body.lines().enumerate() {
                let code = line.trim_start().trim_start_matches("//");
                assert!(
                    !code.contains("gl_FragCoord"),
                    "{}:{} reads `gl_FragCoord`\n  {line}\nIt is window-relative and \
                     bottom-left origin; every view here is pane-relative and top-down. \
                     Use the `v_uv` varying, which the viewport maps onto the pane.",
                    name,
                    n + 1
                );
            }
        }
    }

    /// A density ramp's lower edge is below the field's mean.
    ///
    /// **Nebula rendered a bare background for its whole life because of this.**
    /// Its `fbm` summed three octaves to 0..0.875 with a mean near 0.44, and the
    /// march accumulated `smoothstep(0.62, 0.98, field)` — so a field averaging
    /// 0.44 fed a ramp starting at 0.62, essentially every sample contributed
    /// zero, `acc` stayed at zero, and the pane was the background colour. A
    /// correct program, a linked program, a clean log, and a silent failure.
    ///
    /// The property is the one that makes a ramp show anything at all: **a ramp
    /// whose lower edge is above the field's mean discards more than half of every
    /// sample.** It is read out of the two literals rather than reasoned about,
    /// because both are numbers a shader states and neither a compiler nor a
    /// screenshot compares them. Crude, readable, and it fires.
    #[test]
    fn a_density_ramp_starts_below_the_field_it_reads() {
        for (name, body) in bodies() {
            // `acc += smoothstep(A, B, d) * ...` — the march's accumulation.
            for (n, line) in body.lines().enumerate() {
                let code = line.trim_start();
                if !code.contains("acc +=") {
                    continue;
                }
                let lo = code
                    .split_once("smoothstep(")
                    .and_then(|(_, rest)| rest.split_once(','))
                    .and_then(|(a, _)| a.trim().parse::<f64>().ok());
                let Some(lo) = lo else { continue };
                // The normalised field this reads is 0..1 with a mean near 0.5 —
                // a mean at or above the ramp's lower edge means most of every
                // sample is thrown away before it is accumulated.
                assert!(
                    lo < 0.5,
                    "{}:{} accumulates `smoothstep({lo}, ..)`, which starts above the \
                     field's mean of ~0.5. A ramp that discards more than half of \
                     every sample leaves the accumulated density at zero, and the \
                     view renders the background colour — a working program and a \
                     silent failure.\n  {line}",
                    name,
                    n + 1
                );
            }
        }
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

    /// Every shipped shader compiles **and links** against the shared vertex shader.
    ///
    /// This is the test that had to exist. Two real defects shipped in the first
    /// version of these views and *nothing* in the suite could see either:
    ///
    /// 1. A function named `noise3`, which is a **GLSL built-in** (`vec3
    ///    noise3(vec3)`). Overloading it with a `float` return is a hard error —
    ///    glslang words it as "overloaded functions must have the same return
    ///    type", which reads like a duplicate definition and is not one.
    /// 2. `p *= 17.0;` on a function **parameter**, which is `in` and read-only.
    ///
    /// Both are invisible to every other check: the shaders are strings, the app
    /// is headless, and the pane's only symptom is a blank background — which is
    /// exactly what the documented failure path renders, so a broken shader and
    /// a working one look the same to every other instrument in the repo. So the
    /// claim that "shader compilation cannot be tested" was only half true: it
    /// cannot be tested *in CI*, which has no `glslang`, but it can be tested
    /// **wherever a validator exists**, and that is every developer machine.
    ///
    /// Skipped, not failed, when `glslangValidator` is absent — CI must stay green
    /// and has no GPU toolchain. A test that hard-failed on a missing external
    /// tool would be deleted the first time somebody's machine lacked it.
    ///
    /// It validates `gpu::fragment_source`'s own output rather than a
    /// hand-assembled copy, so what is checked is what ships.
    #[test]
    fn every_shader_compiles_and_links() {
        let Ok(validator) = which("glslangValidator") else {
            eprintln!("skipping: glslangValidator is not installed");
            return;
        };
        let dir = common::test_dir("shader-glsl");
        let vert = dir.join("vert.vert");
        std::fs::write(&vert, gpu::vertex_source()).unwrap();

        for (name, body) in bodies() {
            let path = dir.join(format!("{}.frag", name.replace(' ', "_")));
            std::fs::write(&path, gpu::fragment_source(body)).unwrap();
            let out = std::process::Command::new(&validator)
                .arg("-l")
                .arg(&vert)
                .arg(&path)
                .output()
                .expect("glslangValidator runs");
            assert!(
                out.status.success(),
                "{name}: the shader does not compile or does not link against the shared \
                 vertex shader.\n{}\nThe assembled source is at {}",
                String::from_utf8_lossy(&out.stdout),
                path.display()
            );
        }
    }

    /// The first `glslangValidator` on `PATH`, or `None`.
    fn which(tool: &str) -> Result<std::path::PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(tool))
            .find(|p| p.is_file())
            .ok_or(())
    }

    /// No shader's march is expensive enough to stop the window presenting.
    ///
    /// The one failure mode with no other guard, because it is invisible *and* it
    /// takes the whole app down rather than the one view: a shader asking for
    /// more arithmetic per fragment than a GPU can retire in a frame-time does not
    /// draw slowly, it stops presenting. Nebula's first version was 10752 noise
    /// evaluations per fragment — 7.7 billion a frame at 720k fragments — because
    /// a two-round domain warp is *quadratic* in `fbm` calls and that was
    /// evaluated inside a 48-step march. The symptom was the entire window
    /// flashing, and nothing in the repo could see it, because a shader that never
    /// finishes a frame looks exactly like one that drew nothing.
    ///
    /// **This started as a call-graph cost estimator and was deleted.** It was
    /// wrong twice in ways that made it *pass* — it stopped a function body at the
    /// first `}` at any indentation, and it could not resolve `const int STEPS`, so
    /// the loop multiplier silently became 1. Both bugs made the guard useless in
    /// exactly the situation it existed for, which is the worst failure a test can
    /// have: not a false alarm, but false confidence. So this counts the two
    /// things that actually caused the blowup, with substring counting that can be
    /// verified by reading: how many times the march's field function calls `fbm`,
    /// and the largest step count in the shader. Their product is a crude proxy for
    /// per-fragment cost — it cannot tell a cheap call from an expensive one, and
    /// it does not model octaves — and it is here because a crude check that fires
    /// beats a precise one that does not.
    ///
    /// The real check is whether the view runs at 60 Hz, which needs a GPU and a
    /// human eye. This is the thing that stops the next 100x from being typed.
    #[test]
    fn no_shader_marches_further_than_it_can_afford() {
        /// `fbm` call sites x the largest step count. Nebula as fixed is 1 x 24;
        /// the version that broke the window was 7 x 48.
        const BUDGET: u64 = 150;
        /// A march longer than this is never worth its cost at 720k fragments,
        /// whatever the field does.
        const MAX_STEPS: u64 = 64;
        for (name, body) in bodies() {
            let (sites, steps) = march_cost(body);
            assert!(
                sites * steps <= BUDGET,
                "{name}: {sites} `fbm` call sites in the march x {steps} steps is over the \
                 {BUDGET} budget. This does not slow the view down — it stops the whole \
                 window presenting, because no frame ever finishes. A domain warp is \
                 quadratic in fbm calls, and that is what broke this once already."
            );
            assert!(
                steps <= MAX_STEPS,
                "{name}: {steps} steps is over the {MAX_STEPS} ceiling — at 720k fragments \
                 every step is a full noise evaluation per pixel"
            );
        }
    }

    /// `(fbm call sites, largest step count)` for a shader, by counting
    /// substrings. Crude on purpose — see the test's doc comment.
    fn march_cost(src: &str) -> (u64, u64) {
        let sites = src.matches("fbm(").count() as u64;
        // `const int NAME = N;` in this shader, so a loop bounded by `i < STEPS`
        // resolves. A bound the harness injects (`VIZ_BANDS`) is not in the
        // source to read, so it is skipped rather than guessed at.
        let consts: std::collections::HashMap<&str, u64> = src
            .lines()
            .filter_map(|l| {
                let rest = l.trim().strip_prefix("const int ")?;
                let (name, value) = rest.split_once(" = ")?;
                Some((
                    name.trim(),
                    value.trim().trim_end_matches(';').parse::<u64>().ok()?,
                ))
            })
            .collect();
        let steps = src
            .lines()
            .filter(|l| l.trim_start().starts_with("for"))
            .filter_map(|l| l.split_once("i < "))
            .filter_map(|(_, rest)| rest.split_whitespace().next())
            .filter_map(|tok| {
                tok.trim_end_matches(';')
                    .parse::<u64>()
                    .ok()
                    .or_else(|| consts.get(tok.trim_end_matches(';')).copied())
            })
            .max()
            .unwrap_or(1);
        (sites, steps)
    }

    /// The uniform block fits inside the fragment uniform limit GL 3.3 promises.
    ///
    /// A `float` array's elements may be given a whole `vec4` slot each rather
    /// than packed four to one, so the block's cost is bounded by counting *one
    /// vector per array element* — the pessimistic reading, and the one that
    /// matters because which packing a driver picks is not observable before you
    /// run on it. GL 3.3 core's guaranteed floor for
    /// `MAX_FRAGMENT_UNIFORM_VECTORS` is 224.
    ///
    /// **This is not a compiler-checked property**, which is the whole reason it
    /// needs a test. glslang links a 256-element float array without complaint —
    /// the limit is a driver limit — and a driver that cannot place the block
    /// fails at *its* link time, on one machine, as a blank pane with a log line.
    /// The wave envelope was written at 256 buckets, which is 288 vectors with
    /// the bands, comfortably over the floor and well under what every real GPU
    /// since 2012 reports. That is exactly the reasoning that ships a shader that
    /// renders on the developer's machine and nowhere else.
    #[test]
    fn the_uniform_block_fits_the_gl_33_floor() {
        /// GL 3.3 core, table 2.11: the guaranteed minimum. A literal, because it
        /// is a property of the spec and not of this code.
        const FLOOR: u64 = 224;
        // The block's arrays, worst case one vector per element. **Read from the
        // crate, not written out here** — which is the opposite of the usual
        // "a budget that reads the constants it guards cannot report that they
        // moved" rule. That rule is about asserting a *value*; this asserts a
        // *relationship*, and hardcoding the sizes made it unfailable: the sum
        // was 160 whichever way the constants moved, so a 256-bucket wave envelope
        // passed a test that exists to catch exactly that.
        let arrays: [(&str, u64); 2] = [
            ("u_bands", VIZ_BANDS as u64),
            ("u_wave", WAVE_BUCKETS as u64),
        ];
        let total: u64 = arrays.iter().map(|(_, n)| n).sum();
        assert!(
            total <= FLOOR,
            "the uniform block may need {total} fragment uniform vectors \
             ({arrays:?}) and GL 3.3 only guarantees {FLOOR}. Shrink the array, or \
             move it to a texture."
        );
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
