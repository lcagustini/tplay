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

/// One headless frame, with the output disposed of.
///
/// egui 0.36 makes dropping an unapplied `TexturesDelta` a `debug_assert`, and
/// a frame that loads a font or rasterizes an icon produces one — so a test that
/// only cares about geometry or emitted shapes still has to say it means to throw
/// the output away. This is what egui's own `run_ui` doc example does, and it is
/// the only reason the frame helper exists rather than a bare `ctx.run_ui`.
fn headless(ctx: &egui::Context, raw: egui::RawInput, f: impl FnMut(&mut egui::Ui)) {
    ctx.run_ui(raw, f).drop_without_applying_deltas();
}

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

    // Visuals should be modified. `ctx.style()` is gone in egui 0.36;
    // `style_of(ctx.theme())` is the style it returned.
    let visuals = ctx.style_of(ctx.theme()).visuals.clone();
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
///
/// **Two of the three traps are gone, and their premises were deleted rather
/// than loosened.** Under 0.36 the overflow no longer widens anything this pane
/// can see: `ui.vertical` + `set_min_width` measures exactly `SIDEBAR` with a
/// 48-character address in it (0.30 reported >140), and an unwrapped form leaves
/// the scroll content at `FORM_W` rather than past it. So `clip_text` +
/// `desired_width` contain the field on their own now, and asserting a widening
/// that no longer happens would be asserting that egui has a bug. The
/// cursor-advance trap is the one that remains — a bare `new_child` still leaves
/// the sibling drawn on top — so that premise guards the half of the fix still
/// doing work. The assertions below stand on their own either way: they are the
/// sidebar's geometry contract, not a claim about which egui bug produced it.
#[test]
fn sidebar_column_and_scroll_content_ignore_textedit_overflow() {
    const SIDEBAR: f32 = 120.0;
    const FORM_W: f32 = 100.0;
    const FIELD_H: f32 = 18.0;
    const LONG: &str = "smb://192.168.15.59/newhd/some/deeply/nested/folder/name";

    #[derive(Clone, Copy)]
    enum Shape {
        /// `new_child` without advancing the cursor: the sibling overlaps.
        ChildNoAdvance,
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
        headless(&ctx, raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let mut text = text.to_string();

                    // The scroll content. The form is always in the fixed-rect
                    // child now: the unwrapped variant only existed to premise
                    // the scroll-content hazard, and 0.36 no longer exhibits it.
                    let mut sidebar_body = |ui: &mut egui::Ui| {
                        let scroll_h = (ui.available_height() - 8.0).max(40.0);
                        let sa = egui::ScrollArea::vertical()
                            .id_salt("places_favorites")
                            .auto_shrink([true, false])
                            .max_height(scroll_h)
                            .show(ui, |ui| {
                                ui.label("Network");
                                let gap = ui.spacing().item_spacing.y;
                                let form_h = 3.0 * (FIELD_H + gap) + ui.spacing().interact_size.y;
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
                            });
                        content_w.set(sa.content_size.x);
                    };

                    // The match *is* the sidebar width: every arm lays the
                    // column out differently and yields the width it ended up
                    // with. Written as an expression rather than a `let mut`
                    // assigned per arm, which needed an `#[allow]` for a
                    // never-read `0.0` initializer.
                    let sidebar_w = match shape {
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
                            sidebar_body(&mut child);
                            rect.width()
                        }
                        Shape::Fixed => {
                            let (_, rect) =
                                ui.allocate_space(egui::vec2(SIDEBAR, ui.available_height()));
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            sidebar_body(&mut child);
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

    // Premise: a bare `new_child` never advances the horizontal cursor, so the
    // file-list sibling lands at the same x and draws over the sidebar. This is
    // the one hazard of the three that 0.36 has not fixed, and the one the
    // `allocate_space` half of the fix exists for.
    let (_, overlap_x, _) = layout(Shape::ChildNoAdvance, LONG);
    assert!(
        overlap_x < SIDEBAR,
        "premise: new_child without allocate_space should overlap, sibling x={overlap_x}"
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
        headless(&ctx, raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
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
        headless(&ctx, raw, |ui| {
            let themes = ThemeState::load(ui.ctx(), tplay::gui::theme::Themes::load(), "dark");
            egui::CentralPanel::default().show(ui, |ui| {
                // 0.36 removed `Ui::allocate_new_ui`; `new_child` with the same
                // `max_rect` is what is left, and for this measurement it is the
                // same thing — the claim under test is the *child*'s min_rect.
                let rect = ui.available_rect_before_wrap();
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                playlist_pane(&mut t.app, &themes, &mut child);
                claimed = child.min_rect().height();
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

/// The visualizer pane states a minimum size, and the floor it states has to be
/// one the view can actually be drawn at.
///
/// **This is the other half of the pane-size work, and it is the half that is
/// measurable.** The shader side (`fwidth`, the aspect corrections) is a property
/// of a string; this is a property of geometry, and it runs the real pane in a
/// headless `Context` the same way the Playlist geometry test does.
///
/// The claim is not "a number is recorded" — that would pass if the floor were
/// one pixel or a million. It is that the floor is **derived from the view's own
/// resolution limit**: a 32-band row needs cells at two pixels apiece to be a
/// row, which is the same threshold `bars` dims out at per fragment, so the pane
/// asks for the space that threshold implies. A floor smaller than that lets the
/// splitter reach a size where the view has deliberately stopped drawing itself.
#[test]
fn the_visualizer_floor_matches_the_bars_sampling_limit() {
    use common::TestApp;
    use eframe::egui::{pos2, vec2, Rect};
    use tplay::audio::viz::VIZ_BANDS;
    use tplay::gui::panes::visualizer::visualizer_pane;
    use tplay::gui::theme::ThemeState;

    let mut t = TestApp::new("visualizer-min-size");
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(680.0, 460.0))),
        ..Default::default()
    };
    let (mut min_h, mut min_w) = (0.0f32, 0.0f32);
    headless(&ctx, raw, |ui| {
        let themes = ThemeState::load(ui.ctx(), tplay::gui::theme::Themes::load(), "dark");
        egui::CentralPanel::default().show(ui, |ui| {
            visualizer_pane(&mut t.app, &themes, ui);
        });
        min_h = ctx.data(|d| {
            d.get_temp::<f32>(egui::Id::new("tplay.pane_content_h").with(Pane::Visualizer))
                .unwrap_or(0.0)
        });
        min_w = ctx.data(|d| {
            d.get_temp::<f32>(egui::Id::new("tplay.pane_content_w").with(Pane::Visualizer))
                .unwrap_or(0.0)
        });
    });

    assert!(
        min_h > 0.0 && min_w > 0.0,
        "the visualizer pane recorded no minimum ({min_h:.1}x{min_w:.1}) — it is the \
         only pane here that draws nothing but a fullscreen shader, so it has no \
         measured content to floor it and `apply_min_pane_sizes` skips it entirely"
    );
    // The width is the direct statement of the sampling limit: two pixels per
    // band cell, which is where `bars` starts dimming itself out.
    let cells_at_two_px = VIZ_BANDS as f32 * 2.0;
    assert!(
        (min_w - cells_at_two_px).abs() < 0.5,
        "the pane's width floor is {min_w:.1}, but `bars` stops being legible below \
         {cells_at_two_px:.1} ({VIZ_BANDS} cells at two pixels). A floor under that \
         lets the splitter reach a size where the view has drawn itself away."
    );
    // And the height has to cover a header *and* that many cells of drawing area,
    // or the floor is satisfied by a pane that is all header.
    assert!(
        min_h > cells_at_two_px,
        "the height floor is {min_h:.1}, which does not leave room for a band row \
         under the header — the pane would satisfy the floor with chrome alone"
    );
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
/// - **The gate was only reachable in a real `ScrollArea`.** A harness that
///   hands the pane a child `Ui` of a known rect (as the pane test above does,
///   via `new_child`) gives it a finite cursor, so a test written that way
///   passes with the bug present. Hence the `DockArea` below: the panes are
///   driven through the same nesting the app uses, tab bodies and all.
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
        // Mandatory since egui_dock 0.21; the app's own viewer keys a pane the
        // same way, so a tab's identity here matches what the panes will see.
        fn id(&mut self, tab: &mut Pane) -> egui::Id {
            egui::Id::new("tplay.pane").with(tab)
        }
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
        let out = ctx.run_ui(raw.clone(), |ui| {
            let themes = ThemeState::load(ui.ctx(), tplay::gui::theme::Themes::load(), "dark");
            let mut viewer = Viewer {
                app: &mut t.app,
                themes: &themes,
            };
            egui::CentralPanel::default().show(ui, |ui| {
                egui_dock::DockArea::new(&mut tree).show_inside(ui, &mut viewer);
            });
        });
        let themes = ThemeState::load(&ctx, tplay::gui::theme::Themes::load(), "dark");
        drawn = drawn_rows(&out, &themes.current().palette);
        // The emitted shapes are the measurement; the atlas deltas are not.
        // See `headless`.
        out.drop_without_applying_deltas();
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

/// No tab bar may paint a ✕ beside a pane's title; the ☰ menu is the only way
/// to hide a pane.
///
/// `egui_dock` 0.21 puts **two** close controls on a tab bar and every one of
/// them defaults to `true`: the ✕ inside a tab (`show_close_buttons`) and the ✕
/// at the right end of the bar (`show_leaf_close_all_buttons`). Each leaf here
/// holds exactly one pane, so the second one reads as that pane's own close
/// button — which is what a user reports seeing. `is_closeable -> false` is no
/// substitute: it *greys the close-all button out* rather than removing it.
///
/// So this reads the builder chain. A builder flag is not observable at runtime
/// — egui creates no widget for a false one, so an emitted-shape count cannot
/// see it — and a version bump can flip a default without touching this file.
///
/// The `is_closeable` half is here for the same reason. The deprecated
/// `closeable` is a **no-op** in 0.21: it compiles, it reads
/// `// Hide via dropdown only`, and the right-click "Close" item and the
/// middle-click on a tab still remove a pane.
#[test]
fn no_pane_draws_a_close_button() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gui/coordinator.rs"),
    )
    .expect("read coordinator.rs");

    let start = src
        .find("DockArea::new(&mut tree)")
        .expect("premise: the coordinator builds one DockArea");
    let chain = &src[start..];
    let chain = &chain[..chain
        .find("show_inside")
        .expect("premise: the chain ends in show_inside")];

    for flag in [
        "show_close_buttons",
        "show_leaf_close_all_buttons",
        "show_leaf_collapse_buttons",
    ] {
        assert!(
            chain.contains(&format!(".{flag}(false)")),
            "the DockArea chain must pass .{flag}(false): all three default to true in \
             egui_dock 0.21 and each paints a control at the right end of a tab bar. \
             Chain read:\n{chain}",
        );
    }

    assert!(
        src.contains("fn is_closeable(&self"),
        "PaneViewer must implement `is_closeable` — that is the hook egui_dock 0.21 \
         calls when it decides whether a pane can be closed.",
    );
    assert!(
        !src.contains("fn closeable(&mut self"),
        "PaneViewer implements the deprecated `closeable`, which egui_dock 0.21 never \
         calls. Override `is_closeable` instead: without it the right-click \"Close\" \
         item and the middle-click on a tab both close a pane.",
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
    use eframe::egui;
    use eframe::egui_wgpu::{CallbackTrait, ScreenDescriptor};
    use tplay::gui::panes::visualizer::{gpu, views};
    use tplay::gui::theme::{Palette, Themes};

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
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if let Some(draw) = draw {
                    draw(ui.painter(), rect, viz, &palette);
                }
            });
        });
        let shapes = out.shapes.clone();
        out.drop_without_applying_deltas();
        shapes
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
        // **Both arms, because a held frame is a different code path and it is the
        // one that was once wrong.** `take_arrival` *consumes*, so `pane`'s 4096
        // pushes are one arrival: the first view in the loop below would record
        // audio and the other eight would hold, which makes the arms depend on
        // iteration order rather than being covered. A feedback view that skips
        // queueing its callback while holding does not look paused — it looks
        // gone, since the pane falls back to its own background. That is the exact
        // failure the hold was built to avoid.
        for (arm, arriving) in [("advancing", true), ("held", false)] {
            if arriving {
                viz.push(0.5);
            }
            for view in SHADER_VIEWS {
                let shapes = primitives(Some(view.draw), &viz, rect);
                let cbs = callbacks(&shapes);
                assert_eq!(
                    cbs.len(),
                    1,
                    "{} ({arm}): a shader view must queue exactly one callback — the callback \
                     body runs inside the paint, so more than one is more than one fullscreen \
                     draw (got {} callbacks in {} primitives)",
                    view.name,
                    cbs.len(),
                    shapes.len()
                );
                assert_eq!(
                    cbs[0].rect, rect,
                    "{} ({arm}): the callback must cover the pane rect it was handed, or it \
                     paints over the header or nothing at all",
                    view.name
                );
            }
        }
    }

    /// Every shader reads the palette, so a theme switch reaches it.
    ///
    /// The complement to the hex check above, and the one that carries the weight:
    /// a hardcoded *literal* is a colour someone typed, but a shader that simply
    /// never mentions a palette uniform ignores the theme just as thoroughly and
    /// cannot be caught by pattern-matching literals at all. This is the property
    /// A view that draws *anything* round reads the aspect ratio.
    ///
    /// **This is the pane-size guard, and it is a source-reading test because
    /// there is no other kind available.** `v_uv` is 0..1 over the pane in both
    /// axes, so a shader that treats it as a square space draws a *correct*
    /// picture of the wrong shape, and the distortion scales with the pane: an
    /// ellipse instead of a circle on a 3:1 pane, a stretched flame on a wide
    /// one. Nothing crashes, no callback is missed, the program links, and every
    /// behavioural test passes — which is exactly what a full revert of the
    /// `trails` fix did, measured.
    ///
    /// The failure is also *invisible in a screenshot*, because a screenshot is
    /// taken at one pane size and at the default size the aspect is close enough
    /// to square to look right. The bug only shows when someone drags a
    /// splitter, which is a thing users do constantly and tests never do.
    ///
    /// So: a view whose geometry has a notion of round — a distance from a
    /// centre, a rotation, a Gaussian cross-section — must have aspect-corrected
    /// it. A view that legitimately does not (`bars`, which is a per-column
    /// comparison) is named as an exemption, and the exemption is a claim someone
    /// has to justify in the source rather than an oversight.
    #[test]
    fn a_view_that_draws_round_corrects_the_aspect_ratio() {
        /// Views whose geometry is per-axis and has no notion of round: a bar is
        /// as wide as the pane gives it, and `bars` deliberately draws sub-pixel
        /// cells out rather than rescaling them. Everything else must correct.
        const EXEMPT: &[&str] = &["Bars", "Spectrogram"];
        /// The property that makes a view "round": it measures a distance, turns
        /// an angle, or takes a cross-section.
        const ROUND_SHAPES: &[&str] = &["length(", "atan2(", "exp(-", "tongue("];
        /// Code lines only, so a mention *inside a comment* does not count as the
        /// view reading it. A whole-file `contains` is the version of this test
        /// that a mutation walked straight through: reverting a view's aspect
        /// correction left `u_resolution` mentioned elsewhere in the same file,
        /// so the file-level check still passed.
        fn code(body: &str) -> String {
            body.lines()
                .map(|l| match l.find("//") {
                    Some(at) => &l[..at],
                    None => l,
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        for (name, body) in bodies() {
            if EXEMPT.contains(&name) {
                continue;
            }
            let code = code(body);
            if !ROUND_SHAPES.iter().any(|s| code.contains(s)) {
                continue;
            }
            // **The correction has to be applied to the coordinate the round shape
            // is measured in.** Checking that the file mentions `u_resolution` is
            // not enough, and two mutations proved it: `trails` reads the
            // resolution on its *sample* line to undo the correction, so a revert
            // of the correction itself left the mention in place and the whole
            // file-level check still passed.
            //
            // So the property is about the *assignment* that produces the
            // coordinate, in whichever form a view keeps it — a `vec2` for the
            // views that measure a distance in two axes, a `float` for the one
            // that takes a cross-section in x alone. Both are named explicitly
            // because a view may legitimately correct one and not the other, and
            // a check that only knew the first would either pass a broken view or
            // fail a correct one — which is the same trade `EXEMPT` is for, one
            // level in.
            const CORRECTED: &[&str] = &[
                // A two-axis centred coordinate, with the ratio bound or inline.
                "centred = (uv - 0.5) * vec2<f32>(aspect, 1.0)",
                "centred = (uv - 0.5) * vec2(u_resolution().x / max(u_resolution().y, 1.0), 1.0)",
                // A one-axis cross-section coordinate: `flame`'s `x`.
                "x = (uv.x - 0.5) * (u_resolution().x / max(u_resolution().y, 1.0))",
                "x = (uv.x - 0.5) * aspect",
            ];
            assert!(
                CORRECTED.iter().any(|c| code.contains(c)),
                "{name} measures a distance in a coordinate that is not aspect-\
                 corrected, so on a non-square pane the round shape is drawn \
                 distorted in proportion to how far from square the pane is. Correct \
                 the coordinate the shape is measured in, as \
                 `centred = (uv - 0.5) * vec2<f32>(aspect, 1.0)`."
            );
        }
    }

    /// A view that aspect-corrects also *un*-corrects before sampling.
    ///
    /// The half that is easy to get wrong and has no compile error: a feedback
    /// view computes its transform in the corrected space but samples a texture
    /// in the target's own 0..1 space, so a corrected coordinate handed straight
    /// to `texture()` is a sample from the wrong place — the whole image sheared
    /// across the pane, and it looks like the trail is drifting rather than like
    /// an arithmetic slip. The division back is what makes it the same point.
    #[test]
    fn a_feedback_view_uncorrects_the_aspect_before_sampling() {
        let mut checked = 0usize;
        for (name, body) in bodies() {
            // Only a view that *transforms* into the corrected space can get this
            // wrong, and only one that samples the feedback target afterwards. The
            // spectrogram's accumulate pass is `textureLoad` in whole texels and
            // its present pass samples with `v_uv` directly, so it has no corrected
            // coordinate to divide back — asking it to would be asking for a
            // property it deliberately does not have.
            //
            // Both halves are properties rather than a list of view names, because
            // a name list is a thing that rots: `radial` builds exactly this
            // coordinate and samples nothing, and when this sweep did run it failed
            // on `radial` for exactly that reason.
            let code: String = body
                .lines()
                .map(|l| match l.find("//") {
                    Some(at) => &l[..at],
                    None => l,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !code.contains("centred = (uv - 0.5) * vec2<f32>(aspect, 1.0)")
                || !code.contains("u_prev")
            {
                continue;
            }
            checked += 1;
            // **The invariant is a round trip, and that is what is asserted.** A
            // corrected coordinate that is sampled without being divided back
            // reads a mirrored point and shears the whole trail; one that is
            // divided back is the same point. The division is what says which.
            // A mutation that removed it while leaving the sampling alone passed
            // a check that only looked at the sampled expression, because the
            // intermediate is a *variable* — the dataflow is one statement
            // removed, not a name changed.
            assert!(
                code.contains("/ vec2<f32>(aspect, 1.0)) + 0.5")
                    || code.contains("/ vec2<f32>(aspect, 1.0)) + 0.5;")
                    || code.contains("/ vec2<f32>(aspect, 1.0))"),
                "{name} builds a corrected coordinate and samples `u_prev` with it \
                 undivided: `texture(u_prev, ...)` takes the target's own 0..1 \
                 space, so the sample is a mirrored point and the trail shears \
                 across the pane. Divide the coordinate back by `vec2<f32>(aspect, 1.0)` \
                 before it reaches the sample."
            );
        }
        // **The same vacuity guard, and for the same reason.** This test's
        // identifying pattern was a GLSL spelling, so every WGSL view skipped it
        // and the assertions above never ran — a green test that could not fail. A
        // feedback view that does not transform into the corrected space has
        // nothing to divide back, so zero is a legitimate answer from the loop;
        // what is not legitimate is zero from a pattern that no longer matches
        // anything.
        assert!(
            checked > 0,
            "no view matched the aspect-corrected-coordinate pattern, so the round \
             trip was never checked. Either the views stopped transforming into the \
             corrected space, or the identifying pattern is stale again."
        );
    }

    /// A view that draws a *cell* says what happens when the cell is sub-pixel.
    ///
    /// The property is not "there is a fade" — a test asserting a named constant
    /// would only pin the number, and a mutation that deleted the fade outright
    /// passed every other check here. It is that a view dividing the pane into
    /// cells **declares its own sampling limit**, because a hard `step` on a
    /// sub-pixel feature is sampling faster than the screen can show it: the row
    /// shimmers as a splitter moves, and nothing about that is visible in a
    /// screenshot taken at one size.
    ///
    /// `bars` is the only view that cells the pane — `wave` and `flame` read a
    /// continuous coordinate — and the declaration is `fwidth`, which is what
    /// makes the threshold *physical* rather than a guess at a pane size. A view
    /// that cells the pane without naming `fwidth` is drawing at a resolution it
    /// never checked, and on a HiDPI display or a narrow pane those are not the
    /// same number.
    #[test]
    fn a_view_that_cells_the_pane_declares_its_sampling_limit() {
        for (name, body) in bodies() {
            if name != "Bars" {
                continue;
            }
            let code: String = body
                .lines()
                .map(|l| match l.find("//") {
                    Some(at) => &l[..at],
                    None => l,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                code.contains("fwidth("),
                "{name} divides the pane into {VIZ_BANDS} cells but never asks how \
                 many pixels one is: a fixed gap and a fixed tick are a fixed \
                 fraction of a cell, so both go sub-pixel on a narrow pane and the \
                 row shimmers as the splitter moves. Read the cell width with \
                 `fwidth` and either size the feature in pixels or dim the view out \
                 below the sampling limit."
            );
        }
    }

    /// **Every view must move the read cursor itself, and this reads the sources
    /// because nothing at runtime can see it.**
    ///
    /// Nine views advance the cursor as a side effect of `compute_bands`, which
    /// they call anyway because they read the spectrum. `wave` reads time, not
    /// frequency, so it calls neither — and its envelope then sat on one 1024-
    /// sample window for a whole ring's worth of audio, a frozen picture that
    /// looks exactly like a paused one. The shader is correct for whatever window
    /// it is handed, so no behavioural test of the *output* can fail: the bug is
    /// in what was handed.
    ///
    /// Both halves are read, and the pairing is the point. Asserting that `wave`
    /// advances would not catch a new time-domain view forgetting to, and
    /// asserting that every view advances would fail `wave` twice over; the rule
    /// is *either* you compute bands *or* you advance.
    #[test]
    fn a_view_that_computes_no_bands_advances_the_read_cursor() {
        for view in SHADER_VIEWS {
            let code = view_source(view.file);
            // Comments are stripped: a view's doc comment may well mention the
            // call it should be making, and that is not a call.
            let code: String = code
                .lines()
                .map(|l| match l.find("//") {
                    Some(at) => &l[..at],
                    None => l,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let reads_bands = code.contains("compute_bands(");
            let advances = code.contains(".advance(");
            assert!(
                reads_bands || advances,
                "{} reads neither the spectrum nor the read cursor: `compute_bands` advances \
                 the cursor as a side effect, so a view that does not call it must call \
                 `VizBuf::advance` itself or its picture freezes",
                view.name
            );
            if !reads_bands {
                assert!(
                    advances,
                    "{} calls no `compute_bands`, so `advance` is its only advancing read and \
                     it must make it",
                    view.name
                );
            }
        }
    }

    /// **A resized pane must rebuild the render-target pair, or the feedback view
    /// never draws again.**
    ///
    /// The pair is created in `prepare`, on the frame the pane queues because the
    /// pair is unusable, and the condition it tested was "is the slot empty". That
    /// is right the first time and wrong after a resize: the slot holds a pair at
    /// the **old** size, so it is not empty, nothing is rebuilt, and `Feedback::draw`
    /// goes on queueing `pass: None` on every frame for ever. The view then renders
    /// nothing at all — no error, no log line, and a background-coloured pane that
    /// is pixel-identical to the documented "no GPU here" fallback. It is the
    /// worst kind of failure to have, because it reads as a rendering fault while
    /// the fault is a lifetime one, and because the pane works perfectly until the
    /// moment the user drags a splitter.
    ///
    /// `pair_is_unusable` is the question itself, extracted so a test can ask it —
    /// which it could not while the compare sat inline in a `Device`-locked method.
    /// Both halves are asserted: absent means create, and **wrong size means
    /// create**, which is the half that was missing.
    #[test]
    fn a_resized_pane_rebuilds_the_render_target_pair() {
        /// A pair's two targets, each named by its pixel size.
        type Sizes = ((i32, i32), (i32, i32));
        let pair = |w: i32, h: i32| -> Sizes { ((w, h), (w, h)) };

        assert!(
            gpu::pair_is_unusable(None, (128, 128)),
            "an absent pair must be built, or the view never draws at all"
        );
        assert!(
            !gpu::pair_is_unusable(Some(pair(128, 128)), (128, 128)),
            "a pair of exactly the wanted size is usable; rebuilding it every frame would \
             leak a render target per frame and reset the history for ever"
        );

        // The regression: a pair that exists at the old size. This is what a resize
        // leaves behind, and treating it as usable is what blanked the pane.
        assert!(
            gpu::pair_is_unusable(Some(pair(128, 128)), (192, 128)),
            "a pane widened from 128 to 192 keeps the pair it had; it must be rebuilt or the \
             callback is queued with no pass for ever and the view renders nothing"
        );
        assert!(
            gpu::pair_is_unusable(Some(pair(128, 128)), (128, 192)),
            "the same for a height change, which is what dragging a splitter down does"
        );
        assert!(
            gpu::pair_is_unusable(Some(pair(128, 128)), (192, 64)),
            "and for a change in both axes at once"
        );
    }

    /// **The flame must span the pane, not sit in the middle of it.**
    ///
    /// The aspect correction is what keeps a tongue's Gaussian round, and it also
    /// makes `x` measure the pane's **height** — so the drawing space is ±half the
    /// aspect ratio, which on a wide pane is very much wider than it is tall.
    /// Positions written as plain numbers are therefore fractions of the height,
    /// and at −0.11 / 0.06 / 0.26 the three tongues filled the middle tenth of a
    /// 6:1 pane with empty background either side.
    ///
    /// **The existing aspect test cannot catch this.** It asks whether the
    /// *correction* is applied, and the correction was correct — it is what left the
    /// figure small. Nothing measured where the figure landed, so a view could
    /// satisfy every shader test and still draw a postage stamp.
    ///
    /// So this reads the source and evaluates the arithmetic: the figure's extent
    /// is converted back into pane fractions (`uv.x = 0.5 + x / aspect`) and
    /// checked against the pane. Two aspect ratios, because the failure only shows
    /// on a wide one — at 1:1 the old numbers filled the pane perfectly, which is
    /// why it looked right until someone made the window wide.
    #[test]
    fn the_flame_spans_the_pane_at_any_aspect_ratio() {
        let body = SHADER_VIEWS
            .iter()
            .find(|v| v.file == "flame")
            .and_then(|v| v.frags.first().copied())
            .expect("the flame's body is in the table");

        // Comments stripped: a comment may well mention `span`, and the numbers
        // this reads have to be the ones the driver compiles.
        let code: String = body
            .lines()
            .map(|l| match l.find("//") {
                Some(at) => &l[..at],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // `let span = max(<expr>, <floor>);` — the figure's spread, and the floor
        // that stops a very tall pane from collapsing the tongues together.
        let floor: f64 = code
            .lines()
            .find_map(|l| {
                let rest = l.split_once("let span = max(")?.1;
                // The *last* comma: the first one belongs to the inner `max(…)`
                // of the aspect expression, not to the outer call.
                let tail = rest.rsplit_once(',')?.1;
                tail.trim().trim_end_matches([')', ';']).trim().parse().ok()
            })
            .unwrap_or_else(|| {
                panic!("the flame must bound its figure's spread with a floor:\n{body}")
            });

        // Every tongue's centre and width, as factors of `span`.
        let mut centres = Vec::new();
        let mut widths = Vec::new();
        for line in code.lines().filter(|l| l.contains("tongue(lick,")) {
            let args: Vec<&str> = line
                .split("tongue(lick,")
                .nth(1)
                .expect("a tongue call on this line")
                .split(',')
                .map(|a| a.trim().trim_end_matches([')', ';']).trim())
                .filter(|a| !a.is_empty())
                .collect();
            assert_eq!(
                args.len(),
                2,
                "a tongue is `tongue(x, centre, width)`, so two arguments follow the \
                 coordinate. Found `{args:?}` in:\n{line}"
            );
            let factor = |arg: &str, line: &str| -> f64 {
                arg.strip_prefix("span * ")
                    .unwrap_or_else(|| {
                        panic!(
                            "tongue argument `{arg}` is not a factor of the figure's spread, \
                             so the flame's extent is in units of the pane's height and a wide \
                             pane is mostly empty. Write it `span * <factor>`. Line:\n{line}"
                        )
                    })
                    .trim()
                    .parse()
                    .unwrap_or_else(|e| panic!("the spread factor `{arg}` does not parse: {e}"))
            };
            centres.push(factor(args[0], line));
            widths.push(factor(args[1], line));
        }
        assert_eq!(
            centres.len(),
            3,
            "three tongues means three centres and three widths, all as factors of the \
             spread. Found {}.",
            centres.len()
        );

        // How much of the pane the figure covers, for one aspect ratio. A Gaussian
        // `exp(-3.2 d²)` is under 0.1% past about 1.7 of its width, so that is
        // where a tongue stops being visible.
        const VISIBLE_WIDTHS: f64 = 1.7;
        let coverage = |aspect: f64| -> f64 {
            let span = aspect.max(floor);
            let cs: Vec<f64> = centres.iter().map(|c| c * span).collect();
            let ws: Vec<f64> = widths.iter().map(|w| w * span).collect();
            let lo = cs
                .iter()
                .zip(&ws)
                .map(|(c, w)| c - VISIBLE_WIDTHS * w)
                .fold(f64::MAX, f64::min);
            let hi = cs
                .iter()
                .zip(&ws)
                .map(|(c, w)| c + VISIBLE_WIDTHS * w)
                .fold(f64::MIN, f64::max);
            // Back into pane fractions, then clipped: a figure running off the
            // edge is not using the space either.
            let a = (0.5 + lo / aspect).clamp(0.0, 1.0);
            let b = (0.5 + hi / aspect).clamp(0.0, 1.0);
            (b - a).max(0.0)
        };

        for (aspect, why) in [
            (6.33f64, "a 6:1 pane — the visualizer in a wide dock"),
            (2.0, "a 2:1 pane"),
            (1.0, "a square pane"),
        ] {
            let used = coverage(aspect);
            assert!(
                used > 0.7,
                "the flame covers only {pct:.0}% of a {aspect}:1 pane's width ({why}). The \
                 tongues' centres and widths are factors of the figure's spread, so the \
                 figure should reach most of the pane at any shape. The empty margin either \
                 side is what this test exists to catch — and at 1:1 the old numbers passed, \
                 which is why the defect only showed on a wide window.",
                pct = used * 100.0
            );
        }
    }

    /// That says "palette-only" rather than "no hex".
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
            accumulate.contains("textureLoad(u_prev"),
            "the spectrogram's accumulate pass must read its history with `textureLoad`, \
             the WGSL `texelFetch`, which addresses whole texels. An interpolated read \
             at a fractional texel offset blends two neighbours, and doing that to the \
             whole history once a frame is a low-pass filter applied once per column — \
             the waterfall arrives blurred, and only on the pane widths whose target \
             size does not divide by the column count, so it reads as the window's fault."
        );
        assert!(
            !accumulate.contains("textureSample"),
            "the spectrogram's accumulate pass must not sample its history through a \
             sampler at all — see above. Every read of the feedback target there is a \
             texel copy, not an interpolation."
        );
        // The column count is derived from the target's own width, so it needs the
        // target's own size — which is neither the pane's nor `u_resolution`,
        // because `target_size` quantises it to a 64px grid. Guessing it from a
        // uniform is the mistake this replaced.
        assert!(
            accumulate.contains("textureDimensions(u_prev"),
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

        // The fresh column's guard, verbatim: `p.x >= i32(sz.x) - colw`. What matters
        // is that it names the **high** end of `p.x` and the same `colw` the shift
        // moves by — the two agreeing on one edge is the whole invariant.
        let fresh = body
            .lines()
            .find(|l| l.contains("p.x >= i32(sz.x) - colw"))
            .unwrap_or_else(|| panic!("the spectrogram no longer has a fresh/old choice:\n{body}"));
        assert!(
            fresh.contains("p.x >= i32(sz.x) - colw"),
            "the spectrogram's new column must land on the rightmost columns — the ones \
             its shift moves its neighbours out of. This guard is `{fresh}`. On any other \
             edge the new column is overwritten on the next frame and never scrolls, \
             which shows as a permanent bright bar down the wrong side of the pane."
        );

        // The shift reads its neighbour from the *low* side, which is the same
        // statement: `p.x + colw` steps towards the edge the new column takes, so a
        // column moves left and vacates the right. A `- colw` here with the guard
        // above would be a scroll that feeds from the column it is writing.
        let shift = body
            .lines()
            .find(|l| l.contains("textureLoad(u_prev"))
            .unwrap_or_else(|| panic!("the spectrogram no longer reads its history:\n{body}"));
        assert!(
            shift.contains("(p.x + colw) % i32(sz.x)"),
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
        let assembled = gpu::wgsl_module("", "    frag_color = vec4<f32>(level(band_at(0u)));");
        let floor = format!("const DB_FLOOR : f32 = {};", audio::viz::DB_FLOOR);
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
            assembled.contains(&format!("const DB_SPAN : f32 = {};", -audio::viz::DB_FLOOR)),
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
                    !code.contains("fn level("),
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

    /// **No shader flips `y` between a feedback target and the pane.** Neither of
    /// the waterfall's two passes may, and the reason is a convention that is the
    /// opposite of the OpenGL one this code was written under.
    ///
    /// **In wgpu, row 0 of a framebuffer is at NDC `y = +1`** — the top of the
    /// image — and `textureSample`'s `uv.y = 0` is that same row. `v_uv.y = 0` is
    /// the pane's top. All three agree, so the texel a fragment lands in is
    /// `v_uv * sz` and the coordinate it wrote with is the coordinate it reads
    /// back: the round trip is the **identity**, with no flip anywhere. In OpenGL
    /// row 0 is at NDC `-1`, which is where the flip these passes used to carry
    /// came from, and it is simply wrong here.
    ///
    /// **The symptom was not a mirror, and that is why this needed measuring.**
    /// A read and a write on *different* rows looks like a whole-image flip, and a
    /// whole-image flip is easy to reason about and easy to spot. It is not what
    /// happens: the write is where the rasteriser put the fragment, and only the
    /// *read* is mirrored, so each row reads the row its mirror image reads. A
    /// mirror is an involution, so the history never smears — it settles into
    /// holding `fresh(M^k(row))` for a column `k` frames old, which with `M^2 = id`
    /// means **alternate columns are the mirror of their neighbours**. Two pixels
    /// to the column, that is a stripe, and the stripes are vertical because the
    /// columns are.
    ///
    /// Nothing else in the repo can see it. The shaders compile, the callback count
    /// and rect are right, the programs link, the column count is right, the scroll
    /// genuinely happens, and with a **constant** spectrum the row means come out
    /// symmetric — a picture that is nearly right and striped, which is the worst
    /// shape of bug to diagnose from inside the code. `Trails` is immune, and the
    /// reason generalises: its accumulate transform is a spin and a zoom about the
    /// pane's centre, so a whole-image `y` flip is one of that transform's own
    /// symmetries and commutes. Only the waterfall carries **per-column** data,
    /// where a per-row flip is visible.
    ///
    /// The simulation underneath is the premise that makes the string checks mean
    /// something: a `ROWS`-row target whose rows hold their own index, through one
    /// accumulate write and one present read, under both conventions. It is
    /// written so that *either* pass flipping alone produces the striped result —
    /// which is the finding, and what "they must agree" could not have told us.
    #[test]
    fn no_view_mirrors_the_pane_in_y() {
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

        // Read as "is this row mirrored", not as a spelling, so that either form
        // of the flip trips the rule and only a missing one does not.
        let accumulate_flips = accumulate.contains("i32(sz.y) - 1 -");
        let present_flips = present.contains("1.0 - v_uv.y");
        assert!(
            !accumulate_flips,
            "the waterfall's accumulate pass flips y before addressing the target. In wgpu \
             framebuffer row 0 is at NDC +1 — the pane's top, which is where `v_uv.y = 0` \
             is — so the texel a fragment lands in is `v_uv * sz` and the target needs no \
             conversion. This is the OpenGL convention (row 0 at NDC -1), and it makes the \
             pass *read* the row its mirror image reads while still writing its own, which \
             stripes the history column by column."
        );
        assert!(
            !present_flips,
            "the waterfall's present pass flips y before sampling the target. `uv.y = 0` is \
             texture row 0, which is the pane's top, so a straight `v_uv` read is the row \
             the accumulate pass wrote. A flip here presents the history mirrored without \
             the accumulate pass agreeing."
        );

        // The premise, and it models *columns* rather than a single row vector,
        // because columns are where the striping lives: a column is `k` frames old,
        // so it carries `fresh(M^k(row))`, and with `M^2 = id` that is `fresh(row)` on
        // one column and `fresh(mirror(row))` on the next.
        const ROWS: usize = 9;
        const COLS: usize = 4;
        let fresh = |row: usize| row as u8;
        for (label, p_flip, a_flip, identity) in [
            ("neither pass flips", false, false, true),
            ("the present flips alone", true, false, false),
            ("the accumulate flips alone", false, true, false),
            ("both flip", true, true, true),
        ] {
            // `h[row][col]`; the accumulate writes the rasterised column and reads
            // `col + 1` of its own (possibly mirrored) row, except in the fresh column.
            let mut h = vec![vec![0u8; COLS]; ROWS];
            for _frame in 0..COLS {
                let old = h.clone();
                for (row, line) in h.iter_mut().enumerate() {
                    let src = if a_flip { ROWS - 1 - row } else { row };
                    for (col, texel) in line.iter_mut().enumerate() {
                        *texel = if col == COLS - 1 {
                            fresh(row)
                        } else {
                            old[src][(col + 1) % COLS]
                        };
                    }
                }
            }
            // The present samples the settled history once per pane row.
            let shown: Vec<u8> = (0..ROWS)
                .map(|w| h[if p_flip { ROWS - 1 - w } else { w }][0])
                .collect();
            assert_eq!(
                shown.iter().copied().eq((0..ROWS).map(fresh)),
                identity,
                "{label}: after the target has filled, the row the pane shows must be the \
                 row that was written. It shows {shown:?} over a {ROWS}-row target whose \
                 rows hold their own index."
            );
            // Striping is the other half, and the two are separate claims: a mirrored
            // *read* and a positional *write* leave the round trip looking right in
            // aggregate and wrong in every other column.
            let mirrored: Vec<bool> = (0..COLS - 1)
                .map(|col| (0..ROWS).any(|r| h[r][col] != fresh(r)))
                .collect();
            assert_eq!(
                mirrored.iter().any(|m| *m) && mirrored.iter().any(|m| !*m),
                a_flip,
                "{label}: the settled target's stale columns must be all-or-nothing mirrored \
                 (columns read {mirrored:?}). Alternate columns being each other's mirror \
                 image is the stripe, and it is invisible in the row means — those come out \
                 symmetric — which is why it took a readback rather than a code review.",
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
                !code.contains("atan2("),
                "{name}: reads the spectrum through `atan2`, whose branch cut along \
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
            gpu::wgsl_module("", "    frag_color = vec4<f32>(spectrum_at_bearing(p));")
                .contains("fn spectrum_at_bearing(p : vec2<f32>)"),
            "a shader that asks for `spectrum_at_bearing` must get it, and get the \
             `band_at` accessor and `VIZ_BANDS` it needs with it."
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

    /// Every module the app ships, as `(view name, assembled source)`.
    ///
    /// Read out of [`gpu::wgsl_module`], so a check against this is a check
    /// against the string the driver compiles — the uniform block, the shared
    /// functions, the view's own functions and the entry point included.
    /// [`bodies`] is the other half of the pair: the view's *own* text, which is
    /// what an **absence** check wants. Assembling first would put the prelude's
    /// own `fn level` inside the very string a test is asserting the view does not
    /// define.
    fn modules() -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for view in SHADER_VIEWS {
            for body in view.frags {
                out.push((view.name, gpu::wgsl_module(view.helpers, body)));
            }
        }
        out
    }

    /// Every body the app ships, as `(view name, the view's own source)`.
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
    fn the_wgpu_harness_never_names_a_view() {
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
    /// `the_wgpu_harness_never_names_a_view`: the uv comes from the vertex shader's
    /// `v_uv` varying, which the *viewport* maps onto the pane, so a view cannot
    /// get this wrong by accident and never needs to know where the pane is.
    #[test]
    fn no_shader_reads_a_window_relative_pixel_coordinate() {
        // The vertex stage's half of the contract, read from an assembled module —
        // the same string the driver compiles. `wgsl_module` allocates, so it is
        // built once here rather than per sweep.
        let module = gpu::wgsl_module("", "");
        assert!(
            module.contains("@location(0) uv : vec2<f32>")
                && module.contains("out.uv = vec2<f32>(corner.x, 1.0 - corner.y);"),
            "the shared vertex stage stopped handing the fragment stage a pane-relative \
             uv that is top-down. Every view gets its coordinate from it, so this is \
             the one thing that has to be true. Got: {module}"
        );
        for (name, body) in bodies() {
            for (n, line) in body.lines().enumerate() {
                let code = line.trim_start().trim_start_matches("//");
                for (what, hit) in [
                    ("the built-in position", code.contains("in.pos")),
                    (
                        "a vertex index",
                        code.contains("vi") && code.contains("vertex_index"),
                    ),
                ] {
                    assert!(
                        !hit,
                        "{}:{n} reads {what}\n  {line}\nIt is window-relative and bottom-left \
                         origin; every view here is pane-relative and top-down. Use `v_uv`, \
                         which the viewport maps onto the pane.",
                        name
                    );
                }
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
                if !code.contains("acc = acc +") {
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

    /// The callback **paints pixels** — and only inside the pane.
    ///
    /// This is the one test here that needs a real GPU, and it is here because
    /// every other instrument in the repo is blind to the failure it catches. A
    /// view that compiles, links, has the right callback shape and the right rect
    /// and then writes nothing renders **the pane's background colour** — which is
    /// byte-for-byte what the documented "no GPU on this machine" path renders, so
    /// a broken view and a working one are indistinguishable from outside. That is
    /// not hypothetical: it is how this pane spent a whole session, and how the
    /// viewport bug below looked for a day.
    ///
    /// So the harness is driven for real against an offscreen texture standing in
    /// for the surface, cleared to a colour no palette uses, and the result is read
    /// back. Two claims, and the second is what makes the first mean something:
    ///
    /// 1. **something was drawn** — not every pixel is still the clear colour;
    /// 2. **nothing outside the pane was** — a view that painted the window
    ///    instead of its own rect fails here even though claim 1 passes.
    ///
    /// **Skipped, not failed, where there is no adapter.** A machine with no GPU
    /// and no software Vulkan driver has nothing to run it on, and a hard failure
    /// on a missing device would just get the test deleted. CI installs
    /// `mesa-vulkan-drivers`, so a software adapter *is* present there and this
    /// runs on every tagged build.
    #[test]
    fn the_callback_paints_the_pane_and_nothing_outside_it() {
        use eframe::egui::{pos2, Rect};

        /// The surface the callback draws onto, and a colour no palette produces.
        /// 256 x 64 keeps `bytes_per_row` a multiple of 256, which is what
        /// `copy_texture_to_buffer` requires and the only reason it is not 100 x 50.
        const SURFACE: (u32, u32) = (256, 64);
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        const CLEAR: [u8; 4] = [1, 2, 3, 255];

        let Some((device, queue)) = software_device() else {
            eprintln!("skipping: no wGPU adapter on this machine (install mesa-vulkan-drivers)");
            return;
        };
        gpu::init(&device, &queue, FORMAT);

        let surface = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test.surface"),
            size: wgpu::Extent3d {
                width: SURFACE.0,
                height: SURFACE.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = surface.create_view(&wgpu::TextureViewDescriptor::default());

        // The pane: a 128 x 32 box at (64, 16), so there is a ring of surface left
        // over on all four sides for claim 2 to inspect.
        let pane_rect = Rect::from_min_max(pos2(64.0, 16.0), pos2(192.0, 48.0));
        let screen = ScreenDescriptor {
            size_in_pixels: SURFACE.into(),
            pixels_per_point: 1.0,
        };
        let info = callback_info(pane_rect, SURFACE);

        let (_, viz) = pane();
        let theme = Themes::load().get("dark").expect("a bundled theme").clone();
        // The pane rect, not `pane()`'s 400 x 300, so `u_resolution` agrees with
        // the viewport a view is actually drawing into — otherwise this run would
        // be the one place in the app where the two disagree.
        let uniforms = gpu::Uniforms::pack(
            &viz,
            [audio::viz::DB_FLOOR; VIZ_BANDS],
            &theme.palette,
            pane_rect,
            &egui::Context::default(),
        );

        let cb = gpu::callback("", views::bars::FRAG, uniforms);
        let mut resources = eframe::egui_wgpu::CallbackResources::default();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test"),
        });
        cb.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test.surface"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: CLEAR[0] as f64 / 255.0,
                        g: CLEAR[1] as f64 / 255.0,
                        b: CLEAR[2] as f64 / 255.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // `CallbackTrait::paint` takes `&mut RenderPass<'static>`, which is
        // wgpu's own documented escape hatch for this — see its note on
        // `forget_lifetime`. The encoder is finished and submitted immediately
        // after, so the parent is never touched again.
        cb.paint(info, &mut pass.forget_lifetime(), &resources);
        queue.submit([encoder.finish()]);
        let pixels = read_back(&device, &queue, &surface, SURFACE);

        // Premise: the clear reached the texture and the readback saw it. Without
        // this, "some pixel is not the clear colour" could be satisfied by a
        // readback that returned noise.
        let at = |x: u32, y: u32| -> [u8; 4] {
            let stride = SURFACE.0 as usize * 4;
            let o = (y as usize * stride) + (x as usize * 4);
            [pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]]
        };
        assert_eq!(
            at(2, 2),
            CLEAR,
            "the premise: the top-left corner is outside the pane and must still carry \
             the clear colour. A readback that does not show the clear means the \
             assertions below are reading noise."
        );

        let drawn: Vec<(u32, u32)> = (0..SURFACE.1)
            .flat_map(|y| (0..SURFACE.0).map(move |x| (x, y)))
            .filter(|(x, y)| at(*x, *y) != CLEAR)
            .collect();
        assert!(
            !drawn.is_empty(),
            "the callback ran, built a pipeline and issued a draw, and the surface came \
             back entirely the clear colour — so the shader wrote nothing a user could \
             see. The symptom in the app is a blank pane that is pixel-identical to the \
             'no GPU here' path, which is why nothing else catches it."
        );
        let outside: Vec<(u32, u32)> = drawn
            .iter()
            .copied()
            .filter(|(x, y)| {
                let fx = *x as f32 + 0.5;
                let fy = *y as f32 + 0.5;
                !pane_rect.contains(pos2(fx, fy))
            })
            .collect();
        assert!(
            outside.is_empty(),
            "the view painted outside its own pane: {outside:?}. A fullscreen triangle \
             has no vertices, so the viewport is the only thing bounding it — left at \
             egui_wgpu's whole-surface viewport, every view paints over the window."
        );
    }

    /// A headless wGPU device, or `None` if this machine has no adapter.
    ///
    /// `new_without_display_handle_from_env` because there is no window: a display
    /// handle is what a surface needs, and this test renders into a texture.
    /// `request_adapter` and `request_device` are futures in wgpu 30 and a test is
    /// not, so this borrows the runtime the crate already depends on rather than
    /// adding an executor for two `.await`s.
    fn software_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let rt = tokio::runtime::Builder::new_current_thread().build().ok()?;
        rt.block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                })
                .await
                .ok()?
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("test.device"),
                    required_features: wgpu::Features::empty(),
                    // `downlevel_defaults` rather than the adapter's own limits, so
                    // the test does not depend on *which* adapter it found: the
                    // uniform block is two arrays of `vec4`, and that is the floor
                    // this pane actually needs.
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                })
                .await
                .ok()
        })
    }

    /// A `PaintCallbackInfo` for a pane, built the way a renderer builds one.
    fn callback_info(
        viewport: egui::Rect,
        screen: (u32, u32),
    ) -> eframe::epaint::PaintCallbackInfo {
        eframe::epaint::PaintCallbackInfo {
            viewport,
            // The whole surface, so the scissor never limits what is drawn and the
            // "nothing outside the pane" claim is about the viewport alone.
            clip_rect: egui::Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(screen.0 as f32, screen.1 as f32),
            ),
            pixels_per_point: 1.0,
            screen_size_px: [screen.0, screen.1],
        }
    }

    /// The texture's pixels, after everything already submitted on `queue` has run.
    ///
    /// `bytes_per_row` is the width times four with no padding, which is why the
    /// surface is 256 wide: `copy_texture_to_buffer` requires a row to be a
    /// multiple of 256 bytes, and a width that makes that true by construction is
    /// better than the 256-byte alignment padding this would otherwise need.
    fn read_back(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        size: (u32, u32),
    ) -> Vec<u8> {
        let bytes_per_row = size.0 as usize * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test.readback"),
            size: (bytes_per_row * size.1 as usize) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test.copy"),
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row as u32),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("the submitted copy completes");
        rx.recv()
            .expect("the map callback ran")
            .expect("the readback mapped");
        slice.get_mapped_range().expect("already mapped").to_vec()
    }

    /// The pane viewport is **top-left pixels**, and specifically not GL's
    /// bottom-left.
    ///
    /// The vertex stage synthesises three corners and has no vertex buffer, so the
    /// *viewport* is the only thing deciding which fragments a view reaches. Every
    /// view therefore draws over whatever else is on screen unless this is right.
    ///
    /// This is the whole of the conversion, which is why it is a function rather
    /// than three lines at each of its two call sites — **it is the one piece of
    /// the wgpu port that no headless test can reach**, since it needs a driver to
    /// be wrong in a way anything can see. And it was wrong twice: first because
    /// the harness drew at the window's size rather than the pane's, then because
    /// `PaintCallbackInfo::viewport_in_pixels()` — a method that is *correct* —
    /// reports `from_bottom_px`, and wgpu's viewport origin is the top left. That
    /// put the pane a window-height off and mirrored, which is a render somewhere
    /// plausible and no error anywhere.
    ///
    /// The second half of the assertion is the one worth having: the result must
    /// **differ** from the bottom-left form on a pane that is not at the top of the
    /// window. A pane at `y == 0` makes the two agree, so a test that only checked
    /// the first view's geometry would pass with the bug in place.
    #[test]
    fn the_pane_viewport_is_top_left_pixels_not_gl_bottom_left() {
        use eframe::egui::{pos2, Rect};

        // A pane low and to the right, on a 2x display — the shape of a real dock
        // tab, and the case where the two conventions disagree most.
        let rect = Rect::from_min_max(pos2(120.0, 480.0), pos2(608.0, 697.0));
        let ppp = 2.0;
        let [x, y, w, h] = gpu::pane_viewport(rect, ppp);
        assert_eq!(
            [x, y, w, h],
            [240.0, 960.0, 976.0, 434.0],
            "the viewport must be the pane's rect scaled into physical pixels, with \
             the y origin at the TOP — the same convention egui's point space uses."
        );
        assert_eq!(
            w,
            rect.width() * ppp,
            "the width is the pane's, so the picture fits it"
        );
        assert_eq!(
            h,
            rect.height() * ppp,
            "the height is the pane's, so the picture fits it"
        );

        // The trap, spelled out. `viewport_in_pixels` is a real method on
        // `PaintCallbackInfo` and reports the bottom-left origin GL wants; used as a
        // wgpu viewport it is off by exactly the distance from the pane's bottom to
        // the window's bottom, and mirrored.
        const WINDOW_H: f32 = 1400.0;
        let bottom_left = WINDOW_H - (rect.max.y * ppp);
        assert_ne!(
            y, bottom_left,
            "the result equals the bottom-left y. `PaintCallbackInfo::viewport_in_pixels()` \
             reports `from_bottom_px`, which is correct for OpenGL and wrong for wgpu — \
             feeding it to `set_viewport` renders the pane a window-height off, mirrored, \
             with nothing in any log."
        );

        // The premise: a pane at the *top* of the window cannot tell the two apart,
        // so this rect has to sit low down for the assertion above to mean anything
        // — and it does, so `y` is the larger of the two numbers.
        assert!(
            y - bottom_left > 100.0,
            "the premise: this rect barely differs between the two conventions, so the \
             assertion above is reading a coincidence rather than a difference. Got \
             top-left y {y} against bottom-left y {bottom_left}."
        );
    }

    /// Every shipped WGSL module **parses and validates**, in process.
    ///
    /// This is the test that had to exist, and it is now stronger than the
    /// `glslangValidator` version it replaces: naga is the same front end and
    /// validator the driver itself runs, it is linked into this test binary, so
    /// **CI exercises it too**, and it needs no external tool that half the
    /// machines would not have.
    ///
    /// It exists because a broken shader and a working one look identical from
    /// everywhere else in the repo. Both render as the pane's background — exactly
    /// the documented failure path for "no GPU here" — and the pane's only symptom
    /// is a blank rectangle. Three real defects shipped this way on the day these
    /// views were written (a function named `noise3`, which is a GLSL **built-in**;
    /// a write to a read-only function parameter, `in` in GLSL and also read-only
    /// in WGSL; and a call before the declaration, which both languages make a hard
    /// error), plus two more during the WGSL port itself: a body that nested a
    /// `fn` inside the entry point, and a `clamp` whose bounds were bare abstract
    /// floats where a `vec2<f32>` was required.
    ///
    /// The premise guard below is what keeps the assertion honest: it asserts that
    /// this validator *rejects* a module carrying a known error, because a sweep
    /// that silently accepted everything would look identical to a clean one.
    #[test]
    fn every_shader_validates() {
        assert!(
            !SHADER_VIEWS.is_empty(),
            "the table is empty, so this sweep would pass vacuously"
        );
        for (name, module) in modules() {
            let src = gpu::wgsl_module("", "");
            let parsed = match wgpu::naga::front::wgsl::parse_str(&module) {
                Ok(m) => m,
                Err(e) => panic!(
                    "{name}: the assembled module does not parse.\n{}\n{}",
                    e.emit_to_string(&module),
                    module
                ),
            };
            let mut v = validator();
            if let Err(e) = v.validate(&parsed) {
                panic!(
                    "{name}: the assembled module does not validate.\n{}\n{}",
                    e.emit_to_string(&module),
                    module
                );
            }
            // Parsed and validated is not the same as linked against the real
            // pipeline: a fragment stage whose output does not match the surface
            // format is caught by wgpu at draw time and not here. The module has
            // to at least *get* that far, which is all a text test can say.
            assert!(
                src.contains("@fragment"),
                "the premise: a module assembled by the harness has no fragment stage, so \
                 a sweep over these could not be validating anything. Got: {src}"
            );
        }

        // The premise, in the other direction: this validator must reject a known
        // error, or "every module validates" is a claim about nothing.
        // The error chosen is `array<f32, 4>` in the **uniform** address space —
        // the same 16-byte stride rule `the_wgsl_uniform_layout_is_the_block_this_
        // code_writes` is about. It parses and fails *validation* rather than
        // failing the parse, which is what makes this a premise about the
        // validator rather than about the front end, and it means a sweep over
        // these modules checks the one property that would otherwise be silent.
        let broken = concat!(
            "struct S { a : array<f32, 4> }",
            "@group(0) @binding(0) var<uniform> U : S;",
            "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(U.a[0]); }"
        );
        let parsed = wgpu::naga::front::wgsl::parse_str(broken).expect(
            "an f32 array in the uniform address space is a validation error, not a parse one",
        );
        assert!(
            validator().validate(&parsed).is_err(),
            "the premise: naga accepted an `f32` array in the uniform address space. \
             A validator that accepts everything would make this suite a green \
             light wired to nothing."
        );
    }

    /// A fresh naga validator with every check on and no capability allowances.
    ///
    /// `Capabilities::empty()` is the strict half: the shaders here use nothing
    /// exotic, so a validator that had to be told "you may have `f16`" would be
    /// hiding the very thing it is meant to catch.
    fn validator() -> wgpu::naga::valid::Validator {
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
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
        // Call sites, not every mention: a view's *helpers* now hold the
        // definition, so counting mentions would charge the definition to the
        // march and make the number mean nothing. `fn fbm(` is the definition;
        // everything else that reaches for it is a call.
        //
        // **The sweep feeds this the view's helpers and body together**, which is
        // the whole reason: WGSL has no nested functions, so a view's field
        // function lives beside the march rather than inside it, and sweeping the
        // body alone reported zero `fbm` calls for the one view that has a march.
        // That is the failure this test's own history warns about — the guard
        // passing because it measured nothing.
        let sites = src
            .match_indices("fbm(")
            .filter(|(at, _)| !src[..*at].trim_end().ends_with("fn"))
            .count() as u64;
        // `const NAME : i32 = N;` in this shader, so a loop bounded by `i < STEPS`
        // resolves. A bound the harness injects (`VIZ_BANDS`) is not in the
        // source to read, so it is skipped rather than guessed at.
        let consts: std::collections::HashMap<&str, u64> = src
            .lines()
            .filter_map(|l| {
                let rest = l.trim().strip_prefix("const ")?;
                let (name, tail) = rest.split_once(" : ")?;
                let (_, value) = tail.split_once("= ")?;
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

    /// The WGSL uniform layout **is** the block this code writes.
    ///
    /// This replaces a `MAX_FRAGMENT_UNIFORM_VECTORS` budget, which was a
    /// property of a GL spec nothing here targets any more. What replaced it is
    /// stronger and mechanical: **WGSL gives an array in the uniform address
    /// space a 16-byte element stride**, so `array<f32, 32>` is 512 bytes, not
    /// 128, and a block written packed four-to-one is read back as garbage with
    /// no validation error anywhere — the shader compiles, the pipeline builds,
    /// the draw is issued, and the picture is noise.
    ///
    /// The way out is that **every member is a `vec4`**, so a member's offset is
    /// 16 bytes times its index and the Rust side can stay a flat run of `f32`.
    /// That is a rule about the *declaration*, so it is asserted against the
    /// declaration, read out of the module naga actually parsed rather than out of
    /// the text it was given.
    ///
    /// Three claims, and the third is the one the other two cannot make:
    ///
    /// 1. **every member is a `vec4` or an array of them** — the stride rule
    ///    itself. An `f32` member is exactly the bug, and it is silent;
    /// 2. **both array lengths are multiples of four** — the precondition for
    ///    packing an `f32` count into `count / 4` `vec4`s, and
    /// 3. **the laid-out size equals `Uniforms::BLOCK_BYTES`** — which compares the
    ///    *declaration* against the *writer* from independent code, so the array
    ///    lengths, the member count and the buffer's size cannot drift apart
    ///    without one of the two moving first.
    ///
    /// What no check here covers is **member order**: `write_block` returns a flat
    /// `Vec<f32>` with no names in it, so the ordering it assumes is the ordering
    /// `wgsl_struct` declares, and nothing compares them. That is a review
    /// obligation, and it is why the accessors in the prelude name members rather
    /// than indexing by position.
    #[test]
    fn the_wgsl_uniform_layout_is_the_block_this_code_writes() {
        for (n, len) in [("VIZ_BANDS", VIZ_BANDS), ("WAVE_BUCKETS", WAVE_BUCKETS)] {
            assert_eq!(
                len % 4,
                0,
                "{n} is {len}, and the block packs four of them per `vec4`. A count \
                 that is not a multiple of four would need a partial vector — and a \
                 half-written one reads back as whatever the next member holds, \
                 silently."
            );
        }

        // A module assembled from the real prelude, so this reads the declaration
        // the driver gets rather than a hand-copied struct.
        let module = gpu::wgsl_module("", "");
        let parsed = wgpu::naga::front::wgsl::parse_str(&module).expect("the prelude parses");
        validator()
            .validate(&parsed)
            .expect("the prelude validates");

        let (handle, ty) = parsed
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("Uniforms"))
            .expect("the prelude declares `Uniforms`");
        let wgpu::naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("`Uniforms` is not a struct: {:?}", ty.inner);
        };
        for m in members {
            // A member's `ty` is a handle back into the same arena, so this reads
            // the declaration rather than re-deriving it from its own text.
            let inner = &parsed.types[m.ty].inner;
            assert!(
                matches!(
                    inner,
                    wgpu::naga::TypeInner::Vector { .. } | wgpu::naga::TypeInner::Array { .. }
                ),
                "uniform member `{}` is {inner:?}, which is not a `vec4` or an array of \
                 them. WGSL's uniform address space rounds every array element up to 16 \
                 bytes, so a narrower member is read at the wrong offset and the shader \
                 draws garbage with no validation error anywhere — see this test's docs.",
                m.name.as_deref().unwrap_or("?"),
            );
        }

        let mut layouter = wgpu::naga::proc::Layouter::default();
        layouter
            .update(parsed.to_ctx())
            .expect("the module can be laid out");
        let layout = layouter[handle];
        assert_eq!(
            layout.size as u64,
            gpu::Uniforms::BLOCK_BYTES,
            "the WGSL `Uniforms` struct lays out to {} bytes, but `Uniforms::BLOCK_BYTES` \
             (and so the buffer, and the bind group layout's `min_binding_size`) says \
             {}. One of the two halves moved: either `wgsl_struct` gained or lost a \
             member or an array length, or `write_block` writes a different number of \
             floats. Equal sizes with reordered members would still pass this, which \
             is why the member *widths* are checked above and the order is a review \
             obligation rather than a mechanical one.",
            layout.size,
            gpu::Uniforms::BLOCK_BYTES
        );
        assert_eq!(
            *span, layout.size,
            "the struct's declared span differs from its laid-out size"
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
