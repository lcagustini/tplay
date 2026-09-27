//! The Library pane's main column: search, the sortable column header, the rows,
//! the composition counts + Add All, and the remote browser.
//!
//! `file_list_ui` is drawn once and used twice. The local browser passes
//! `library::Entry` values whose `path` is a path; the SMB browser passes entries
//! whose `path` is an `smb://` URI. A URI's last segment is the filename, so
//! every key here — `file_name`, `file_stem`, `title_or_stem`, `sort_key`, the
//! playlist-file check, the search haystack — treats it like any other path with
//! no special case. That is what gives a share real tag columns, sorting and
//! search instead of the name+size list it started with.

use super::dir_name;
use crate::app::TPlayApp;
use crate::gui::dialogs;
use crate::gui::theme::{self, ThemeState};
use crate::library;
use crate::network;
use eframe::egui;
use std::path::{Path, PathBuf};

/// egui memory: the active search filter text.
const LIB_QUERY: &str = "tplay.library.query";
/// egui memory: the main-pane login prompt, keyed per host (`(NET_LOGIN, host)`).
const NET_LOGIN: &str = "tplay.network.login";

/// Column widths shared by the sortable header and the file rows (matches SORT_OPTIONS).
const CELL_WIDTHS: [f32; 4] = [0.0, 90.0, 100.0, 44.0]; // Title is flexible
/// The rating cell: a star and its digit, side by side.
///
/// **Not** one of `CELL_WIDTHS`, because those are the sortable columns and this is
/// not one — the header draws one cell per `SORT_OPTIONS` entry and stops, so the
/// rating sits past the last header cell and the columns still line up. It is not
/// sortable on purpose: rating is a whole-star field, so ordering by it would
/// produce long runs of indistinguishable rows.
const RATING_W: f32 = 24.0;
/// The star inside that cell. Half the row height, so a 24px row holds it without
/// the cell's text growing.
const RATING_STAR: f32 = 10.0;
/// Fixed right-hand width per row: the three tag cells + the rating cell + the `+`
/// button + inter-cell spacing, leaving the title column the flexible remainder.
const ROW_FIXED_W: f32 = CELL_WIDTHS[1] + CELL_WIDTHS[2] + CELL_WIDTHS[3] + RATING_W + 40.0;

/// One clickable column header; true when clicked, and the caller picks the sort
/// key. The active column is accent-colored with a theme icon (`arrow`) for the
/// direction; clicking it again flips it in `TPlayApp::set_library_sort`.
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
    let layout = theme.layout;
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
    let text = if folder.is_none() {
        format!("📁  {name}")
    } else {
        name.to_string()
    };
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

/// A saved playlist row: full filename (extension visible) + a "Playlist" tag —
/// no tag cells, no `+`. Click loads it (the caller adds the replace-confirm).
fn draw_playlist_row(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    row_h: f32,
    i: usize,
    path: &Path,
) -> bool {
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
            egui::Label::new(egui::RichText::new(&name).color(p.text_primary.gamma_multiply(0.85)))
                .truncate()
                .sense(egui::Sense::click()),
        )
        .on_hover_text_at_pointer("Load playlist");

    // Tag sits right after the name (left-of-center), not floated to the
    // row's far right past the empty tag cells.
    row.add(egui::Label::new(
        egui::RichText::new("Playlist").small().color(p.accent),
    ));

    title_resp.clicked()
}

/// What a track row's click asked for, before it becomes a full `Act` (the
/// caller supplies the path, since it owns the entry list).
enum FileAct {
    Play,
    Add,
    /// Open the track editor for the row's track.
    Edit,
}

/// What the user asked for by clicking a file-list row.
///
/// The local and SMB browsers render identical rows, so they share one list
/// widget and differ only in carrying the action out: `Nav` is `navigate_to`
/// locally and `browse_open` on a share; `LoadPlaylist` is `load_playlist_from`
/// locally and `network_mut().fetch` (spool, then load) on one.
enum Act {
    /// Enter a subdirectory — a local path, or an `smb://` share directory URI.
    Nav(PathBuf),
    /// Play a track (local path or `smb://` URI; `play_file` routes both).
    Play(PathBuf),
    /// Append a track to the playlist.
    Add(PathBuf),
    /// Load a `.tplay` — local path, or an `smb://` URI to fetch first.
    LoadPlaylist(PathBuf),
    /// Open the track editor for one track. The **caller** carries it out, because
    /// only it knows whether the row came from disk or from a share: a share row's
    /// rating is display-only, since a spool-cache copy is not the track.
    Edit(PathBuf),
}

// Nine arguments, and a params struct would be longer than the call sites it
// replaces — every one of them is a value the caller already has in hand.
#[allow(clippy::too_many_arguments)]
fn draw_file_row(
    app: &TPlayApp,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    row_h: f32,
    i: usize,
    path: &Path,
    info: Option<&library::TrackInfo>,
    star_on: Option<&egui::TextureHandle>,
    star_off: Option<&egui::TextureHandle>,
) -> Option<FileAct> {
    let p = theme.palette;
    let layout = theme.layout;
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
    let dur = info.and_then(|i| i.duration);

    // Title takes the flexible remainder; the tag cells mirror the header.
    let title_w = (rect.width() - ROW_FIXED_W - 6.0).max(40.0);
    let title_resp = row
        .add_sized(
            egui::vec2(title_w, row_h),
            egui::Label::new(egui::RichText::new(&display).color(if is_current {
                p.text_primary
            } else {
                p.text_primary.gamma_multiply(0.85)
            }))
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
    row.add_sized(
        egui::vec2(CELL_WIDTHS[3], row_h),
        egui::Label::new(
            egui::RichText::new(TPlayApp::fmt_duration(dur))
                .color(p.text_secondary)
                .font(egui::FontId::new(
                    layout.text_meta,
                    theme.metadata_font.clone(),
                )),
        ),
    );

    // The rating: a star, then its digit, in one fixed-width cell. The star
    // carries "is it rated" and the digit "how much", because a filled-vs-empty
    // star alone cannot tell 1 from 5.
    //
    // No tint: `star_on`/`star_off` are baked with the accent and secondary
    // tokens per theme, so the two textures already differ the way the rating
    // should read. `theme::icon` also falls back to the glyph when a theme ships
    // no file for the slot, which is the one case where a broken install would
    // otherwise draw nothing at all.
    let rating = info.map(|i| i.rating).unwrap_or(0);
    let (star, star_tex) = if rating > 0 {
        (theme::Icon::StarOn, star_on)
    } else {
        (theme::Icon::StarOff, star_off)
    };
    theme::icon(&mut row, star_tex, star, RATING_STAR);
    let digits = if rating > 0 {
        rating.to_string()
    } else {
        String::new()
    };
    let rating_resp = row
        .add_sized(
            egui::vec2(RATING_W - RATING_STAR, row_h),
            egui::Label::new(egui::RichText::new(&digits).color(if rating > 0 {
                p.accent
            } else {
                p.text_secondary
            }))
            .truncate()
            .sense(egui::Sense::click()),
        )
        .on_hover_text("Edit tags — click to change");
    // The rating is the thing you scan a column for, so the play count rides on
    // hover rather than in a counter column nobody asked for.
    let rating_resp = match app.play_history(path) {
        Some(history) => rating_resp.on_hover_text(history),
        None => rating_resp,
    };

    let add_clicked = row
        .add(egui::Button::new("+").small().frame(false))
        .on_hover_text_at_pointer("Add to playlist")
        .clicked();

    if title_resp.clicked() {
        Some(FileAct::Play)
    } else if add_clicked {
        Some(FileAct::Add)
    } else if rating_resp.clicked() {
        Some(FileAct::Edit)
    } else {
        None
    }
}

/// The file list shared by the local folder browser and the SMB share browser:
/// the search box, the sortable 4-column header, the folder/track/playlist rows
/// and the "Scanning…" note.
///
/// There is deliberately **no `..` row**: the breadcrumb is the only way up. It
/// walks every ancestor and makes each non-last segment a jump target, so "one
/// level up" is the second-to-last segment — a dedicated row duplicated
/// navigation, consumed a banded row, and counted toward the folder total in the
/// composition counts ("3 folders" when only 2 are). A share's breadcrumb works
/// the same way (Local / host / share / dir), which is why that view never had
/// one either.
///
/// `scan_note` is the status line under the list while it fills in, each caller
/// wording it for its own source: the local browser says "Scanning…", the share
/// browser counts down the tracks it is still downloading (`Network::pending_tags`),
/// because a share browse is a real transfer — a bare "Scanning…" on a 300-file
/// directory reads as a hung pane.
///
/// Returns the row the user clicked, if any; the caller carries it out, since only
/// it knows whether a `Nav` means `navigate_to` or `browse_open`. `remote` says
/// which browser this is — the share list's paths are `smb://` URIs, so arming a
/// playlist load needs to know which transport to run it through.
fn file_list_ui(
    app: &mut TPlayApp,
    themes: &ThemeState,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    entries: &[library::Entry],
    scan_note: Option<String>,
    remote: bool,
) -> Option<Act> {
    let p = theme.palette;
    let folder_tex = themes.icon(theme::Icon::Folder).cloned();
    let star_on = themes.icon(theme::Icon::StarOn).cloned();
    let star_off = themes.icon(theme::Icon::StarOff).cloned();
    let mut out = None;

    // Search box. State is the one `LIB_QUERY` key, so the query deliberately
    // survives a trip between the local and share browsers.
    let mut q = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp::<String>(egui::Id::new(LIB_QUERY))
            .unwrap_or_default()
    });
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Search")
                .small()
                .color(p.text_secondary),
        );
        ui.add(
            egui::TextEdit::singleline(&mut q)
                .hint_text("title, artist, album…")
                .desired_width(220.0),
        );
    });
    ui.ctx()
        .memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_QUERY), q.clone()));
    let query = q.trim().to_lowercase();
    ui.add_space(4.0);

    // Sortable column header only when the folder has audio files —
    // folders/playlists-only rows have no tag columns to align to.
    let has_audio = entries
        .iter()
        .any(|e| !e.is_dir() && !library::is_playlist(e.path()));
    if has_audio {
        let (cur, asc) = (app.library().sort(), app.library().sort_asc());
        let asc_tex = themes.icon(theme::Icon::SortAsc).cloned();
        let desc_tex = themes.icon(theme::Icon::SortDesc).cloned();
        ui.horizontal(|ui| {
            let (head, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
            ui.painter().rect_filled(head, 0.0, p.row_odd);
            let mut h = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        head.min + egui::vec2(6.0, 0.0),
                        head.max,
                    ))
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
                    if asc {
                        asc_tex.as_ref()
                    } else {
                        desc_tex.as_ref()
                    }
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
                        if app.playlist_dirty() {
                            dialogs::ask(
                                ui.ctx(),
                                dialogs::ConfirmAction::LoadPlaylist {
                                    path: path.to_path_buf(),
                                    remote,
                                },
                                "Load playlist",
                                &format!("Replace the current playlist with '{stem}'?"),
                            );
                        } else {
                            action = Some(Act::LoadPlaylist(path.to_path_buf()));
                        }
                    }
                } else {
                    let info = app.db().cache().get(path);
                    if !library::entry_matches(path, info, &query) {
                        i += 1;
                        continue;
                    }
                    match draw_file_row(
                        app,
                        ui,
                        theme,
                        row_h,
                        i,
                        path,
                        info,
                        star_on.as_ref(),
                        star_off.as_ref(),
                    ) {
                        Some(FileAct::Play) => action = Some(Act::Play(path.to_path_buf())),
                        Some(FileAct::Add) => action = Some(Act::Add(path.to_path_buf())),
                        // Display-only on a share. The cell still shows the rating
                        // the cached copy carries, and the click is dropped here
                        // rather than refused later: `tracks::write_tags` also
                        // refuses, so a future caller that forgets this guard still
                        // cannot write to a spool cache.
                        Some(FileAct::Edit) if remote => {}
                        Some(FileAct::Edit) => action = Some(Act::Edit(path.to_path_buf())),
                        None => {}
                    }
                }
                i += 1;
            }
            if i == 0 {
                ui.label(
                    egui::RichText::new("No files")
                        .small()
                        .color(p.text_secondary),
                );
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

/// The local folder browser's main column: composition counts + Add All, then
/// the shared file list. `remote_list_ui` is the same two steps with
/// `browse_open` behind `Nav`, which is why both end in `file_list_ui`.
pub fn local_list_ui(
    app: &mut TPlayApp,
    themes: &ThemeState,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
) {
    let entries = app.library().entries().to_vec();
    list_header_right(app, ui, theme.palette, &entries);
    ui.add_space(4.0);
    let act = file_list_ui(
        app,
        themes,
        ui,
        theme,
        &entries,
        app.library_scanning().then(|| "Scanning…".to_string()),
        false,
    );
    match act {
        Some(Act::Nav(dir)) => app.navigate_to(dir),
        Some(Act::Play(path)) => app.play_file(path),
        Some(Act::Add(path)) => app.add_files(vec![path]),
        Some(Act::LoadPlaylist(path)) => app.load_playlist_from(path),
        Some(Act::Edit(path)) => arm_edit(ui, app, path),
        None => {}
    }
}

/// Arm the track editor for a local row, carrying the tags the file has now so
/// the modal opens on them and needs no lookup of its own.
fn arm_edit(ui: &egui::Ui, app: &TPlayApp, path: PathBuf) {
    let info = app.db().cache().get(&path);
    dialogs::ask_edit(ui.ctx(), path, info);
}

/// The right-hand end of a file-list header: composition counts and Add All.
/// Shared because both browsers work from `&[library::Entry]` — a share's
/// entries carry `smb://` URIs, which go into the playlist unchanged.
///
/// **`Align::Min` is load-bearing, not cosmetic.** A horizontal `with_layout`
/// whose cross-axis align is `Center` or `Max` gives its child a `min_rect`
/// spanning the parent's *whole remaining height*, not the height used —
/// `scope_dyn` ends with `advance_cursor_after_rect(child.min_rect())`, so the
/// parent cursor jumps by that entire span. Measured in a 460px window with this
/// pane's real shape: `right_to_left(Center)` consumed **430px** of a 444px
/// column (leaving `available_height() == 0`), while `right_to_left(Min)`
/// consumed the **21px** it used. With `Center` the list's ScrollArea sat at
/// y≈497 — below the pane — and only the Add All row showed beside the sidebar.
///
/// The damage scales with the parent's available height, which is why it went
/// unnoticed elsewhere: every other `with_layout` in the app sits under a
/// *horizontal* parent (`coordinator.rs` top bar, `now_playing.rs` balance row,
/// `playlist.rs` rows, `visualizer.rs` header), where the child's available height
/// is one row. The one other vertical-parent site is `now_playing.rs`'s transport
/// block, whose pane is ~15% of the window, so the over-consumption is bounded by
/// an already-short height.
/// Regression test: `right_to_left_center_does_not_swallow_the_column` in
/// `tests/gui_tests.rs`.
fn list_header_right(
    app: &mut TPlayApp,
    ui: &mut egui::Ui,
    p: theme::Palette,
    entries: &[library::Entry],
) {
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

/// The remote browser's main column: the shared file list, with `browse_open`
/// standing in for `navigate_to`. The breadcrumb above it is `header`'s.
pub fn remote_list_ui(
    app: &mut TPlayApp,
    themes: &ThemeState,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    browse: &network::NetworkBrowse,
) {
    let p = theme.palette;
    let folder_tex = themes.icon(theme::Icon::Folder).cloned();

    // Busy / error states replace the list entirely: nothing to show while a
    // listing is in flight, and on a logon failure the main pane owes the user a
    // credential prompt rather than a file list.
    if browse.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                egui::RichText::new("Connecting…")
                    .small()
                    .color(p.text_secondary),
            );
        });
        return;
    }
    if let Some(err) = &browse.error {
        // SessionSetup is where a server rejects the *identity*, so a bare
        // NTSTATUS is not actionable — ask for credentials here, with the saved
        // address already filled in.
        if err.contains("LOGON_FAILURE") || err.contains("ACCESS_DENIED") {
            login_form_ui(app, ui, theme, browse);
        } else {
            ui.label(egui::RichText::new(err).small().color(p.text_secondary));
            ui.label(
                egui::RichText::new(
                    "Check the address, username/password, and that the share allows access.",
                )
                .small()
                .color(p.text_secondary),
            );
        }
        return;
    }

    // At the share-list stage there is no directory, so the shared list has
    // nothing to show: shares are a flat name list, and the breadcrumb's "Local"
    // is the way back out.
    if browse.share.is_none() {
        let mut i = 0usize;
        // A share name, NOT a URI — unlike every other row in this pane, the
        // share list is built from `RemoteEntry::name` and has no Entry to
        // carry a path. Named `share_name` so it can't be confused with one.
        let mut share_name: Option<String> = None;
        let mut names: Vec<&network::RemoteEntry> = browse.entries.iter().collect();
        names.sort_by_key(|e| e.name.to_lowercase());
        for e in names {
            if draw_dir_row(ui, theme, 24.0, i, &e.name, folder_tex.as_ref()) {
                share_name = Some(e.name.clone());
            }
            i += 1;
        }
        if i == 0 {
            ui.label(
                egui::RichText::new("No shares")
                    .small()
                    .color(p.text_secondary),
            );
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
    // file's URI, which is what lets the shared list treat a share exactly like a
    // folder: same tag columns, same sort keys, same playlist-file rows. Anything
    // neither audio nor a `.tplay` is not listed, same as a local folder.
    let mut entries: Vec<library::Entry> = browse
        .entries
        .iter()
        .filter(|e| {
            e.is_dir
                || library::is_audio(Path::new(&e.name))
                || library::is_playlist(Path::new(&e.name))
        })
        .map(|e| library::Entry {
            path: PathBuf::from(network::child_uri(&dir, &e.name)),
            is_dir: e.is_dir,
        })
        .collect();
    library::sort_entries(
        &mut entries,
        app.db().cache(),
        app.library().sort(),
        app.library().sort_asc(),
    );

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
    let untagged = audio
        .iter()
        .filter(|p| app.db().cache().get(*p).is_none())
        .count();
    app.ensure_tags(audio);

    // Show the outstanding transfer count, not just a bool: a share browse
    // downloads whole files, so this is a real wait and a bare "Scanning…" reads
    // as a hang. Count only what is actually in flight — a track that gave up
    // after its retries is not coming, so counting it would pin a number on
    // screen forever.
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
    let act = file_list_ui(app, themes, ui, theme, &entries, note, true);

    match act {
        Some(Act::Nav(path)) => {
            // The shared list hands back `entry.path()`, which for a share is the
            // FULL child URI the entry was built from — not a bare folder name.
            // Re-joining it with `child_browse` appended the whole URI as a name
            // segment, giving a `rel` of "music/smb://nas/media/music/Rock" and a
            // PATH_NOT_FOUND from the server. Split it instead: for a URI already
            // in child form that is a no-op round trip. (See
            // `nav_uri_round_trips_but_renaming_one_does_not` in
            // `tests/smb_helpers.rs`.)
            let uri = path.to_string_lossy().into_owned();
            if let Some((_host, share, rel)) = network::split_uri(&uri) {
                app.network_mut().browse_open(uri, share, rel);
            }
        }
        Some(Act::Play(path)) => app.play_file(path),
        Some(Act::Add(path)) => app.add_files(vec![path]),
        Some(Act::LoadPlaylist(uri)) => app.network_mut().fetch(uri.to_string_lossy().into_owned()),
        // Unreachable: `file_list_ui` drops an `Edit` when `remote` is set, since a
        // spool-cache copy is not the track. Named so that is a compile-time fact
        // rather than a comment.
        Some(Act::Edit(_)) => {}
        None => {}
    }
}

/// Credential prompt for a server that rejected our identity.
///
/// Deliberately in the **main** pane, not the sidebar: the address is already
/// known and saved, so only the login should ever need retyping. The username is
/// persisted with the server; the password stays in the session map only.
fn login_form_ui(
    app: &mut TPlayApp,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    browse: &network::NetworkBrowse,
) {
    let host = browse.host.clone();
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
        app.network_mut().add_server(host.clone(), user);
        app.network_mut().set_password(host.clone(), pass);
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
