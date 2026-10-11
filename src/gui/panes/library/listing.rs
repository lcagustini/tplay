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
use crate::gui::panes::row_visible;
use crate::gui::panes::IpodDirection;
use crate::gui::theme::{self, ThemeState};
use crate::ipod::remove;
use crate::library;
use crate::network;
use eframe::egui;
use std::path::{Path, PathBuf};

/// egui memory: the active search filter text.
const LIB_QUERY: &str = "tplay.library.query";
/// egui memory: the main-pane login prompt, keyed per host (`(NET_LOGIN, host)`).
const NET_LOGIN: &str = "tplay.network.login";

/// Width of the `+` button plus the inter-cell spacing that no token owns. The
/// rest of a row's fixed width is `rows.columns` and the rating cell.
const ACTION_W: f32 = 40.0;

/// The rating cell: a star and its digit, side by side. One row wide, and the star
/// is 5/24 of it — both *derived from the row height* rather than written down,
/// which is what keeps the star's own comment ("half the row height") true after a
/// theme changes `rows.height`.
///
/// **Not** one of `rows.columns`, because those are the sortable columns and this
/// is not one — the header draws one cell per `SORT_OPTIONS` entry and stops, so
/// the rating sits past the last header cell and the columns still line up. It is
/// not sortable on purpose: rating is a whole-star field, so ordering by it would
/// produce long runs of indistinguishable rows.
fn rating_w(rows: &theme::Rows) -> f32 {
    rows.height
}

/// Fixed right-hand width per row: the tag cells + the rating cell + the `+` button
/// and its spacing, leaving the title column the flexible remainder.
fn fixed_w(rows: &theme::Rows) -> f32 {
    rows.columns[1] + rows.columns[2] + rows.columns[3] + rating_w(rows) + ACTION_W
}

/// One clickable column header; true when clicked, and the caller picks the sort
/// key. The active column is accent-colored with a theme icon (`arrow`) for the
/// direction; clicking it again flips it in `TPlayApp::set_library_sort`.
fn header_cell(
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    label: &str,
    active: bool,
    arrow: Option<&egui::TextureHandle>,
    w: f32,
    row_h: f32,
) -> bool {
    let p = theme.palette;
    let color = if active { p.accent } else { p.text_secondary };
    let btn = match arrow {
        Some(tex) => egui::Button::image_and_text(
            egui::Image::new(tex)
                .fit_to_exact_size(egui::vec2(theme.panes.tiny_icon, theme.panes.tiny_icon)),
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
    let rows = theme.rows;
    let rect = theme::row(ui, i, false, row_h, theme).0;
    let mut text_x = rect.min.x + rows.inset;
    if let Some(tex) = folder {
        let s = theme.panes.folder_icon;
        let img_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + rows.inset, rect.center().y - s / 2.0),
            egui::vec2(s, s),
        );
        ui.painter().image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        text_x += s + rows.cell_gap;
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
        theme::meta_font(&theme.type_),
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
    let rows = theme.rows;
    let (rect, mut row) = theme::row(ui, i, false, row_h, theme);

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let title_resp = row
        .add_sized(
            egui::vec2(
                (rect.width() - fixed_w(&rows) - rows.inset).max(rows.title_floor),
                row_h,
            ),
            egui::Label::new(
                egui::RichText::new(&name).color(p.text_primary.gamma_multiply(rows.dim)),
            )
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
    /// Take the row's track off an iPod. Only ever returned for a
    /// [`Source::Device`] row — the shared list widget is handed the source, not
    /// a guess.
    Remove,
}

/// Which browser is drawing this list.
///
/// A share row is read-only where a device row is not: its ratings come from a
/// spool-cache copy rather than the track, and it is never a file this app may
/// delete. A device row is the reverse — it is the one list whose row button
/// means something destructive. One enum rather than two `bool`s because the two
/// facts are not independent, and `true, true` is a combination no caller means
/// and no reader would believe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Local,
    Share,
    Device,
}

impl Source {
    /// Whether a row can open the track editor.
    fn editable(self) -> bool {
        !matches!(self, Self::Share)
    }
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
    /// A sortable header cell was clicked. The **caller** carries it out, for the
    /// same reason `file_list_ui` is a reader: one `&mut` here would force the
    /// caller to clone the folder's whole entry list to pass a borrow alongside it.
    Sort(usize),
    /// Take this track off an iPod: its file, its record, its playlist entries.
    /// The **caller** carries it out, because only it can arm the confirm modal.
    Remove(PathBuf),
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
    source: Source,
) -> Option<FileAct> {
    let p = theme.palette;
    let rows = theme.rows;
    // A spooled `smb://` track is "current" under its URI, which is what the
    // playlist holds — so the highlight follows the track across both sources.
    let is_current = app.current_path().is_some_and(|c| c == path);
    let (rect, mut row) = theme::row(ui, i, is_current, row_h, theme);

    let title = library::title_or_stem(path, info);
    let display = match info.and_then(|i| i.track_no.clone()) {
        Some(n) => format!("{n}. {title}"),
        None => title,
    };
    let artist = info.map(|i| i.artist.as_str()).unwrap_or_default();
    let album = info.map(|i| i.album.as_str()).unwrap_or_default();
    let dur = info.and_then(|i| i.duration);

    // Title takes the flexible remainder; the tag cells mirror the header.
    let title_w = (rect.width() - fixed_w(&rows) - rows.inset).max(rows.title_floor);
    let title_resp = row
        .add_sized(
            egui::vec2(title_w, row_h),
            egui::Label::new(egui::RichText::new(&display).color(if is_current {
                p.text_primary
            } else {
                p.text_primary.gamma_multiply(rows.dim)
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
    cell(&mut row, rows.columns[1], artist);
    cell(&mut row, rows.columns[2], album);
    row.add_sized(
        egui::vec2(rows.columns[3], row_h),
        egui::Label::new(
            egui::RichText::new(TPlayApp::fmt_duration(dur))
                .color(p.text_secondary)
                .font(theme::meta_font(&theme.type_)),
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
    theme::icon(&mut row, star_tex, star, theme.panes.tiny_icon);
    let digits = if rating > 0 {
        rating.to_string()
    } else {
        String::new()
    };
    let rating_resp = row
        .add_sized(
            egui::vec2(rating_w(&rows) - theme.panes.tiny_icon, row_h),
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

    // One row button, two meanings: on a folder of an iPod the useful action is
    // "take this off the device", and adding a second button to a row whose
    // width is already accounted for to the pixel is the wrong trade for it. The
    // `✕` is the Playlist pane's own spelling of the same verb.
    let removable = source == Source::Device;
    let (glyph, hover) = if removable {
        ("✕", "Remove from the iPod")
    } else {
        ("+", "Add to playlist")
    };
    let add_clicked = row
        .add(egui::Button::new(glyph).small().frame(false))
        .on_hover_text_at_pointer(hover)
        .clicked();

    if title_resp.clicked() {
        Some(FileAct::Play)
    } else if add_clicked {
        if removable {
            Some(FileAct::Remove)
        } else {
            Some(FileAct::Add)
        }
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
/// it knows whether a `Nav` means `navigate_to` or `browse_open`. `source` says
/// which browser this is, and it is one enum rather than two `bool`s because the
/// two facts are not independent: a share list's paths are `smb://` URIs that are
/// never a device's own files, so arming a playlist load or a removal has to know
/// which transport it is running through.
/// **`&TPlayApp`, and that is the whole reason the caller does not clone the entry
/// list.** It needed `&mut` for exactly one call — `set_library_sort` from the
/// header — and that one mutation is now returned as an [`Act::Sort`] for the
/// caller to carry out, which is how every other action in this enum already
/// worked. What the borrow cost was a **deep clone of every `PathBuf` in the
/// folder, every frame**: a 2 000-track library allocated 2 000 paths sixty
/// times a second to hand a slice to a function that only reads it.
fn file_list_ui(
    app: &TPlayApp,
    themes: &ThemeState,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    entries: &[library::Entry],
    scan_note: Option<String>,
    source: Source,
) -> Option<Act> {
    let p = theme.palette;
    let folder_tex = themes.icon(theme::Icon::Folder).cloned();
    let star_on = themes.icon(theme::Icon::StarOn).cloned();
    let star_off = themes.icon(theme::Icon::StarOff).cloned();
    let mut out = None;
    // The sortable header is drawn *above* the scroll area, so its click cannot
    // return the way a row does. It parks here and is folded into `out` after the
    // rows, which is the same one-action-out-of-the-bottom rule every other
    // `Act` follows.
    let mut action_sort: Option<usize> = None;

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
                .desired_width(theme.panes.field_w),
        );
    });
    ui.ctx()
        .memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_QUERY), q.clone()));
    let query = q.trim().to_lowercase();
    ui.add_space(theme.panes.gap);

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
            let (head, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), theme.rows.header_h),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(head, 0.0, p.row_odd);
            let mut h = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        head.min + egui::vec2(theme.rows.inset, 0.0),
                        head.max,
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            for (key, label) in library::SORT_OPTIONS.iter().enumerate() {
                let w = if key == 0 {
                    (head.width() - fixed_w(&theme.rows)).max(2.0)
                } else {
                    theme.rows.columns[key]
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
                if header_cell(
                    &mut h,
                    theme,
                    label,
                    key == cur,
                    arrow,
                    w,
                    theme.rows.header_h,
                ) {
                    action_sort = Some(key);
                }
            }
        });
    }

    // One row's height is both the reserve for the counts line above and the floor
    // under the list, so neither figure is written down twice.
    let scroll_h = (ui.available_height() - theme.rows.height).max(theme.rows.height);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(scroll_h)
        .show(ui, |ui| {
            let row_h = theme.rows.height;
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
                    // A `ScrollArea` lays out and paints **every** row it is
                    // given, scrolled out of sight or not, and a row here is a
                    // dozen widgets and several text layouts. A 2 000-file folder
                    // was doing all of that on every frame to show the ~30 that
                    // fit. The height is still reserved off-screen — reserve
                    // nothing and the scrollbar collapses to the visible slice.
                    if row_visible(ui, row_h) {
                        if draw_dir_row(ui, theme, row_h, i, &name, folder_tex.as_ref()) {
                            action = Some(Act::Nav(path.to_path_buf()));
                        }
                    } else {
                        ui.allocate_space(egui::vec2(0.0, row_h));
                    }
                } else if library::is_playlist(path) {
                    if !query.is_empty() && !path.to_string_lossy().to_lowercase().contains(&query)
                    {
                        i += 1;
                        continue;
                    }
                    if !row_visible(ui, row_h) {
                        ui.allocate_space(egui::vec2(0.0, row_h));
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
                                    remote: source == Source::Share,
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
                    if !row_visible(ui, row_h) {
                        ui.allocate_space(egui::vec2(0.0, row_h));
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
                        source,
                    ) {
                        Some(FileAct::Play) => action = Some(Act::Play(path.to_path_buf())),
                        Some(FileAct::Add) => action = Some(Act::Add(path.to_path_buf())),
                        Some(FileAct::Remove) => action = Some(Act::Remove(path.to_path_buf())),
                        // Display-only on a share. The cell still shows the rating
                        // the cached copy carries, and the click is dropped here
                        // rather than refused later: `tracks::write_tags` also
                        // refuses, so a future caller that forgets this guard still
                        // cannot write to a spool cache.
                        Some(FileAct::Edit) if !source.editable() => {}
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
    if let Some(key) = action_sort {
        // A sort click wins over any row click: the header was drawn first, so
        // it cannot be the later of the two.
        out = Some(Act::Sort(key));
    }
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
    // A borrow, not a clone. The clone existed only because both calls below
    // wanted `&mut TPlayApp` while `entries` came out of it; both are readers
    // now, so the folder is not walked at all.
    let on_device = browsing_device(app.library().dir());
    let source = if on_device {
        Source::Device
    } else {
        Source::Local
    };
    list_header_right(
        app,
        ui,
        theme.palette,
        Counts::of(app.library().entries()),
        on_device,
    );
    ui.add_space(theme.panes.gap);
    let scanning = app.library_scanning();
    let act = file_list_ui(
        app,
        themes,
        ui,
        theme,
        app.library().entries(),
        scanning.then(|| "Scanning…".to_string()),
        source,
    );
    match act {
        Some(Act::Nav(dir)) => app.navigate_to(dir),
        Some(Act::Play(path)) => app.play_file(path),
        Some(Act::Add(path)) => app.add_files(vec![path]),
        Some(Act::LoadPlaylist(path)) => app.load_playlist_from(path),
        Some(Act::Edit(path)) => arm_edit(ui, app, path),
        Some(Act::Sort(key)) => app.set_library_sort(key),
        Some(Act::Remove(path)) => arm_remove(ui, path),
        None => {}
    }
}

/// Whether this folder is **on** an iPod, rather than merely beside one.
///
/// An ancestor holding `iPod_Control` is the whole test, and it is a path walk
/// rather than `device::find()` — which reads `/proc/self/mounts` and probes
/// every mounted volume, and this runs on every frame of a pane. The check is
/// also the honest one: it asks where the file is, not whether a device happens
/// to be plugged in, so the button appears for a folder that *is* the device's
/// even if some other iPod is the one that would be written to.
fn browsing_device(dir: &Path) -> bool {
    dir.ancestors().any(|a| a.join("iPod_Control").is_dir())
}

/// Arm the remove-from-iPod confirm for one row's track.
fn arm_remove(ui: &egui::Ui, path: PathBuf) {
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    dialogs::ask(
        ui.ctx(),
        dialogs::ConfirmAction::RemoveFromIpod {
            paths: vec![path],
            what: name.clone(),
        },
        "Remove from the iPod",
        &format!(
            "Take '{name}' off the iPod?\n\nIts file, its library entry and its playlist entries \
             all go. A copy of the previous database is kept in tplay's cache folder."
        ),
    );
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
/// What a folder is made of, for the "N tracks · M folders · K playlists" line.
///
/// A named struct rather than three `usize` arguments because the three are
/// adjacent and interchangeable: `list_header_right(app, ui, p, d, t, pl)` compiles,
/// and the only symptom is the header reporting the wrong folder. One value is
/// also what let the counting move out of both callers — it is the *same* count
/// for the local list and the share browser, and having it inlined in each meant
/// two copies of a loop that both had to be right.
#[derive(Clone, Copy)]
struct Counts {
    tracks: usize,
    dirs: usize,
    playlists: usize,
}

impl Counts {
    /// One pass, and the entry kinds are mutually exclusive: a row is a directory,
    /// a playlist file, or a track, which is the same partition both browsers'
    /// rows already make when they draw them.
    fn of(entries: &[library::Entry]) -> Self {
        let mut c = Self {
            tracks: 0,
            dirs: 0,
            playlists: 0,
        };
        for e in entries {
            if e.is_dir() {
                c.dirs += 1;
            } else if library::is_playlist(e.path()) {
                c.playlists += 1;
            } else {
                c.tracks += 1;
            }
        }
        c
    }
}

fn list_header_right(
    app: &mut TPlayApp,
    ui: &mut egui::Ui,
    p: theme::Palette,
    c: Counts,
    on_device: bool,
) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if ui
            .button("Add All")
            .on_hover_text("Add this folder's tracks to the playlist")
            .clicked()
        {
            // Walked only on the click, and the borrow ends before `add_files` —
            // the other half of why this is a reader: a slice of the entries
            // could not be held across that call.
            let files: Vec<PathBuf> = app
                .library()
                .entries()
                .iter()
                .filter(|e| !e.is_dir() && !library::is_playlist(e.path()))
                .map(|e| e.path().to_path_buf())
                .collect();
            app.add_files(files);
        }
        // The same button in both directions: browsing the device's own folder is
        // what makes "take these off" meaningful, and a permanently-greyed button
        // beside an enabled one reads as a broken app rather than as an
        // inapplicable action.
        let direction = if on_device {
            IpodDirection::Remove
        } else {
            IpodDirection::Add
        };
        if super::super::ipod_button(ui, app, p, direction) {
            // Every playable file under this folder, at any depth. **Recursive
            // because the folder the user browsed to is not always an album**:
            // `iPod_Control/Music` holds fifty `Fnn` folders and no files at all,
            // so a one-level walk found zero tracks and the button did nothing.
            // Walked on the click, for the same reason `Add All` walks then: a
            // list handed to a dialog that may never open is a list built sixty
            // times a second for nothing.
            let files = remove::audio_files_under(app.library().dir());
            let name = app.library().dir().file_name().map_or_else(
                || match direction {
                    IpodDirection::Remove => "this folder".to_string(),
                    IpodDirection::Add => "this album".to_string(),
                },
                |n| n.to_string_lossy().into_owned(),
            );
            let n = files.len();
            if direction == IpodDirection::Remove {
                dialogs::ask(
                    ui.ctx(),
                    dialogs::ConfirmAction::RemoveFromIpod {
                        paths: files,
                        what: name.clone(),
                    },
                    "Remove from the iPod",
                    &format!(
                        "Take {n} track{} off the iPod from '{name}'?\n\n\
                         Everything in this folder and its subfolders goes: the files, their \
                         library entries and their playlist entries. A copy of the previous \
                         database is kept in tplay's cache folder.",
                        if n == 1 { "" } else { "s" }
                    ),
                );
            } else {
                dialogs::ask(
                    ui.ctx(),
                    dialogs::ConfirmAction::PushToIpod {
                        tracks: files,
                        playlist: None,
                    },
                    "Copy to the iPod",
                    &format!(
                        "Copy {n} track{} from '{name}' to the connected iPod?\n\n\
                         Everything in this folder and its subfolders is included. Its database \
                         is rewritten and the previous one is kept in tplay's cache folder.",
                        if n == 1 { "" } else { "s" }
                    ),
                );
            }
        }
        // "1 track" stays singular. The three counts arrive as numbers: they are
        // identical for both callers, and counting them here meant either a slice
        // the caller had to own or a walk repeated on every frame.
        let n = |n: usize, s: &str| format!("{n} {s}{}", if n == 1 { "" } else { "s" });
        ui.label(
            egui::RichText::new(format!(
                "{} · {} · {}",
                n(c.tracks, "track"),
                n(c.dirs, "folder"),
                n(c.playlists, "playlist"),
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
            if draw_dir_row(
                ui,
                theme,
                theme.rows.height,
                i,
                &e.name,
                folder_tex.as_ref(),
            ) {
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

    // A share is never a device's own folder, so the direction is fixed here rather
    // than by a second check.
    list_header_right(app, ui, p, Counts::of(&entries), false);
    ui.add_space(theme.panes.gap);
    let act = file_list_ui(app, themes, ui, theme, &entries, note, Source::Share);

    match act {
        Some(Act::Nav(path)) => {
            // The shared list hands back `entry.path()`, which for a share is the
            // FULL child URI, not a bare folder name — so split it rather than
            // re-joining it as a name, which is what a `child_browse`-shaped
            // helper invites and what cost a PATH_NOT_FOUND here once. For a URI
            // already in child form this is a no-op round trip, pinned by
            // `nav_uri_round_trips_but_renaming_one_does_not`.
            let uri = path.to_string_lossy().into_owned();
            if let Some((_host, share, rel)) = network::split_uri(&uri) {
                app.network_mut().browse_open(uri, share, rel);
            }
        }
        Some(Act::Play(path)) => app.play_file(path),
        Some(Act::Add(path)) => app.add_files(vec![path]),
        Some(Act::LoadPlaylist(uri)) => app.network_mut().fetch(uri.to_string_lossy().into_owned()),
        // The header is the shared one, so the sort is the same call as on disk —
        // `sort_entries` reads a track id and an `smb://` URI is one.
        Some(Act::Sort(key)) => app.set_library_sort(key),
        // Unreachable: `file_list_ui` drops an `Edit` on a share, since a spool-cache
        // copy is not the track, and a `Remove` is only ever returned for a device
        // row. Named so that is a compile-time fact rather than a comment.
        Some(Act::Edit(_)) | Some(Act::Remove(_)) => {}
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
    ui.add_space(theme.panes.gap);

    // Enter in either field submits (TextEdit surrenders focus on Enter).
    let mut enter = false;
    let field_w = theme.panes.field_w;
    let mut field = ui.add_sized(
        egui::vec2(field_w, theme.rows.height),
        egui::TextEdit::singleline(&mut creds.0)
            .hint_text("username")
            .desired_width(field_w)
            .clip_text(true),
    );
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        enter = true;
    }
    field = ui.add_sized(
        egui::vec2(field_w, theme.rows.height),
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
