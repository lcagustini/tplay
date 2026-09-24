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



/// Column widths shared by the sortable header and the file rows (matches SORT_OPTIONS).
const CELL_WIDTHS: [f32; 6] = [0.0, 90.0, 100.0, 34.0, 64.0, 44.0]; // Title is flexible
/// Fixed right-hand width per row: the five tag cells + the `+` button +
/// inter-cell spacing, leaving the title column the flexible remainder.
const ROW_FIXED_W: f32 = CELL_WIDTHS[1] + CELL_WIDTHS[2] + CELL_WIDTHS[3] + CELL_WIDTHS[4] + CELL_WIDTHS[5] + 40.0;

/// One clickable column header. Returns true when clicked; the caller picks
/// the sort key. The active column is accent-colored with a theme icon
/// (`arrow`) showing the sort direction; clicking it again flips the
/// direction in `TPlayApp::set_library_sort`.
fn header_cell(
    ui: &mut egui::Ui,
    p: theme::Palette,
    label: &str,
    active: bool,
    arrow: Option<&egui::TextureHandle>,
    w: f32,
    row_h: f32,
) -> bool {
    let color = if active { p.accent } else { p.text_secondary };
    let btn = match arrow {
        Some(tex) => egui::Button::image_and_text(
            egui::Image::new(tex).fit_to_exact_size(egui::vec2(10.0, 10.0)),
            egui::RichText::new(label).small().color(color),
        ),
        None => egui::Button::new(egui::RichText::new(label).small().color(color)),
    };
    ui.add_sized(egui::vec2(w, row_h), btn.frame(false))
        .on_hover_text("Sort by this column")
        .clicked()
}

fn draw_dir_row(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    row_h: f32,
    i: usize,
    name: &str,
    folder: Option<&egui::TextureHandle>,
) -> bool {
    let p = theme.palette;
    let layout = theme.layout.with_defaults();
    let (rect, mut row) = theme::row(ui, i, false, row_h, theme);
    row.spacing_mut().item_spacing.x = 4.0;
    let mut text_x = rect.min.x + 6.0;
    if let Some(tex) = folder {
        let s = 14.0;
        let img_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 2.0, rect.center().y - s / 2.0),
            egui::vec2(s, s),
        );
        ui.painter().image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        text_x += s + 4.0;
    }
    let text = if folder.is_none() { format!("📁  {name}") } else { name.to_string() };
    ui.painter().text(
        egui::pos2(text_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::new(layout.text_meta, theme.metadata_font.clone()),
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
    let (rect, mut row) = theme::row(ui, i, false, row_h, theme);
    row.spacing_mut().item_spacing.x = 4.0;

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

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

    // Tag sits right after the name (left-of-center), not floated to the
    // row's far right past the empty tag cells.
    row.add(egui::Label::new(egui::RichText::new("Playlist").small().color(p.accent)));

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
    let layout = theme.layout.with_defaults();
    let is_current = app.current_path().is_some_and(|c| c == path);
    let (rect, mut row) = theme::row(ui, i, is_current, row_h, theme);
    row.spacing_mut().item_spacing.x = 4.0;

    let title = library::title_or_stem(path, info);
    let display = match info.and_then(|i| i.track_no.clone()) {
        Some(n) => format!("{n}. {title}"),
        None => title,
    };
    let artist = info.map(|i| i.artist.as_str()).unwrap_or_default();
    let album = info.map(|i| i.album.as_str()).unwrap_or_default();
    let year = info.and_then(|i| i.year.as_deref()).unwrap_or_default();
    let genre = info.map(|i| i.genre.as_str()).unwrap_or_default();
    let dur = info.and_then(|i| i.duration);

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
    cell(&mut row, CELL_WIDTHS[1], artist);
    cell(&mut row, CELL_WIDTHS[2], album);
    cell(&mut row, CELL_WIDTHS[3], year);
    cell(&mut row, CELL_WIDTHS[4], genre);
    row.add_sized(
        egui::vec2(CELL_WIDTHS[5], row_h),
        egui::Label::new(
            egui::RichText::new(TPlayApp::fmt_duration(dur))
                .color(p.text_secondary)
                .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
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
    let layout = theme.layout.with_defaults();

    // First frame this session: list the saved/current dir and start the scan.
    if !ui.ctx().memory_mut(|m| m.data.get_temp::<bool>(egui::Id::new(LIB_INIT)).unwrap_or(false)) {
        ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_INIT), true));
        app.navigate_to(app.library_dir().to_path_buf());
    }

    // Header: breadcrumb to the current dir (clickable ancestors) + favorite
    // toggle; composition counts + Add All on the right.
    let folder_tex = app.theme_icon(theme::Icon::Folder).cloned();
    let (n_tracks, n_dirs, n_playlists) = {
        let mut c = (0usize, 0usize, 0usize);
        for e in app.library_entries() {
            if e.is_dir() {
                c.1 += 1;
            } else if library::is_playlist(e.path()) {
                c.2 += 1;
            } else {
                c.0 += 1;
            }
        }
        c
    };
    ui.horizontal(|ui| {
        // Breadcrumb: root → current, every ancestor clickable; middle
        // segments collapse to "…" beyond depth 3 so deep paths fit narrow
        // panes. The trailing segment is the current dir (strong, inactive).
        let dir = app.library_dir().to_path_buf();
        let mut segs: Vec<PathBuf> = Vec::new();
        let mut cur = Some(dir.clone());
        while let Some(d) = cur {
            segs.push(d.clone());
            cur = d.parent().map(Path::to_path_buf);
        }
        segs.reverse();
        let mut jump: Option<PathBuf> = None;
        for (i, seg) in segs.iter().enumerate() {
            // Root + last two always show; anything in between → one "…".
            if i != 0 && i + 2 < segs.len() {
                if i == 1 {
                    ui.label(egui::RichText::new("…").small().color(p.text_secondary));
                }
                continue;
            }
            let name = dir_name(seg);
            if i + 1 == segs.len() {
                ui.label(egui::RichText::new(name).strong().color(p.text_primary))
                    .on_hover_text(seg.display().to_string());
            } else {
                // Cap each ancestor's width so long names truncate instead of
                // shoving the ★ toggle / Add All off the pane edge.
                let w = (name.chars().count() as f32 * 8.0 + 6.0).min(70.0);
                if ui
                    .add_sized(
                        egui::vec2(w, 18.0),
                        egui::Label::new(
                            egui::RichText::new(name).small().color(p.text_secondary),
                        )
                        .truncate()
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text(seg.display().to_string())
                    .clicked()
                {
                    jump = Some(seg.clone());
                }
                ui.label(egui::RichText::new("/").small().color(p.text_secondary));
            }
        }
        if let Some(seg) = jump {
            app.navigate_to(seg);
        }
        let fav = app.is_favorite(&dir);
        let tex = if fav {
            app.theme_icon(theme::Icon::StarOn).cloned()
        } else {
            app.theme_icon(theme::Icon::StarOff).cloned()
        };
        let fav_btn = match tex {
            Some(tex) => egui::Button::image(
                egui::Image::new(&tex).fit_to_exact_size(egui::vec2(14.0, 14.0)),
            )
            .selected(fav),
            None => egui::Button::new(if fav { "★" } else { "☆" }).selected(fav),
        };
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
            // "1 track" stays singular.
            let n = |n: usize, s: &str| format!("{n} {s}{}", if n == 1 { "" } else { "s" });
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} · {}",
                    n(n_tracks, "track"),
                    n(n_dirs, "folder"),
                    n(n_playlists, "playlist"),
                ))
                .small()
                .color(p.text_secondary),
            );
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

    // Places + Favorites column, file browser column.
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(120.0);
            let scroll_h = (ui.available_height() - 8.0).max(40.0);
            // id_salt: without it this would share the default "scroll_area"
            // persistent id with the file list's ScrollArea (sibling column
            // uis resolve to the same ui.id) → egui ID-clash debug overlay.
            egui::ScrollArea::vertical()
                .id_salt("places_favorites")
                // auto_shrink x=true: a vertical scroll area must not claim the
                // whole row width (auto_shrink=false expands it to fill) — that
                // starves the file-list column next to it.
                .auto_shrink([true, false])
                .max_height(scroll_h)
                .show(ui, |ui| {
                    let mut jump: Option<PathBuf> = None;

                    // Places: fixed user-folder shortcuts (Home + XDG dirs) —
                    // same row style as favorites, no ✕ (not removable).
                    let places = app.quick_folders();
                    if !places.is_empty() {
                        ui.label(egui::RichText::new("Places").small().strong().color(p.text_secondary));
                        ui.add_space(4.0);
                        for (label, path) in places {
                            let active = app.library_dir() == path;
                            if ui
                                .add_sized(
                                    egui::vec2(100.0, 18.0),
                                    egui::Label::new(
                                        egui::RichText::new(label)
                                            .color(if active { p.accent } else { p.text_secondary })
                                            .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                jump = Some(path.clone());
                            }
                        }
                        ui.add_space(8.0);
                    }

                    // Volumes: local block partitions from /proc/self/mounts.
                    let volumes = library::Volume::mounted_volumes();
                    if !volumes.is_empty() {
                        ui.label(egui::RichText::new("Volumes").small().strong().color(p.text_secondary));
                        ui.add_space(4.0);
                        for vol in volumes {
                            let active = app.library_dir() == vol.path;
                            if ui
                                .add_sized(
                                    egui::vec2(100.0, 18.0),
                                    egui::Label::new(
                                        egui::RichText::new(&vol.label)
                                            .color(if active { p.accent } else { p.text_secondary })
                                            .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(vol.path.display().to_string())
                                .clicked()
                            {
                                jump = Some(vol.path.clone());
                            }
                        }
                        ui.add_space(8.0);
                    }

                    ui.label(egui::RichText::new("Favorites").small().strong().color(p.text_secondary));
                    ui.add_space(4.0);
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
                                            .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .clicked()
                            {
                                jump = Some(dir.clone());
                            }
                            if theme::icon_button(ui, app.theme_icon(theme::Icon::Remove), theme::Icon::Remove, 13.0, true, false).clicked() {
                                app.toggle_favorite(dir.clone());
                            }
                        });
                    }
                    if let Some(dir) = jump {
                        app.navigate_to(dir);
                    }
                });
        });
        ui.separator();
        ui.vertical(|ui| {
            // Sortable column header only when the folder has audio files —
            // folders/playlists-only rows have no tag columns to align to.
            let has_audio = app
                .library_entries()
                .iter()
                .any(|e| !e.is_dir() && !library::is_playlist(e.path()));
            if has_audio {
                let (cur, asc) = (app.library_sort(), app.library_sort_asc());
                let asc_tex = app.theme_icon(theme::Icon::SortAsc).cloned();
                let desc_tex = app.theme_icon(theme::Icon::SortDesc).cloned();
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
                    for (key, label) in library::SORT_OPTIONS.iter().enumerate() {
                        let w = if key == 0 {
                            (head.width() - ROW_FIXED_W).max(2.0)
                        } else {
                            CELL_WIDTHS[key]
                        };
                        let arrow = if key == cur {
                            // Ascending → up triangle; descending → down.
                            if asc { asc_tex.as_ref() } else { desc_tex.as_ref() }
                        } else {
                            None
                        };
                        if header_cell(&mut h, p, label, key == cur, arrow, w, 22.0) {
                            app.set_library_sort(key);
                        }
                    }
                });
            }

            let scroll_h = (ui.available_height() - 24.0).max(40.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(scroll_h)
                .show(ui, |ui| {
                    let row_h = 24.0;
                    let mut i = 0usize;
                    enum Act { Nav(PathBuf), Action(fn(&mut TPlayApp, PathBuf), PathBuf) }
                    let mut action: Option<Act> = None;
                    let mut load: Option<PathBuf> = None;

                    // Parent dir row, then the sorted folder + file rows.
                    if let Some(up) = app.library_dir().parent() {
                        if draw_dir_row(ui, &theme, row_h, i, "..", folder_tex.as_ref()) {
                            action = Some(Act::Nav(up.to_path_buf()));
                        }
                        i += 1;
                    }
                    for entry in app.library_entries() {
                        if entry.is_dir {
                            let name = dir_name(entry.path());
                            if !query.is_empty() && !name.to_lowercase().contains(&query) {
                                i += 1;
                                continue;
                            }
                            if draw_dir_row(ui, &theme, row_h, i, &name, folder_tex.as_ref()) {
                                action = Some(Act::Nav(entry.path().to_path_buf()));
                            }
                        } else {
                            let file = entry.path();
                            if library::is_playlist(file) {
                                let name = file
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                if !query.is_empty() && !name.to_lowercase().contains(&query) {
                                    i += 1;
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
                                        load = Some(file.to_path_buf());
                                    }
                                }
                            } else {
                                let info = app.track_info(file);
                                let title = library::title_or_stem(file, info);
                                let hay = format!(
                                    "{} {} {} {title}",
                                    info.map(|i| i.title.as_str()).unwrap_or_default(),
                                    info.map(|i| i.artist.as_str()).unwrap_or_default(),
                                    info.map(|i| i.album.as_str()).unwrap_or_default(),
                                );
                                if !query.is_empty() && !hay.to_lowercase().contains(&query) {
                                    i += 1;
                                    continue;
                                }
                                if let Some(f) = draw_file_row(app, ui, &theme, row_h, i, file, info) {
                                    action = Some(Act::Action(f, file.to_path_buf()));
                                }
                            }
                        }
                        i += 1;
                    }

                    if let Some(Act::Nav(dir)) = action {
                        app.navigate_to(dir);
                    } else if let Some(Act::Action(f, path)) = action {
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