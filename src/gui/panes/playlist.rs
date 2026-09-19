use crate::app::TPlayApp;
use eframe::egui;

pub fn playlist_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if ui.button("Add Files").clicked() {
            if let Some(paths) = TPlayApp::audio_dialog().pick_files() {
                app.add_files(paths);
            }
        }

        let mut shuffle = app.shuffle();
        let shuffle_changed = ui.checkbox(&mut shuffle, "🔀 Shuffle").changed();
        if shuffle_changed {
            app.toggle_shuffle();
        }

        let mut repeat = app.repeat();
        ui.checkbox(&mut repeat, "🔁 Repeat");
        if repeat != app.repeat() {
            app.toggle_repeat();
        }

        if ui.button("Save Playlist").clicked() {
            app.save_playlist();
        }
        if ui.button("Load Playlist").clicked() {
            app.load_playlist();
        }
    });

    let drag_from_id = egui::Id::new("tplay.drag_from");
    let drag_hover_id = egui::Id::new("tplay.drag_hover");

    // Drag state persists across frames via egui memory
    let mut drag_from = ui.ctx().memory_mut(|m| m.data.get_temp::<Option<usize>>(drag_from_id).unwrap_or(None));
    let mut drag_hover = ui.ctx().memory_mut(|m| m.data.get_temp::<Option<usize>>(drag_hover_id).unwrap_or(None));

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut to_delete: Option<usize> = None;

            for i in 0..app.playlist().len() {
                let is_current = app.current_index() == Some(i);
                let name = app.playlist()[i]
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy().into_owned();
                let track_num = format!("{}.", i + 1);

                let row_response = ui.horizontal(|ui| {
                    ui.label(track_num);

                    let label_resp = ui.add(
                        egui::Label::new(egui::RichText::new(&name).color(
                            if is_current { ui.style().visuals.strong_text_color() } else { ui.style().visuals.text_color() }
                        ))
                        .selectable(is_current)
                        .sense(egui::Sense::click_and_drag()),
                    );

                    if label_resp.drag_started() {
                        drag_from = Some(i);
                        drag_hover = None;
                    }

                    if label_resp.dragged() {
                        ui.painter().text(
                            label_resp.rect.center(),
                            egui::Align2::CENTER_CENTER,
                            &name,
                            egui::FontId::proportional(14.0),
                            ui.style().visuals.hyperlink_color,
                        );
                    }

                    if drag_hover == Some(i) || is_current {
                        ui.painter().rect_filled(
                            label_resp.rect.expand(4.0),
                            4.0,
                            ui.style().visuals.hyperlink_color.gamma_multiply(0.15),
                        );
                    }

                    if label_resp.clicked() {
                        app.play_track(i);
                    }

                    if ui.button("✕").clicked() {
                        to_delete = Some(i);
                    }

                    label_resp
                }).inner;

                let row_rect = row_response.rect;
                if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    if drag_from.is_some() && drag_from != Some(i) {
                        if row_rect.contains(pointer_pos) {
                            drag_hover = Some(i);
                        }
                    }
                }

                if row_response.drag_stopped() {
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
                app.remove_track(idx);
            }
        });

    // Persist drag state for the next frame
    ui.ctx().memory_mut(|m| {
        m.data.insert_temp(drag_from_id, drag_from);
        m.data.insert_temp(drag_hover_id, drag_hover);
    });
}