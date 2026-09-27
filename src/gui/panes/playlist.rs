use crate::app::TPlayApp;
use crate::gui::dialogs;
use crate::gui::theme::{self, Icon, ThemeState};
use crate::library;
use crate::network;
use eframe::egui;
use std::path::{Path, PathBuf};

/// egui memory: the jump-to-file search text.
const PL_QUERY: &str = "tplay.playlist.query";
/// The playlist toolbar's icon size — the app's icon buttons all carry a literal
/// (13 in the row chrome, 18 in Now Playing's transport); this is the toolbar's.
const TOOLBAR_ICON: f32 = 16.0;

pub fn playlist_pane(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui) {
    // Owned Arc copy — panes take `&mut app` while using theme data.
    let theme = themes.current().clone();
    let p = theme.palette;
    let layout = theme.layout;

    // Header: the playlist's name (single source — app.playlist_name; follows
    // New/Save/Load live).
    let pl_name = app.playlist_name();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(pl_name).strong().color(p.text_primary));
    });
    ui.add_space(4.0);

    // Jump-to-file. Same box, same haystack and the same single `LIB_QUERY`-style
    // key shape as the Library's, so the two filters feel like one feature; the
    // text is held here, not the app, because it is view state.
    let query_id = egui::Id::new(PL_QUERY);
    let mut q: String = ui
        .ctx()
        .memory_mut(|m| m.data.get_temp::<String>(query_id))
        .unwrap_or_default();
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
        .memory_mut(|m| m.data.insert_temp(query_id, q.clone()));
    let query = q.trim().to_lowercase();
    ui.add_space(4.0);

    // The toolbar: one row of icons above the list — the three content ops on
    // the left, the two file actions hard right. Text labels would eat 500px of a
    // 320px minimum window; at `TOOLBAR_ICON` the whole row is ~230px, so it is a
    // `horizontal`, not a wrapping one, and every button carries a tooltip.
    //
    // A row above the list rather than the old bottom bar, because the footer
    // sized the ScrollArea from a hardcoded 30px and was drawn *after* it — a row
    // down there could only overflow what was reserved, while up here
    // `available_height()` already accounts for it and the list gets the rest.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        // The sort control is the one button that keeps a text label, and it is
        // the right place for text: it is a ComboBox because it *picks a column*
        // out of `SORT_OPTIONS`, and an icon could only say "something about
        // order" — the two arrows that exist (`sort_asc`/`sort_desc`) mean the
        // Library's *active column direction*, a different piece of state, and
        // there is no third arrow to spend. The label is always "Sort by…"
        // because the ops are one-shot: there is no standing order for it to
        // report, and the dropdown is where the column names live.
        egui::ComboBox::from_id_salt("playlist_sort_by")
            .selected_text("Sort by…")
            .show_ui(ui, |ui| {
                for (i, name) in library::SORT_OPTIONS.iter().enumerate() {
                    // Nothing is ever "selected": each entry applies immediately,
                    // the way a menu item does, so a checkmark would be a lie.
                    if ui.selectable_label(false, *name).clicked() {
                        app.sort_playlist(i);
                    }
                }
            });

        if theme::icon_button(
            ui,
            themes.icon(Icon::Reverse),
            Icon::Reverse,
            TOOLBAR_ICON,
            true,
            false,
        )
        .on_hover_text("Play the playlist back to front")
        .clicked()
        {
            app.reverse_playlist();
        }
        // `Shuffle`, not a new icon: randomizing once IS a shuffle, and the same
        // mark already means "shuffle" on the transport row.
        if theme::icon_button(
            ui,
            themes.icon(Icon::Shuffle),
            Icon::Shuffle,
            TOOLBAR_ICON,
            true,
            false,
        )
        .on_hover_text("Shuffle the playlist's order once")
        .clicked()
        {
            app.randomize_playlist();
        }

        // `Align::Min`, NOT `Center` — see `list_header_right` in
        // library/listing.rs for the measurement.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // Right-to-left puts the first widget furthest right, so Save is added
            // first to read Create → Save left to right.
            let save_hover = app
                .playlist_file()
                .map(|p| format!("Overwrite playlist: {}", p.display()))
                .unwrap_or_else(|| "Save the playlist to a .tplay file".into());
            if theme::icon_button(
                ui,
                themes.icon(Icon::Save),
                Icon::Save,
                TOOLBAR_ICON,
                true,
                false,
            )
            .on_hover_text(save_hover)
            .clicked()
            {
                save_playlist(app, ui.ctx());
            }
            if theme::icon_button(
                ui,
                themes.icon(Icon::NewList),
                Icon::NewList,
                TOOLBAR_ICON,
                true,
                false,
            )
            .on_hover_text("Create a new, empty playlist")
            .clicked()
            {
                if app.playlist_dirty() {
                    dialogs::ask(
                        ui.ctx(),
                        dialogs::ConfirmAction::NewPlaylist,
                        "New playlist",
                        "Discard unsaved changes and start a new playlist?",
                    );
                } else {
                    app.new_playlist();
                }
            }
        });
    });
    ui.add_space(4.0);

    let drag_from_id = egui::Id::new("tplay.drag_from");
    let drag_hover_id = egui::Id::new("tplay.drag_hover");

    // Drag state persists across frames via egui memory
    let mut drag_from = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp::<Option<usize>>(drag_from_id)
            .unwrap_or(None)
    });
    let mut drag_hover = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp::<Option<usize>>(drag_hover_id)
            .unwrap_or(None)
    });

    // The list gets everything that is left, which is the whole reason the
    // actions moved above it: there is no row after this one to reserve space
    // for, so the old `available_height() - footer_h` guess is gone. The 40px
    // floor is the one thing that survives — a pane squeezed to nothing should
    // still show one row rather than none.
    let scroll_h = ui.available_height().max(40.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(scroll_h)
        .show(ui, |ui| {
            let mut to_delete: Option<usize> = None;
            let mut shown = 0usize;
            let row_h = 24.0;

            for i in 0..app.playlist().len() {
                let path = &app.playlist()[i];
                // Tagged title (filename stands in until the scan lands; the
                // cache is shared with the Library, so visited folders show tags
                // instantly).
                let info = app.db().cache().get(path);
                // Filtering skips a *row*, never reindexes the list: `i` stays
                // the true playlist index, so the banding, the ✕, the click and
                // the drag all act on the track that is actually there.
                if !row_matches(path, info, &query) {
                    continue;
                }
                shown += 1;
                let is_current = app.current_index() == Some(i);
                let name = library::title_or_stem(path, info);
                let fmt = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_uppercase())
                    .unwrap_or_default();
                // Secondary artist · album block (right of the title, before FORMAT).
                let meta = {
                    let mut parts: Vec<&str> = Vec::new();
                    if let Some(i) = info {
                        for s in [i.artist.as_str(), i.album.as_str()] {
                            if !s.is_empty() {
                                parts.push(s);
                            }
                        }
                    }
                    parts.join(" · ")
                };

                // Fixed-height row rect; background, full-row highlight and the
                // accent stripe all go under the widgets. Drag hover too.
                let (rect, mut row) =
                    theme::row(ui, i, is_current || drag_hover == Some(i), row_h, &theme);
                row.spacing_mut().item_spacing.x = 8.0;

                row.label(
                    egui::RichText::new(format!("{:>2}.", i + 1))
                        .color(p.text_secondary)
                        .font(egui::FontId::new(
                            layout.text_meta,
                            theme.metadata_font.clone(),
                        )),
                );

                // Title fills the remaining width. The right side is the ✕ button
                // and FORMAT cell when there is no secondary block, and adds the
                // 150px artist·album cell when there is one.
                let right_w = 130.0 + if meta.is_empty() { 0.0 } else { 158.0 };
                let title_resp = row.add_sized(
                    egui::vec2((rect.width() - right_w).max(40.0), row_h),
                    egui::Label::new(egui::RichText::new(&name).color(if is_current {
                        p.text_primary
                    } else {
                        p.text_primary.gamma_multiply(0.85)
                    }))
                    .truncate()
                    .sense(egui::Sense::click_and_drag()),
                );

                if title_resp.drag_started() {
                    drag_from = Some(i);
                    drag_hover = None;
                }

                if title_resp.dragged() {
                    ui.painter().text(
                        title_resp.rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &name,
                        egui::FontId::new(layout.text_time, theme.metadata_font.clone()),
                        p.accent,
                    );
                }

                if title_resp.clicked() {
                    app.play_track(i);
                }

                row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(
                        ui,
                        themes.icon(Icon::Remove),
                        Icon::Remove,
                        13.0,
                        true,
                        false,
                    )
                    .clicked()
                    {
                        to_delete = Some(i);
                    }
                    if !fmt.is_empty() {
                        ui.label(
                            egui::RichText::new(fmt.clone())
                                .color(p.text_secondary)
                                .font(egui::FontId::new(
                                    layout.text_meta,
                                    theme.metadata_font.clone(),
                                )),
                        );
                    }
                    if !meta.is_empty() {
                        ui.add_sized(
                            egui::vec2(150.0, row_h),
                            egui::Label::new(
                                egui::RichText::new(meta.clone()).color(p.text_secondary),
                            )
                            .truncate(),
                        );
                    }
                });

                let row_rect = rect;
                if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    if drag_from.is_some() && drag_from != Some(i) && row_rect.contains(pointer_pos)
                    {
                        drag_hover = Some(i);
                    }
                }

                if title_resp.drag_stopped() {
                    if let (Some(from), Some(to)) = (drag_from, drag_hover) {
                        app.move_track(from, to);
                    }
                    drag_from = None;
                    drag_hover = None;
                }
            }

            // Reset drag if pointer released outside any item
            if drag_from.is_some() && ui.input(|i| i.pointer.any_released()) {
                drag_from = None;
                drag_hover = None;
            }

            // Only a filtered-out list needs saying so — an empty playlist is
            // already obvious.
            if shown == 0 && !query.is_empty() {
                ui.label(
                    egui::RichText::new("No tracks match")
                        .small()
                        .color(p.text_secondary),
                );
            }

            if let Some(idx) = to_delete {
                let name = app
                    .playlist()
                    .get(idx)
                    .map(|p| library::title_or_stem(p, app.db().cache().get(p)))
                    .unwrap_or_default();
                // Armed, not run: the index can be gone by the time the modal is
                // answered (an in-flight remote playlist can replace the list),
                // and `remove_track` panics on a stale one.
                dialogs::ask(
                    ui.ctx(),
                    dialogs::ConfirmAction::RemoveTrack(idx),
                    "Remove track",
                    &format!("Remove '{name}' from the playlist?"),
                );
            }
        });

    // Persist drag state for the next frame
    ui.ctx().memory_mut(|m| {
        m.data.insert_temp(drag_from_id, drag_from);
        m.data.insert_temp(drag_hover_id, drag_hover);
    });
}

/// Carry out a Save click: overwrite the tracked file, else ask for a name over
/// the directory on screen. Split out of the toolbar because the three-branch
/// rule is the interesting part and it reads better as its own function than as
/// a hundred lines inside a `with_layout` closure.
fn save_playlist(app: &mut TPlayApp, ctx: &egui::Context) {
    if let Some(path) = app.playlist_file().map(PathBuf::from) {
        // Tracked file: overwrite, no prompt. An smb:// URI overwrites on the
        // server.
        app.save_playlist_to(path);
    } else if let Some(dir) = browsing_share_dir(app) {
        // Browsing a share: a name over the directory on screen, so no mount and
        // no temp file. No dialog.
        dialogs::ask_save_name(
            ctx,
            dialogs::SaveTarget::PlaylistShare,
            dir,
            library::default_playlist_name(app.playlist_file()),
        );
    } else {
        // Local: a name over the Library's current folder — the user navigates
        // there first for a different directory, the same rule the share branch
        // follows.
        dialogs::ask_save_name(
            ctx,
            dialogs::SaveTarget::PlaylistLocal,
            app.library().dir().to_string_lossy().into_owned(),
            library::default_playlist_name(app.playlist_file()),
        );
    }
}

/// Where a share-save would write: the share/dir open in the Library's remote
/// browser. `None` at the share-list stage (no directory on screen) and while a
/// listing is in flight, so the caller falls back to the local save dialog
/// rather than guessing a target.
fn browsing_share_dir(app: &TPlayApp) -> Option<String> {
    let b = app.network().browse()?;
    if b.busy {
        return None;
    }
    let share = b.share.as_ref()?;
    Some(network::dir_uri(&b.host, share, &b.rel))
}

/// Does a row survive the search text? The haystack is the Library's
/// title·artist·album, plus the filename so "jump to file" works on a track the
/// tag scan has not reached yet.
///
/// `query` arrives trimmed and lowercased — the caller folds the case once per
/// frame, so the folding here is the haystack's. `pub` and free of any
/// `egui::Ui` so the filtering is testable headlessly, the rows themselves are
/// not. An empty query short-circuits, which is also what keeps an unfiltered
/// list from building a haystack string per row per frame.
pub fn row_matches(path: &Path, info: Option<&library::TrackInfo>, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let title = library::title_or_stem(path, info);
    let hay = format!(
        "{title} {} {} {}",
        info.map(|i| i.artist.as_str()).unwrap_or_default(),
        info.map(|i| i.album.as_str()).unwrap_or_default(),
        path.file_name().unwrap_or_default().to_string_lossy(),
    )
    .to_lowercase();
    hay.contains(query)
}
