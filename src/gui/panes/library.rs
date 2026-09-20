//! Library pane — file browser + favorite folders + tag-scanning track list.
//!
//! Left: favorite folders. Right: the current folder's subfolders and audio
//! files, each row showing title + artist/album/year/genre + duration from
//! the tag scan (cached in `TPlayApp`, so revisits are instant). Click a row
//! to play it directly; the `+` button adds it to the playlist.

use crate::app::TPlayApp;
use crate::gui::theme;
use crate::library;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// egui memory: has the pane listed `library_dir` at least once this session.
const LIB_INIT: &str = "tplay.library.init";
/// egui memory: the active search filter text.
const LIB_QUERY: &str = "tplay.library.query";

/// Leaf name of a dir (`/` at the filesystem root).
fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

fn fmt_duration(d: Option<Duration>) -> String {
    d.map(TPlayApp::fmt_duration).unwrap_or_else(|| "--:--".into())
}

/// Column widths shared by the sortable header and the file rows.
const CELL_ARTIST: f32 = 90.0;
const CELL_ALBUM: f32 = 100.0;
const CELL_YEAR: f32 = 34.0;
const CELL_GENRE: f32 = 64.0;
const CELL_DUR: f32 = 44.0;
/// Fixed right-hand width per row: the five tag cells + the `+` button +
/// inter-cell spacing, leaving the title column the flexible remainder.
const ROW_FIXED_W: f32 = CELL_ARTIST + CELL_ALBUM + CELL_YEAR + CELL_GENRE + CELL_DUR + 40.0;

/// One clickable column header. Returns true when clicked; the caller picks
/// the sort key. The active column is accent-colored with a ▾/▴ direction
/// hint; clicking it again flips the direction in `TPlayApp::set_library_sort`.
fn header_cell(
    ui: &mut egui::Ui,
    p: theme::Palette,
    label: &str,
    active: bool,
    asc: bool,
    w: f32,
    row_h: f32,
) -> bool {
    let text = if active {
        format!("{label} {}", if asc { "▾" } else { "▴" })
    } else {
        label.to_string()
    };
    ui.add_sized(
        egui::vec2(w, row_h),
        egui::Button::new(
            egui::RichText::new(text)
                .small()
                .color(if active { p.accent } else { p.text_secondary }),
        )
        .frame(false),
    )
    .on_hover_text("Sort by this column")
    .clicked()
}

/// Cell background + interplay used for every library row.
fn row_bg(ui: &mut egui::Ui, i: usize, p: theme::Palette, row_h: f32) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, if i % 2 == 0 { p.row_even } else { p.row_odd });
    rect
}

fn draw_dir_row(ui: &mut egui::Ui, theme: &theme::Theme, row_h: f32, i: usize, name: &str) -> bool {
    let p = theme.palette;
    let rect = row_bg(ui, i, p, row_h);
    ui.painter().text(
        rect.min + egui::vec2(6.0, row_h / 2.0),
        egui::Align2::LEFT_CENTER,
        format!("📁  {name}"),
        egui::FontId::new(12.0, theme.metadata_font.clone()),
        p.text_secondary,
    );
    let resp = ui.interact(rect, egui::Id::new("lib_dir").with(i), egui::Sense::click());
    resp.clicked()
}

/// A saved playlist file row: full filename (extension visible) + a
/// right-aligned "Playlist" tag — no tag cells, no `+`. Click loads it into
/// the playlist pane (the caller adds the replace-confirm).
fn draw_playlist_row(ui: &mut egui::Ui, theme: &theme::Theme, row_h: f32, i: usize, path: &Path) -> bool {
    let p = theme.palette;
    let rect = row_bg(ui, i, p, row_h);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(rect.min + egui::vec2(6.0, 0.0), rect.max))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 4.0;

    let title_resp = row
        .add_sized(
            egui::vec2((rect.width() - ROW_FIXED_W - 6.0).max(40.0), row_h),
            egui::Label::new(
                egui::RichText::new(&name).color(p.text_primary.gamma_multiply(0.85)),
            )
            .truncate()
            .sense(egui::Sense::click()),
        )
        .on_hover_text("Load playlist");

    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new("Playlist").small().color(p.accent));
    });

    title_resp.clicked()
}

fn draw_file_row(
    app: &TPlayApp,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    row_h: f32,
    i: usize,
    path: &Path,
    info: Option<&library::TrackInfo>,
) -> Option<fn(&mut TPlayApp, PathBuf)> {
    let p = theme.palette;
    let rect = row_bg(ui, i, p, row_h);
    let is_current = app.current_path().is_some_and(|c| c == path);
    if is_current {
        ui.painter().rect_filled(rect, 2.0, p.accent.gamma_multiply(0.15));
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            0.0,
            p.accent,
        );
    }

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = info.and_then(|i| (!i.title.is_empty()).then_some(i.title.clone())).unwrap_or(stem);
    let display = match info.and_then(|i| i.track_no.clone()) {
        Some(n) => format!("{n}. {title}"),
        None => title,
    };
    let artist = info.map(|i| i.artist.as_str()).unwrap_or_default();
    let album = info.map(|i| i.album.as_str()).unwrap_or_default();
    let year = info.and_then(|i| i.year.as_deref()).unwrap_or_default();
    let genre = info.map(|i| i.genre.as_str()).unwrap_or_default();
    let dur = info.and_then(|i| i.duration);

    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(rect.min + egui::vec2(6.0, 0.0), rect.max))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 4.0;

    // Title takes the flexible remainder; the tag cells mirror the header.
    let title_w = (rect.width() - ROW_FIXED_W - 6.0).max(40.0);
    let title_resp = row
        .add_sized(
            egui::vec2(title_w, row_h),
            egui::Label::new(
                egui::RichText::new(&display).color(if is_current { p.text_primary } else { p.text_primary.gamma_multiply(0.85) }),
            )
            .truncate()
            .sense(egui::Sense::click()),
        )
        .on_hover_text("Play");

    let cell = |ui: &mut egui::Ui, w: f32, text: &str| {
        ui.add_sized(
            egui::vec2(w, row_h),
            egui::Label::new(egui::RichText::new(text).color(p.text_secondary)).truncate(),
        );
    };
    cell(&mut row, CELL_ARTIST, artist);
    cell(&mut row, CELL_ALBUM, album);
    cell(&mut row, CELL_YEAR, year);
    cell(&mut row, CELL_GENRE, genre);
    row.add_sized(
        egui::vec2(CELL_DUR, row_h),
        egui::Label::new(
            egui::RichText::new(fmt_duration(dur))
                .color(p.text_secondary)
                .font(egui::FontId::new(12.0, theme.metadata_font.clone())),
        ),
    );
    let add_clicked = row
        .add(egui::Button::new("+").small().frame(false))
        .on_hover_text("Add to playlist")
        .clicked();

    if title_resp.clicked() {
        Some(TPlayApp::play_file)
    } else if add_clicked {
        Some(|app: &mut TPlayApp, path: PathBuf| app.add_files(vec![path]))
    } else {
        None
    }
}

pub fn library_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let theme = app.theme().clone();
    let p = theme.palette;

    // First frame this session: list the saved/current dir and start the scan.
    if !ui.ctx().memory_mut(|m| m.data.get_temp::<bool>(egui::Id::new(LIB_INIT)).unwrap_or(false)) {
        ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_INIT), true));
        app.navigate_to(app.library_dir().to_path_buf());
    }

    // Header: current dir + favorite toggle (+ Add All on the right).
    ui.horizontal(|ui| {
        let dir = app.library_dir().to_path_buf();
        ui.label(
            egui::RichText::new(dir_name(&dir))
                .color(p.text_secondary)
                .font(egui::FontId::new(12.0, theme.metadata_font.clone())),
        );
        let fav = app.is_favorite(&dir);
        let fav_btn = egui::Button::new(if fav { "★" } else { "☆" }).selected(fav);
        if ui.add(fav_btn).on_hover_text("Favorite folder").clicked() {
            app.toggle_favorite(dir);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Add All")
                .on_hover_text("Add this folder's tracks to the playlist")
                .clicked()
            {
                let files: Vec<PathBuf> = app
                    .library_entries()
                    .iter()
                    .filter(|e| !e.is_dir() && !library::is_playlist(e.path()))
                    .map(|e| e.path().to_path_buf())
                    .collect();
                app.add_files(files);
            }
        });
    });

    // Search filter over the current folder's rows.
    let mut q = ui
        .ctx()
        .memory_mut(|m| m.data.get_temp::<String>(egui::Id::new(LIB_QUERY)).unwrap_or_default());
    let query = {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Search").small().color(p.text_secondary));
            ui.add(
                egui::TextEdit::singleline(&mut q)
                    .hint_text("title, artist, album…")
                    .desired_width(220.0),
            );
        });
        q.trim().to_lowercase()
    };
    ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_QUERY), q));

    ui.add_space(4.0);

    // Favorites column + file browser column.
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(140.0);
            ui.label(egui::RichText::new("Favorites").small().strong().color(p.text_secondary));
            ui.add_space(4.0);
            let mut jump: Option<PathBuf> = None;
            for dir in app.favorite_dirs().to_vec() {
                ui.horizontal(|ui| {
                    let name = dir_name(&dir);
                    let active = app.library_dir() == dir;
                    if ui
                        .add_sized(
                            egui::vec2(100.0, 18.0),
                            egui::Label::new(
                                egui::RichText::new(&name)
                                    .color(if active { p.accent } else { p.text_secondary })
                                    .font(egui::FontId::new(12.0, theme.metadata_font.clone())),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        )
                        .clicked()
                    {
                        jump = Some(dir.clone());
                    }
                    if ui.small_button("✕").clicked() {
                        app.toggle_favorite(dir.clone());
                    }
                });
            }
            if let Some(dir) = jump {
                app.navigate_to(dir);
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            // Sortable column header over the file list (Title takes the
            // flexible column; the tag cells mirror the row widths).
            let (cur, asc) = (app.library_sort(), app.library_sort_asc());
            ui.horizontal(|ui| {
                let (head, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 22.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(head, 0.0, p.row_odd);
                let mut h = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_min_max(head.min + egui::vec2(6.0, 0.0), head.max))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                h.spacing_mut().item_spacing.x = 4.0;
                let title_w = (head.width() - ROW_FIXED_W).max(2.0);
                if header_cell(&mut h, p, "Title", 0 == cur, asc, title_w, 22.0) {
                    app.set_library_sort(0);
                }
                for (label, key, w) in [
                    ("Artist", 1usize, CELL_ARTIST),
                    ("Album", 2, CELL_ALBUM),
                    ("Year", 3, CELL_YEAR),
                    ("Genre", 4, CELL_GENRE),
                    ("Duration", 5, CELL_DUR),
                ] {
                    if header_cell(&mut h, p, label, key == cur, asc, w, 22.0) {
                        app.set_library_sort(key);
                    }
                }
            });

            let scroll_h = (ui.available_height() - 24.0).max(40.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(scroll_h)
                .show(ui, |ui| {
                    let row_h = 24.0;
                    let mut i = 0usize;
                    let mut nav: Option<PathBuf> = None;
                    let mut action: Option<(fn(&mut TPlayApp, PathBuf), PathBuf)> = None;
                    let mut load: Option<PathBuf> = None;

                    // Parent dir row, then the sorted folder + file rows.
                    if let Some(up) = app.library_dir().parent() {
                        if draw_dir_row(ui, &theme, row_h, i, "..") {
                            nav = Some(up.to_path_buf());
                        }
                        i += 1;
                    }
                    for entry in app.library_entries() {
                        match entry {
                            library::Entry::Dir(dir) => {
                                let name = dir_name(dir);
                                if !query.is_empty() && !name.to_lowercase().contains(&query) {
                                    continue;
                                }
                                if draw_dir_row(ui, &theme, row_h, i, &name) {
                                    nav = Some(dir.clone());
                                }
                            }
                            library::Entry::File(file) => {
                                if library::is_playlist(file) {
                                    let name = file
                                        .file_name()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    if !query.is_empty() && !name.to_lowercase().contains(&query) {
                                        continue;
                                    }
                                    if draw_playlist_row(ui, &theme, row_h, i, file) {
                                        let stem = file
                                            .file_stem()
                                            .map(|s| s.to_string_lossy().into_owned())
                                            .unwrap_or_default();
                                        if TPlayApp::confirm(
                                            "Load playlist",
                                            &format!("Replace the current playlist with '{stem}'?"),
                                            app.playlist_dirty(),
                                        ) {
                                            load = Some(file.clone());
                                        }
                                    }
                                } else {
                                    let info = app.track_info(file);
                                    let stem = file
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    let hay = format!(
                                        "{} {} {} {stem}",
                                        info.map(|i| i.title.as_str()).unwrap_or_default(),
                                        info.map(|i| i.artist.as_str()).unwrap_or_default(),
                                        info.map(|i| i.album.as_str()).unwrap_or_default(),
                                    );
                                    if !query.is_empty() && !hay.to_lowercase().contains(&query) {
                                        continue;
                                    }
                                    if let Some(f) = draw_file_row(app, ui, &theme, row_h, i, file, info) {
                                        action = Some((f, file.clone()));
                                    }
                                }
                            }
                        }
                        i += 1;
                    }

                    if let Some(dir) = nav {
                        app.navigate_to(dir);
                    }
                    if let Some((f, path)) = action {
                        f(app, path);
                    }
                    if let Some(path) = load {
                        app.load_playlist_from(path);
                    }
                    if i == 0 {
                        ui.label(egui::RichText::new("No audio files").small().color(p.text_secondary));
                    }
                });
            if app.library_scanning() {
                ui.label(egui::RichText::new("Scanning…").small().color(p.text_secondary));
            }
        });
    });
}