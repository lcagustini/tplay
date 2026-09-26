//! Library pane — file browser + favorite folders + tag-scanning track list.
//!
//! Left: favorite folders. Right: the current folder's subfolders and audio
//! files, each row showing title + artist/album/year/genre + duration from
//! the tag scan (cached in `TPlayApp`, so revisits are instant). Click a row
//! to play it directly; the `+` button adds it to the playlist.

use crate::app::TPlayApp;
use crate::gui::theme;
use crate::library;
use crate::network;
use eframe::egui;
use std::path::{Path, PathBuf};

/// egui memory: has the pane listed `library_dir` at least once this session.
const LIB_INIT: &str = "tplay.library.init";
/// egui memory: the active search filter text.
const LIB_QUERY: &str = "tplay.library.query";
/// egui memory: the SMB add-server form state (host, username, password).
/// `None` = closed; `Some` = open with the fields being edited.
const NET_FORM: &str = "tplay.network.form";
/// egui memory: the main-pane login prompt, keyed per host (`(NET_LOGIN, host)`).
const NET_LOGIN: &str = "tplay.network.login";

/// Fixed width of the Places/Favorites sidebar column. Allocated as an exact
/// rect (not `set_min_width`) so no child can resize it — see the comment at
/// the `new_child` call in the sidebar body.
const SIDEBAR_W: f32 = 120.0;
/// Width of the add-server form's text fields, inside the fixed-width sidebar.
const FORM_W: f32 = 100.0;
/// Height of one add-server form text field.
const FORM_FIELD_H: f32 = 18.0;

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
        .on_hover_text_at_pointer("Load playlist");

    // Tag sits right after the name (left-of-center), not floated to the
    // row's far right past the empty tag cells.
    row.add(egui::Label::new(egui::RichText::new("Playlist").small().color(p.accent)));

    title_resp.clicked()
}

/// What a track row's click asked for, before it becomes a full `Act` (the
/// caller supplies the path, since it owns the entry list).
enum FileAct {
    Play,
    Add,
}

/// What the user asked for by clicking a file-list row.
///
/// The local folder browser and the SMB share browser render identical rows, so
/// they share one list widget and differ only in how they carry the action
/// out: `Nav` is `navigate_to` locally and `browse_open` on a share,
/// `LoadPlaylist` is `load_playlist_from` locally and `fetch_remote_playlist`
/// (spool, then load) on one.
pub enum Act {
    /// Enter a subdirectory — a local path, or an `smb://` share directory URI.
    Nav(PathBuf),
    /// Play a track (local path or `smb://` URI; `play_file` routes both).
    Play(PathBuf),
    /// Append a track to the playlist.
    Add(PathBuf),
    /// Load a `.tplay` — local path, or an `smb://` URI to fetch first.
    LoadPlaylist(PathBuf),
}

fn draw_file_row(
    app: &TPlayApp,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    row_h: f32,
    i: usize,
    path: &Path,
    info: Option<&library::TrackInfo>,
) -> Option<FileAct> {
    let p = theme.palette;
    let layout = theme.layout.with_defaults();
    // A spooled `smb://` track is "current" under its URI, which is what the
    // playlist holds — so the highlight follows the track across both sources.
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
        .on_hover_text_at_pointer("Play");

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
        .on_hover_text_at_pointer("Add to playlist")
        .clicked();

    if title_resp.clicked() {
        Some(FileAct::Play)
    } else if add_clicked {
        Some(FileAct::Add)
    } else {
        None
    }
}

/// The file list shared by the local folder browser and the SMB share browser:
/// the search box, the sortable 6-column header, the folder/track/playlist rows
/// and the "Scanning…" note.
///
/// Both sources are `&[library::Entry]`. For a share, `path` is the `smb://`
/// URI, which every key here treats like any other path — `file_name` and
/// `file_stem` return the remote name, so `title_or_stem`/`sort_key` and the
/// playlist-file check all work without a special case. That is what lets the
/// share browser have real tag columns, sorting and search instead of the
/// name+size list it started with.
///
/// There is deliberately **no `..` row**: the breadcrumb is the only way up.
/// It walks every ancestor and makes each non-last segment a jump target, so
/// "one level up" is the second-to-last segment — a dedicated row duplicated
/// navigation, consumed a banded row, and counted toward the folder total in
/// the composition counts ("3 folders" when only 2 are). A share's breadcrumb
/// works the same way (Local / host / share / dir), which is why that view
/// never had one either.
///
/// `scan_note` is the status line shown under the list while it fills in, and
/// each caller words it for its own source: the local browser says
/// "Scanning…", the share browser counts down the tracks it is still
/// downloading (see `Network::pending_tags`) because a share browse is a real
/// transfer, not a local read, and a bare "Scanning…" on a 300-file directory
/// looks like a hung pane.
///
/// Returns the row the user clicked, if any; the caller carries it out, since
/// only it knows whether a `Nav` means `navigate_to` or `browse_open`.
fn file_list_ui(
    app: &mut TPlayApp,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    entries: &[library::Entry],
    scan_note: Option<String>,
) -> Option<Act> {
    let p = theme.palette;
    let folder_tex = app.theme_icon(theme::Icon::Folder).cloned();
    let mut out = None;

    // Search box. State is the one `LIB_QUERY` key, so the query deliberately
    // survives a trip between the local and share browsers.
    let mut q = ui
        .ctx()
        .memory_mut(|m| m.data.get_temp::<String>(egui::Id::new(LIB_QUERY)).unwrap_or_default());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Search").small().color(p.text_secondary));
        ui.add(
            egui::TextEdit::singleline(&mut q)
                .hint_text("title, artist, album…")
                .desired_width(220.0),
        );
    });
    ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_QUERY), q.clone()));
    let query = q.trim().to_lowercase();
    ui.add_space(4.0);

    // Sortable column header only when the folder has audio files —
    // folders/playlists-only rows have no tag columns to align to.
    let has_audio = entries
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
            let mut action: Option<Act> = None;

            // Sorted folder + file rows. No `..` row — see `file_list_ui`.
            for entry in entries {
                let path = entry.path();
                if entry.is_dir {
                    let name = dir_name(path);
                    if !query.is_empty() && !name.to_lowercase().contains(&query) {
                        i += 1;
                        continue;
                    }
                    if draw_dir_row(ui, theme, row_h, i, &name, folder_tex.as_ref()) {
                        action = Some(Act::Nav(path.to_path_buf()));
                    }
                } else if library::is_playlist(path) {
                    if !query.is_empty() && !path.to_string_lossy().to_lowercase().contains(&query)
                    {
                        i += 1;
                        continue;
                    }
                    if draw_playlist_row(ui, theme, row_h, i, path) {
                        let stem = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        // Replacing a playlist with unsaved edits asks first —
                        // on a share exactly as on disk.
                        if TPlayApp::confirm(
                            "Load playlist",
                            &format!("Replace the current playlist with '{stem}'?"),
                            app.playlist_dirty(),
                        ) {
                            action = Some(Act::LoadPlaylist(path.to_path_buf()));
                        }
                    }
                } else {
                    let info = app.track_info(path);
                    let title = library::title_or_stem(path, info);
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
                    match draw_file_row(app, ui, theme, row_h, i, path, info) {
                        Some(FileAct::Play) => action = Some(Act::Play(path.to_path_buf())),
                        Some(FileAct::Add) => action = Some(Act::Add(path.to_path_buf())),
                        None => {}
                    }
                }
                i += 1;
            }
            if i == 0 {
                ui.label(egui::RichText::new("No files").small().color(p.text_secondary));
            }
            if action.is_some() {
                out = action;
            }
        });
    if let Some(note) = scan_note {
        ui.label(egui::RichText::new(note).small().color(p.text_secondary));
    }
    out
}

/// The right-hand end of a file-list header: composition counts and Add All.
/// Shared because both browsers now work from `&[library::Entry]` — a share's
/// entries carry `smb://` URIs, which go into the playlist unchanged.
///
/// **`Align::Min` is load-bearing, not cosmetic.** A horizontal `with_layout`
/// whose cross-axis align is `Center` or `Max` gives its child a `min_rect`
/// spanning the parent's *whole remaining height*, not the height actually used
/// — `scope_dyn` ends with `advance_cursor_after_rect(child.min_rect())`, so
/// the parent cursor jumps by that entire span. Measured in a 460px window with
/// this pane's real shape: `right_to_left(Center)` consumed **430px** of a
/// 444px column (leaving `available_height() == 0`), while `right_to_left(Min)`
/// consumed the **21px** it used. With `Center` the file list's ScrollArea was
/// positioned at y≈497 — below the pane — and only the Add All row was visible
/// beside the sidebar.
///
/// The damage scales with the parent's available height, which is why this went
/// unnoticed elsewhere: every other `with_layout` in the app sits under a
/// *horizontal* parent (`coordinator.rs` top bar, `now_playing.rs` balance row,
/// `playlist.rs` rows, `visualizer.rs` header), where the child's available
/// height is one row tall. The one other vertical-parent site is
/// `now_playing.rs`'s transport block, whose pane is ~15% of the window, so the
/// over-consumption is bounded by an already-short height.
/// Regression test: `right_to_left_center_does_not_swallow_the_column` in
/// `tests/gui_tests.rs`.
fn list_header_right(
    app: &mut TPlayApp,
    ui: &mut egui::Ui,
    p: theme::Palette,
    entries: &[library::Entry],
) {
    // `Align::Min`, NOT `Center` — see the note below. It looks like a cosmetic
    // choice (top-align vs vertically centre a ~21px row) and is anything but.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if ui
            .button("Add All")
            .on_hover_text("Add this folder's tracks to the playlist")
            .clicked()
        {
            let files: Vec<PathBuf> = entries
                .iter()
                .filter(|e| !e.is_dir() && !library::is_playlist(e.path()))
                .map(|e| e.path().to_path_buf())
                .collect();
            app.add_files(files);
        }
        // "1 track" stays singular.
        let n = |n: usize, s: &str| format!("{n} {s}{}", if n == 1 { "" } else { "s" });
        let (mut tracks, mut dirs, mut playlists) = (0usize, 0usize, 0usize);
        for e in entries {
            if e.is_dir() {
                dirs += 1;
            } else if library::is_playlist(e.path()) {
                playlists += 1;
            } else {
                tracks += 1;
            }
        }
        ui.label(
            egui::RichText::new(format!(
                "{} · {} · {}",
                n(tracks, "track"),
                n(dirs, "folder"),
                n(playlists, "playlist"),
            ))
            .small()
            .color(p.text_secondary),
        );
    });
}

/// The remote browser: swaps the main column when a server is selected.
/// Breadcrumb (Local / host / share / dir) + the shared file list, with
/// `browse_open` standing in for `navigate_to`.
fn remote_list_ui(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let Some(browse) = app.network().browse().cloned() else { return };
    let theme = app.theme().clone();
    let p = theme.palette;
    let folder_tex = app.theme_icon(theme::Icon::Folder).cloned();

    // Breadcrumb segments: (label, uri, share, rel). "Local" is the exit.
    let mut segs: Vec<(String, String, Option<String>, String)> =
        vec![(String::from("Local"), String::new(), None, String::new())];
    segs.push((
        browse.host.clone(),
        network::server_uri(&browse.host),
        None,
        String::new(),
    ));
    if let Some(share) = &browse.share {
        segs.push((
            share.clone(),
            network::share_uri(&browse.host, share),
            Some(share.clone()),
            String::new(),
        ));
        if !browse.rel.is_empty() {
            let mut walk = String::new();
            for part in browse.rel.split('/') {
                if !walk.is_empty() {
                    walk.push('/');
                }
                walk.push_str(part);
                segs.push((
                    part.to_string(),
                    network::dir_uri(&browse.host, share, &walk),
                    Some(share.clone()),
                    walk.clone(),
                ));
            }
        }
    }

    ui.horizontal(|ui| {
        for (i, (label, uri, share, rel)) in segs.iter().enumerate() {
            let last = i + 1 == segs.len();
            if last {
                ui.label(egui::RichText::new(label).strong().color(p.text_primary));
                continue;
            }
            let w = (label.chars().count() as f32 * 8.0 + 6.0).min(70.0);
            let clicked = ui
                .add_sized(
                    egui::vec2(w, 18.0),
                    egui::Label::new(
                        egui::RichText::new(label).small().color(p.text_secondary),
                    )
                    .truncate()
                    .sense(egui::Sense::click()),
                )
                .clicked();
            ui.label(egui::RichText::new("/").small().color(p.text_secondary));
            if clicked {
                if i == 0 {
                    app.network_mut().leave_network();
                } else if share.is_none() {
                    app.network_mut().browse_server(browse.host.clone());
                } else {
                    app.network_mut().browse_open(uri.clone(), share.clone(), rel.clone());
                }
            }
        }
    });

    ui.add_space(4.0);

    // Busy / error states replace the list entirely — there is nothing to show
    // while a listing is in flight, and on a logon failure the main pane owes
    // the user a credential prompt rather than a file list.
    if browse.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new("Connecting…").small().color(p.text_secondary));
        });
        return;
    }
    if let Some(err) = &browse.error {
        // SessionSetup is where a server rejects the *identity*, so a bare
        // NTSTATUS is not actionable — ask for the credentials here
        // instead, with the saved address already filled in.
        if err.contains("LOGON_FAILURE") || err.contains("ACCESS_DENIED") {
            login_form_ui(app, ui, &browse);
        } else {
            ui.label(egui::RichText::new(err).small().color(p.text_secondary));
            ui.label(
                egui::RichText::new("Check the address, username/password, and that the share allows access.")
                    .small()
                    .color(p.text_secondary),
            );
        }
        return;
    }

    // At the share-list stage there is no directory, so the shared list has
    // nothing to show — shares are a flat list of names, and the breadcrumb's
    // "Local" is the way back out.
    if browse.share.is_none() {
        let mut i = 0usize;
        // A share name, NOT a URI — unlike every other row in this pane, the
        // share list is built from `RemoteEntry::name` and has no Entry to
        // carry a path. Named `share_name` so it can't be confused with one.
        let mut share_name: Option<String> = None;
        let mut names: Vec<&network::RemoteEntry> = browse.entries.iter().collect();
        names.sort_by_key(|e| e.name.to_lowercase());
        for e in names {
            if draw_dir_row(ui, &theme, 24.0, i, &e.name, folder_tex.as_ref()) {
                share_name = Some(e.name.clone());
            }
            i += 1;
        }
        if i == 0 {
            ui.label(egui::RichText::new("No shares").small().color(p.text_secondary));
        }
        if let Some(share) = share_name {
            app.network_mut().browse_open(
                network::share_uri(&browse.host, &share),
                Some(share),
                String::new(),
            );
        }
        return;
    }

    let share = browse.share.clone().unwrap_or_default();
    let dir = network::dir_uri(&browse.host, &share, &browse.rel);

    // Share rows become ordinary `library::Entry` values whose `path` is the
    // file's URI, which is what lets the shared list treat a share exactly like
    // a folder: same tag columns, same sort keys, same playlist-file rows.
    // Anything that is neither audio nor a `.tplay` is not listed, same as a
    // local folder.
    let entries: Vec<library::Entry> = browse
        .entries
        .iter()
        .filter(|e| e.is_dir || library::is_audio(Path::new(&e.name)) || library::is_playlist(Path::new(&e.name)))
        .map(|e| library::Entry {
            path: PathBuf::from(network::child_uri(&dir, &e.name)),
            is_dir: e.is_dir,
        })
        .collect();
    let mut entries = entries;
    library::sort_entries(&mut entries, app.tag_cache(), app.library_sort(), app.library_sort_asc());

    // Ask for the tags of everything on screen that isn't cached yet. Safe to
    // do every frame: `ensure_tags` skips cached paths, and the network side
    // keeps an in-flight set plus a per-track attempt count, so a running batch
    // is never re-queued and a hopeless one is given up on. Rows start as
    // filenames and fill in as the downloads land.
    let audio: Vec<PathBuf> = entries
        .iter()
        .filter(|e| !e.is_dir() && !library::is_playlist(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect();
    let untagged = audio.iter().filter(|p| app.track_info(p).is_none()).count();
    app.ensure_tags(audio);

    // Show the outstanding transfer count, not just a bool: a share browse
    // downloads whole files, so this is a real wait and a bare "Scanning…"
    // reads as a hang. Count only what is actually in flight — a track that
    // gave up after its retries is not coming, so counting it would pin a
    // number on screen forever.
    let note = (untagged > 0).then(|| {
        let in_flight = app.network().pending_tags();
        if in_flight > 0 {
            format!("Reading tags… ({in_flight} left)")
        } else {
            format!("Reading tags… ({untagged} unavailable)")
        }
    });

    list_header_right(app, ui, p, &entries);
    ui.add_space(4.0);
    let act = file_list_ui(app, ui, &theme, &entries, note);

    match act {
        Some(Act::Nav(path)) => {
            // The shared list hands back `entry.path()`, which for a share is
            // the FULL child URI the entry was built from — not a bare folder
            // name. Re-joining it with `child_browse` appended the whole URI as
            // a name segment, giving a `rel` of
            // "music/smb://nas/media/music/Rock" and a PATH_NOT_FOUND from the
            // server. Split it instead: for a URI already in child form that is
            // a no-op round trip. (See `nav_uri_round_trips_but_renaming_one_does_not`
            // in `tests/smb_helpers.rs`.)
            let uri = path.to_string_lossy().into_owned();
            if let Some((_host, share, rel)) = network::split_uri(&uri) {
                app.network_mut().browse_open(uri, share, rel);
            }
        }
        Some(Act::Play(path)) => app.play_file(path),
        Some(Act::Add(path)) => app.add_files(vec![path]),
        Some(Act::LoadPlaylist(uri)) => app.fetch_remote_playlist(uri.to_string_lossy().into_owned()),
        None => {}
    }
}

/// Credential prompt for a server that rejected our identity.
///
/// Deliberately in the **main** pane, not the sidebar: the address is already
/// known and saved, so only the login should ever need retyping. The username is
/// persisted with the server; the password stays in the session map only.
fn login_form_ui(app: &mut TPlayApp, ui: &mut egui::Ui, browse: &network::NetworkBrowse) {
    let host = browse.host.clone();
    let theme = app.theme().clone();
    let p = theme.palette;
    // Keyed per host, so each saved server keeps the username it was given and
    // a revisit prefills it instead of starting blank.
    let id = egui::Id::new((NET_LOGIN, host.as_str()));
    let saved_user = app
        .network()
        .servers()
        .iter()
        .find(|s| s.host == host)
        .map(|s| s.username.clone())
        .unwrap_or_default();
    let mut creds = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp::<(String, String)>(id)
            .unwrap_or_else(|| (saved_user.clone(), String::new()))
    });

    ui.label(
        egui::RichText::new(format!("Login required for {host}"))
            .small()
            .strong()
            .color(p.accent),
    );
    ui.label(
        egui::RichText::new(
            "The server rejected the anonymous login. A blank username is not a guest account.",
        )
        .small()
        .color(p.text_secondary),
    );
    ui.add_space(4.0);

    // Enter in either field submits (TextEdit surrenders focus on Enter).
    let mut enter = false;
    let field_w = 220.0;
    let mut field = ui.add_sized(
        egui::vec2(field_w, 18.0),
        egui::TextEdit::singleline(&mut creds.0)
            .hint_text("username")
            .desired_width(field_w)
            .clip_text(true),
    );
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        enter = true;
    }
    field = ui.add_sized(
        egui::vec2(field_w, 18.0),
        egui::TextEdit::singleline(&mut creds.1)
            .password(true)
            .hint_text("password")
            .desired_width(field_w)
            .clip_text(true),
    );
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        enter = true;
    }

    let mut connect = false;
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new("Connect").small()).clicked() {
            connect = true;
        }
        if ui.add(egui::Button::new("Cancel").small()).clicked() {
            app.network_mut().leave_network();
        }
    });
    if !connect && enter {
        connect = true;
    }
    if connect {
        let (user, pass) = (creds.0.clone(), creds.1.clone());
        // Username goes to the saved server (persisted); the password only ever
        // reaches the session map — never config.json.
        app.add_network_server(host.clone(), user);
        app.set_network_password(host.clone(), pass);
        // Retry the stage we were actually on, so a login deep inside a share
        // does not bounce the user back out to the share list.
        match &browse.share {
            Some(share) => {
                let rel = browse.rel.clone();
                app.network_mut().browse_open(
                    network::dir_uri(&host, share, &rel),
                    Some(share.clone()),
                    rel,
                );
            }
            None => app.network_mut().browse_server(host.clone()),
        }
    }
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, creds));
}

pub fn library_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let theme = app.theme().clone();
    let p = theme.palette;
    let layout = theme.layout.with_defaults();

    // Network mode swaps the main column for the remote browser; the local
    // header (breadcrumb + favorite) only applies to the local folder browser
    // (the Places column below stays in both modes). Everything below the
    // header — search, sort header, rows, counts, Add All — is the shared
    // `file_list_ui`, so both browsers get it.
    let network_mode = app.network().browse().is_some();
    if !network_mode {
    // First frame this session: list the saved/current dir and start the scan.
    if !ui.ctx().memory_mut(|m| m.data.get_temp::<bool>(egui::Id::new(LIB_INIT)).unwrap_or(false)) {
        ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_INIT), true));
        app.navigate_to(app.library_dir().to_path_buf());
    }

    // Header: breadcrumb to the current dir (clickable ancestors) + favorite
    // toggle. The composition counts and Add All live in `list_header_right`,
    // drawn just above the list so the share browser gets them too.
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
    });

    // The counts + Add All that used to sit at the end of the header now live
    // in `list_header_right`, drawn by both browsers just above their list.
    }

    ui.add_space(4.0);

    // Add-server form state (just the host) in egui memory — carried across
    // frames. Credentials are prompted for in the main pane instead, so a saved
    // address can be reused without retyping it.
    let mut form = ui.ctx().memory_mut(|m| {
        m.data.get_temp::<Option<String>>(egui::Id::new(NET_FORM)).unwrap_or(None)
    });

    // Places + Favorites column, file browser column.
    ui.horizontal_top(|ui| {
        // The Places/Favorites column is a FIXED 120px sidebar. Two egui facts
        // force this exact shape:
        //
        // 1. Not `ui.vertical` + `set_min_width`: a `ui.vertical` child is sized
        //    by its own `min_rect`, and `TextEdit` deliberately grows that by
        //    the text overflow ("allocate additional space … so a ScrollArea can
        //    scroll to the cursor"). This ScrollArea is vertical-only, so its
        //    width *is* the content width and the overflow propagated up —
        //    typing a long address widened the whole sidebar. `set_max_width`
        //    and `clip_text` cannot stop it: caps bound painting, but the
        //    overflow grows `min_rect`, and `min_rect` wins the layout.
        // 2. `ui.new_child` alone does NOT advance this horizontal cursor (only
        //    `allocate_new_ui` does), so the file-list sibling was laid out at
        //    the same x and drew on top of the sidebar. `allocate_space`
        //    reserves the rect *and* moves the cursor — both halves needed.
        let (_, sidebar_rect) = ui.allocate_space(egui::vec2(SIDEBAR_W, ui.available_height()));
        let mut sidebar = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(sidebar_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        sidebar.vertical(|ui| {
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
                                .on_hover_text_at_pointer(path.display().to_string())
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
                                .on_hover_text_at_pointer(vol.path.display().to_string())
                                .clicked()
                            {
                                jump = Some(vol.path.clone());
                            }
                        }
                        ui.add_space(8.0);
                    }

                    // Network: built-in SMB browsing (no mount required).
                        {
                            let servers = app.network().servers().to_vec();
                            let active_host = app.network().browse().map(|b| b.host.clone());
                            // Same LTR row style as the section labels above — a
                            // right_to_left header would fill the scroll area's
                            // (unbounded) content width and push the + past the
                            // 120px sidebar.
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Network").small().strong().color(p.text_secondary));
                                if ui.add(egui::Button::new("+").small()).on_hover_text("Add server").clicked() {
                                    // is_none + assign: `Option::or` would move `form` out of this scope.
                                    if form.is_none() {
                                        form = Some(String::new());
                                    }
                                }
                            });
                        if let Some(host) = form.as_mut() {
                            // Enter submits (singleline TextEdit surrenders focus
                            // on Enter — the standard pattern).
                            let mut enter = false;
                            let mut submit: Option<bool> = None; // Some(true) = Add, Some(false) = Cancel
                            // The form lives in a FIXED-RECT child so egui's TextEdit
                            // overflow allocation ("allocate additional space … so a
                            // ScrollArea can properly scroll to the cursor") cannot
                            // widen the scroll content — that growth is what kept
                            // dragging the sidebar's scrollbar around while typing.
                            //
                            // Both halves are load-bearing and they are *different*
                            // halves: `allocate_space` reserves the rect AND advances
                            // the layout cursor (a bare `new_child` would leave the
                            // next row drawn on top of the form), while the raw
                            // `new_child` does NOT propagate its own min_rect to the
                            // scroll content, which is what contains the overflow.
                            // `clip_text` alone is not enough — it pins the field rect
                            // but the overflow allocation still grows the parent.
                            let gap = ui.spacing().item_spacing.y;
                            let form_h = FORM_FIELD_H + gap + ui.spacing().interact_size.y;
                            let (_, form_rect) = ui.allocate_space(egui::vec2(FORM_W, form_h));
                            let mut form_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(form_rect)
                                    .layout(egui::Layout::top_down(egui::Align::Min)),
                            );
                            form_ui.vertical(|ui| {
                                let field = ui.add_sized(egui::vec2(FORM_W, FORM_FIELD_H), egui::TextEdit::singleline(host).hint_text("host or smb://host/share").clip_text(true))
                                    .on_hover_text("like 192.168.1.50 — ask for the login in the main pane");
                                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    enter = true;
                                }
                                ui.horizontal(|ui| {
                                    if ui.add(egui::Button::new("Add").small()).clicked() {
                                        submit = Some(true);
                                    }
                                    if ui.add(egui::Button::new("Cancel").small()).clicked() {
                                        submit = Some(false);
                                    }
                                });
                            });
                            if submit.is_none() && enter {
                                submit = Some(true);
                            }
                            match submit {
                                Some(true) => {
                                    // The field accepts a bare host OR a full
                                    // `smb://host/share[/dir]` URI. Parsing it
                                    // keeps the full URI out of the saved-server
                                    // list and drops us straight into the share
                                    // when one is named — the GNOME-Files path,
                                    // which never enumerates shares first.
                                    // Only the host is stored; credentials are asked
                                    // for in the main pane, so a saved server can be
                                    // reused without retyping its address.
                                    if let Some((h, share, rel)) = network::parse_server_input(host) {
                                        app.add_network_server(h.clone(), String::new());
                                        form = None;
                                        if let Some(share) = share.filter(|s| !s.is_empty()) {
                                            app.network_mut().browse_open(
                                                network::dir_uri(&h, &share, &rel),
                                                Some(share),
                                                rel,
                                            );
                                        }
                                    }
                                }
                                Some(false) => form = None,
                                None => {}
                            }
                            ui.add_space(4.0);
                        }
                        for s in servers {
                            ui.horizontal(|ui| {
                                let active = active_host.as_ref() == Some(&s.host);
                                if ui
                                    .add_sized(
                                        egui::vec2(88.0, 18.0),
                                        egui::Label::new(
                                            egui::RichText::new(&s.host)
                                                .color(if active { p.accent } else { p.text_secondary })
                                                .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                                        )
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text_at_pointer("Browse shares")
                                    .clicked()
                                {
                                    app.network_mut().browse_server(s.host.clone());
                                }
                                if theme::icon_button(ui, app.theme_icon(theme::Icon::Remove), theme::Icon::Remove, 13.0, true, false).clicked() {
                                    app.remove_network_server(&s.host);
                                }
                            });
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
                        // Entering a local folder exits network browse mode.
                        app.network_mut().leave_network();
                        app.navigate_to(dir);
                    }
                });
        });
        ui.separator();
        ui.vertical(|ui| {
            if network_mode {
                remote_list_ui(app, ui);
            } else {
            let entries = app.library_entries().to_vec();
            list_header_right(app, ui, p, &entries);
            ui.add_space(4.0);
            let act = file_list_ui(
                app,
                ui,
                &theme,
                &entries,
                app.library_scanning().then(|| "Scanning…".to_string()),
            );
            match act {
                Some(Act::Nav(dir)) => app.navigate_to(dir),
                Some(Act::Play(path)) => app.play_file(path),
                Some(Act::Add(path)) => app.add_files(vec![path]),
                Some(Act::LoadPlaylist(path)) => app.load_playlist_from(path),
                None => {}
            }
            }
        });
    });
    ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new(NET_FORM), form));
}