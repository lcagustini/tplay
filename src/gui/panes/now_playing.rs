use crate::app::TPlayApp;
use crate::gui::theme::{self, Icon, Theme};
use eframe::egui;

/// egui Id for storing the seek slider position in memory
fn seek_id() -> egui::Id {
    egui::Id::new("tplay.seek")
}

/// Small label with the theme's metadata color + font (monospace in retro).
fn meta(s: impl Into<String>, theme: &Theme, size: f32) -> egui::RichText {
    egui::RichText::new(s.into())
        .color(theme.palette.text_secondary)
        .font(egui::FontId::new(size, theme.metadata_font.clone()))
}

pub fn now_playing_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    // Owned Arc copy (refcount bump) — lets panes call &mut app while keeping
    // the theme data; `meta` takes &Theme via deref.
    let theme = app.theme().clone();

    let body = ui.scope(|ui| {
        ui.vertical(|ui| {
        // Progress row: elapsed | full-width seek bar | total.
        ui.horizontal(|ui| {
            let total_secs = app.total_duration().map(|d| d.as_secs_f32());
            let actual_ratio = app.playback_position();

            let pos_str = TPlayApp::fmt_duration(app.playback_position_secs());
            let total_str = app.total_duration()
                .map(TPlayApp::fmt_duration)
                .unwrap_or_else(|| "--:--".into());

            let mut seek_normalized = ui.ctx().memory_mut(|m| m.data.get_temp::<f32>(seek_id()).unwrap_or(0.0));

            // Measure both time labels so the bar between them takes exactly
            // the leftover width. (available_width() read from nested
            // right-to-left scopes under-sizes inside the dock's ScrollArea.)
            let font = egui::FontId::new(13.0, theme.metadata_font.clone());
            let label_w = |s: &str| {
                ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), theme.palette.text_secondary).size().x)
            };
            let gaps = ui.spacing().item_spacing.x * 2.0;
            let bar_w = (ui.available_width() - label_w(&pos_str) - label_w(&total_str) - gaps).max(40.0);

            ui.label(meta(pos_str, &theme, 13.0));

            let bar = ui.add_enabled_ui(total_secs.is_some(), |ui| {
                // A Slider ignores add_sized — it requests spacing().slider_width
                // itself — so set that to span the leftover width.
                ui.spacing_mut().slider_width = bar_w;
                ui.add(
                    egui::Slider::new(&mut seek_normalized, 0.0..=1.0)
                        .show_value(false)
                        .trailing_fill(true),
                )
            }).inner;

            ui.label(meta(total_str, &theme, 13.0));

            if bar.dragged() {
                // hold
            } else if bar.drag_stopped() || bar.clicked() {
                app.seek(seek_normalized);
            } else {
                let target_reached = app.seek_target_reached().unwrap_or(true);
                if target_reached {
                    app.clear_seek_target();
                    if total_secs.is_some() {
                        seek_normalized = actual_ratio;
                    }
                }
            }

            ui.ctx().memory_mut(|m| m.data.insert_temp(seek_id(), seek_normalized));
        });

        ui.add_space(8.0);

        // Controls row: transport + volume, with track info sitting between
        // them (mirrors the reference status-bar right group).
        ui.horizontal(|ui| {
            // Prev track
            let prev_enabled = app.has_prev_track();
            if theme::icon_button(ui, app.theme_icon(Icon::Prev), Icon::Prev, 18.0, prev_enabled).clicked() {
                app.prev_track();
            }

            // Play/Pause/Stop
            let is_paused = app.is_paused();
            let is_empty = app.is_empty();
            if is_paused || is_empty {
                if theme::icon_button(ui, app.theme_icon(Icon::Play), Icon::Play, 18.0, true).clicked() {
                    app.play();
                }
            } else if theme::icon_button(ui, app.theme_icon(Icon::Pause), Icon::Pause, 18.0, true).clicked() {
                app.pause();
            }

            if theme::icon_button(ui, app.theme_icon(Icon::Stop), Icon::Stop, 18.0, true).clicked() {
                app.stop();
            }

            // Next track
            let next_enabled = app.has_next_track();
            if theme::icon_button(ui, app.theme_icon(Icon::Next), Icon::Next, 18.0, next_enabled).clicked() {
                app.next_track();
            }

            ui.separator();

            // Track info: name · FORMAT
            let fmt = app
                .current_path()
                .and_then(|p| p.extension())
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_uppercase())
                .unwrap_or_default();
            let stem = app
                .current_path()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            ui.label(meta(if fmt.is_empty() { stem } else { format!("{stem} · {fmt}") }, &theme, 12.0));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Volume
                theme::icon(ui, app.theme_icon(Icon::Volume), Icon::Volume, 15.0);
                let mut volume = app.volume();
                if ui.add(egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false)).changed() {
                    app.set_volume(volume);
                }
            });
        });
        });
    });

    // Record the natural content height so the coordinator can pin this pane
    // to exactly its content (no empty dead space below the controls).
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("tplay.pane_content_h").with(crate::app::Pane::NowPlaying),
            body.response.rect.height(),
        );
    });
}