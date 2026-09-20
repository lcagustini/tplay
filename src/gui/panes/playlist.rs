use crate::app::TPlayApp;
use crate::gui::theme::{self, Icon};
use crate::library;
use eframe::egui;
use std::path::PathBuf;

pub fn playlist_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    // Owned Arc copy — panes call &mut app while using theme data.
    let theme = app.theme().clone();
    let p = theme.palette;

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
                        .font(egui::FontId::new(12.0, theme.metadata_font.clone())),
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
                        egui::FontId::proportional(14.0),
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
                                .font(egui::FontId::new(12.0, theme.metadata_font.clone())),
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

    // Bottom actions: New Playlist (confirm only when there are unsaved edits —
    // tracks are added from the Library now), Save Playlist (native save
    // dialog, defaults to the Library's current folder; later saves overwrite
    // the tracked file directly).
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("New Playlist").clicked() {
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
            .map(|p| format!("Overwrite {}", p.display()))
            .unwrap_or_else(|| "Save to a .tplay file".into());
        if ui.button("Save Playlist").on_hover_text(save_hover).clicked() {
            if let Some(path) = app.playlist_file().map(PathBuf::from) {
                // Already saved once this session: overwrite, no dialog.
                app.save_playlist_to(path);
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
}