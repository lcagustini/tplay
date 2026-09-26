use crate::app::TPlayApp;
use crate::gui::theme::{self, Icon};
use crate::library;
use crate::network;
use eframe::egui;
use std::path::PathBuf;

/// egui memory: the share-save prompt's state, `Option<(dir_uri, filename)>`.
const SAVE_SHARE_ID: &str = "tplay.playlist.save_share";
/// Width of the share-save name field.
const SAVE_SHARE_FIELD_W: f32 = 220.0;

pub fn playlist_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    // Owned Arc copy — panes call &mut app while using theme data.
    let theme = app.theme().clone();
    let p = theme.palette;
    let layout = theme.layout.with_defaults();

    // Header: the current playlist's name (single source — app.playlist_name;
    // follows New/Save/Load live).
    let pl_name = app.playlist_name();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(pl_name).strong().color(p.text_primary));
    });
    ui.add_space(4.0);

    let drag_from_id = egui::Id::new("tplay.drag_from");
    let drag_hover_id = egui::Id::new("tplay.drag_hover");

    // Drag state persists across frames via egui memory
    let mut drag_from = ui.ctx().memory_mut(|m| m.data.get_temp::<Option<usize>>(drag_from_id).unwrap_or(None));
    let mut drag_hover = ui.ctx().memory_mut(|m| m.data.get_temp::<Option<usize>>(drag_hover_id).unwrap_or(None));

    // The inner ScrollArea would otherwise consume every free pixel
    // (auto_shrink(false, false) sizes to the full available rect), leaving
    // zero height for the footer row below and clipping it invisible.
    let footer_h = 30.0;
    let scroll_h = (ui.available_height() - footer_h).max(40.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(scroll_h)
        .show(ui, |ui| {
            let mut to_delete: Option<usize> = None;
            let row_h = 24.0;

            for i in 0..app.playlist().len() {
                let is_current = app.current_index() == Some(i);
                let path = &app.playlist()[i];
                // Tagged title (filename stands in until the scan lands; the
                // cache is shared with the Library, so already-visited folders
                // show tags instantly).
                let info = app.track_info(path);
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
                // accent stripe all go under the widgets. Highlight on drag hover too.
                let (rect, mut row) = theme::row(ui, i, is_current || drag_hover == Some(i), row_h, &theme);
                row.spacing_mut().item_spacing.x = 8.0;

                row.label(
                    egui::RichText::new(format!("{:>2}.", i + 1))
                        .color(p.text_secondary)
                        .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                );

                // Title fills the remaining width (meta + fmt + ✕ right-aligned).
                let right_w = 130.0 + if meta.is_empty() { 0.0 } else { 158.0 };
                let title_resp = row.add_sized(
                    egui::vec2((rect.width() - right_w).max(40.0), row_h),
                    egui::Label::new(
                        egui::RichText::new(&name).color(if is_current { p.text_primary } else { p.text_primary.gamma_multiply(0.85) })
                    )
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
                    if theme::icon_button(ui, app.theme_icon(Icon::Remove), Icon::Remove, 13.0, true, false).clicked() {
                        to_delete = Some(i);
                    }
                    if !fmt.is_empty() {
                        ui.label(
                            egui::RichText::new(fmt.clone())
                                .color(p.text_secondary)
                                .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                        );
                    }
                    if !meta.is_empty() {
                        ui.add_sized(
                            egui::vec2(150.0, row_h),
                            egui::Label::new(egui::RichText::new(meta.clone()).color(p.text_secondary)).truncate(),
                        );
                    }
                });

                let row_rect = rect;
                if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    if drag_from.is_some() && drag_from != Some(i) {
                        if row_rect.contains(pointer_pos) {
                            drag_hover = Some(i);
                        }
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

            if let Some(idx) = to_delete {
                let name = app
                    .playlist()
                    .get(idx)
                    .map(|p| library::title_or_stem(p, app.track_info(p)))
                    .unwrap_or_default();
                if TPlayApp::confirm("Remove track", &format!("Remove '{name}' from the playlist?"), true) {
                    app.remove_track(idx);
                }
            }
        });

    // Persist drag state for the next frame
    ui.ctx().memory_mut(|m| {
        m.data.insert_temp(drag_from_id, drag_from);
        m.data.insert_temp(drag_hover_id, drag_hover);
    });

    // Bottom actions: Create Playlist (confirm only when there are unsaved edits —
    // tracks are added from the Library now), Save Playlist (native save
    // dialog, defaults to the Library's current folder; later saves overwrite
    // the tracked file directly).
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if ui.button("Create Playlist").clicked() {
            if TPlayApp::confirm(
                "New playlist",
                "Discard unsaved changes and start a new playlist?",
                app.playlist_dirty(),
            ) {
                app.new_playlist();
            }
        }
        let save_hover = app
            .playlist_file()
            .map(|p| format!("Overwrite playlist: {}", p.display()))
            .unwrap_or_else(|| "Save the playlist to a .tplay file".into());
        if ui.button("Save Playlist").on_hover_text(save_hover).clicked() {
            if let Some(path) = app.playlist_file().map(PathBuf::from) {
                // Already saved once this session: overwrite, no dialog. A
                // tracked smb:// URI overwrites on the server.
                app.save_playlist_to(path);
            } else if let Some(dir) = browsing_share_dir(app) {
                // Browsing a share: the native dialog cannot target an
                // smb:// URI, so ask for a filename and write into the
                // directory on screen. No mount, no temp file, no dialog.
                open_share_save(ui, dir, library::default_playlist_name(app.playlist_file()));
            } else if let Some(path) = rfd::FileDialog::new()
                .add_filter("TPlay playlist", &["tplay"])
                .set_directory(app.library_dir())
                .set_file_name("playlist.tplay")
                .save_file()
            {
                app.save_playlist_to(path);
            }
        }
    });

    share_save_modal(app, ui);
}

/// The directory a share-save would write into: the share/dir currently open in
/// the Library's remote browser. `None` at the share-list stage (no directory
/// on screen) and while a listing is in flight, so the caller falls back to the
/// local save dialog rather than guessing a target.
fn browsing_share_dir(app: &TPlayApp) -> Option<String> {
    let b = app.network().browse()?;
    if b.busy {
        return None;
    }
    let share = b.share.as_ref()?;
    Some(network::dir_uri(&b.host, share, &b.rel))
}

/// Arm the share-save prompt. The directory is captured here, at press time, so
/// the render needs no browse state and a later navigation can't retarget it.
fn open_share_save(ui: &egui::Ui, dir: String, name: String) {
    ui.ctx()
        .memory_mut(|m| m.data.insert_temp(egui::Id::new(SAVE_SHARE_ID), Some((dir, name))));
}

/// The one non-native dialog in the app: a filename is needed, and
/// `rfd::MessageDialog` is a native Yes/No with no text field. State lives in
/// egui memory like every other per-frame UI value — `TPlayApp` stays
/// UI-state-free, and no new state machine is needed because
/// `save_playlist_to` is non-blocking (the write goes to the SMB worker).
fn share_save_modal(app: &mut TPlayApp, ui: &egui::Ui) {
    let id = egui::Id::new(SAVE_SHARE_ID);
    let state = ui
        .ctx()
        .memory_mut(|m| m.data.get_temp::<Option<(String, String)>>(id).unwrap_or(None));
    let Some((dir, mut name)) = state else { return };

    let theme = app.theme().clone();
    let p = theme.palette;
    // Some(None) = dismissed, Some(Some(name)) = confirmed.
    let mut action: Option<Option<String>> = None;
    let resp = egui::Modal::new(id).show(ui.ctx(), |ui| {
        ui.set_min_width(SAVE_SHARE_FIELD_W);
        ui.label(egui::RichText::new("Save playlist to share").strong().color(p.text_primary));
        // Full target, truncated to the modal but complete on hover.
        ui.add(
            egui::Label::new(egui::RichText::new(&dir).small().color(p.text_secondary)).truncate(),
        )
        .on_hover_text(&dir);
        ui.add_space(4.0);

        // Enter submits (TextEdit surrenders focus on Enter).
        let mut enter = false;
        let field = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(SAVE_SHARE_FIELD_W)
                .clip_text(true),
        );
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            enter = true;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                action = Some(Some(name.clone()));
            }
            if ui.button("Cancel").clicked() {
                action = Some(None);
            }
        });
        if enter {
            action = Some(Some(name.clone()));
        }
    });
    if resp.should_close() {
        action = Some(None);
    }

    match action {
        Some(Some(typed)) => {
            ui.ctx().memory_mut(|m| m.data.insert_temp(id, None::<(String, String)>));
            // `.tplay` is implied and separators are stripped, not sent to the
            // server as a bogus path. Blank input cancels.
            if let Some(file) = library::playlist_file_name(&typed) {
                let target = network::child_uri(&dir, &file);
                app.save_playlist_to(PathBuf::from(target));
            }
        }
        Some(None) => {
            ui.ctx().memory_mut(|m| m.data.insert_temp(id, None::<(String, String)>));
        }
        // Still open: keep whatever was typed so it survives the next frame.
        None => {
            ui.ctx().memory_mut(|m| m.data.insert_temp(id, Some((dir, name))));
        }
    }
}
