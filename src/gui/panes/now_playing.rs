use crate::app::TPlayApp;
use crate::gui::theme::{self, Icon, Theme};
use crate::library;
use eframe::egui;

/// egui memory Id for the seek slider position
fn seek_id() -> egui::Id {
    egui::Id::new("tplay.seek")
}

/// Label in the theme's metadata color + font (monospace in retro).
fn meta(s: impl Into<String>, theme: &Theme, size: f32) -> egui::RichText {
    egui::RichText::new(s.into())
        .color(theme.palette.text_secondary)
        .font(egui::FontId::new(size, theme.metadata_font.clone()))
}

pub fn now_playing_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    // Owned Arc copy — panes take `&mut app` while `meta` needs `&Theme`.
    let theme = app.theme().clone();
    let layout = theme.layout.with_defaults();

    // Fill pane (dock-sized, resizable — the Fixed pin is gone), so pad the top
    // to center the controls. The pad comes from last frame's measured content
    // height (the value the coordinator floors the split at), so it converges
    // one frame after a resize.
    let content_h_id = egui::Id::new("tplay.pane_content_h").with(crate::app::Pane::NowPlaying);
    let avail_h = ui.available_height();
    let last_h = ui.ctx().data(|d| d.get_temp::<f32>(content_h_id)).unwrap_or(avail_h);
    ui.add_space(((avail_h - last_h) / 2.0).max(0.0));

    let body = ui.scope(|ui| {
        ui.vertical(|ui| {
            // Track info: title, then artist · album, on their own lines above
            // the seeker. Tagged, with the filename standing in until the scan
            // lands (`start_track` reads the playing track up front, so this is
            // filled the moment a track starts).
            let cur = app.current_path().map(|p| p.to_path_buf());
            let info = cur.as_deref().and_then(|p| app.track_info(p));
            let title = cur.as_deref().map(|p| library::title_or_stem(p, info)).unwrap_or_default();
            if !title.is_empty() {
                ui.label(meta(title, &theme, layout.text_time));
                let artist = info.map(|i| i.artist.as_str()).unwrap_or_default();
                let album = info.map(|i| i.album.as_str()).unwrap_or_default();
                let sub = match (artist.is_empty(), album.is_empty()) {
                    (true, true) => None,
                    (true, false) => Some(album.to_owned()),
                    (false, true) => Some(artist.to_owned()),
                    (false, false) => Some(format!("{artist} · {album}")),
                };
                if let Some(sub) = sub {
                    ui.label(meta(sub, &theme, layout.text_meta));
                }
            } else if app.network().pending().is_some() {
                // Remote track spooling from the server; current_path stays
                // None until the download lands.
                let name = app
                    .network()
                    .pending()
                    .map(|p| library::title_or_stem(p, None))
                    .unwrap_or_default();
                ui.label(meta(name.to_string(), &theme, layout.text_time));
                ui.label(meta("Loading from server…", &theme, layout.text_meta));
            }

        // Progress row: elapsed | full-width seek bar | total.
        ui.horizontal(|ui| {
            let total_secs = app.total_duration().map(|d| d.as_secs_f32());
            let actual_ratio = app.playback_position();

            let pos_secs = app.playback_position_secs();
            let pos_str = if app.remaining() {
                if let Some(total) = app.total_duration() {
                    let rem = total.saturating_sub(pos_secs);
                    TPlayApp::fmt_duration(Some(rem))
                } else {
                    "--:--".to_string()
                }
            } else {
                TPlayApp::fmt_duration(Some(pos_secs))
            };
            let total_str = TPlayApp::fmt_duration(app.total_duration());

            let mut seek_normalized = ui.ctx().memory_mut(|m| m.data.get_temp::<f32>(seek_id()).unwrap_or(0.0));

            // Measure both labels so the bar between them takes exactly the
            // leftover width. (available_width() read from a nested
            // right-to-left scope under-sizes inside the dock's ScrollArea.)
            let font = egui::FontId::new(layout.text_time, theme.metadata_font.clone());
            let label_w = |s: &str| {
                ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), theme.palette.text_secondary).size().x)
            };
            let gaps = ui.spacing().item_spacing.x * 2.0;
            let bar_w = (ui.available_width() - label_w(&pos_str) - label_w(&total_str) - gaps).max(40.0);

            // Elapsed/remaining label — click to toggle mode
            let pos_label = ui.label(meta(pos_str, &theme, layout.text_time));
            if pos_label.clicked() {
                app.set_remaining(!app.remaining());
            }
            pos_label.on_hover_text(if app.remaining() { "Click to show elapsed" } else { "Click to show remaining" });

            let bar = ui.add_enabled_ui(total_secs.is_some(), |ui| {
                // A Slider ignores add_sized and requests
                // spacing().slider_width itself, so set that to span the
                // leftover width.
                ui.spacing_mut().slider_width = bar_w;
                ui.add(
                    egui::Slider::new(&mut seek_normalized, 0.0..=1.0)
                        .show_value(false)
                        .trailing_fill(true),
                )
            }).inner;

            ui.label(meta(total_str, &theme, layout.text_time));

            if bar.dragged() {
                // hold
            } else if bar.drag_stopped() || bar.clicked() {
                app.seek(seek_normalized);
            } else if let Some(target) = app.seek_target() {
                // A seek holds the bar at the target until the sink's position
                // catches up. (seek_target stays set after a seek by design —
                // catch-up is monotonic, so past this point we track playback
                // forever.)
                if total_secs.is_some() && app.playback_position() >= target - 0.02 {
                    seek_normalized = actual_ratio;
                }
            } else {
                // Resting: track the real position — also what resets the bar to
                // 0:00 on a track change, since loading clears seek_target and
                // the new track starts at 0.
                seek_normalized = actual_ratio;
            }

            ui.ctx().memory_mut(|m| m.data.insert_temp(seek_id(), seek_normalized));
        });

        ui.add_space(8.0);

        // Controls row: transport + volume.
        ui.horizontal(|ui| {
            // Prev track
            let prev_enabled = app.has_prev_track();
            if theme::icon_button(ui, app.theme_icon(Icon::Prev), Icon::Prev, 18.0, prev_enabled, false).clicked() {
                app.prev_track();
            }

            // Play/Pause/Stop
            let is_paused = app.is_paused();
            let is_empty = app.is_empty();
            if is_paused || is_empty {
                // Nothing loaded and an empty playlist: greyed out, since
                // `play()` would have nothing to start.
                if theme::icon_button(ui, app.theme_icon(Icon::Play), Icon::Play, 18.0, app.can_play(), false).clicked() {
                    app.play();
                }
            } else if theme::icon_button(ui, app.theme_icon(Icon::Pause), Icon::Pause, 18.0, true, false).clicked() {
                app.pause();
            }

            if theme::icon_button(ui, app.theme_icon(Icon::Stop), Icon::Stop, 18.0, true, false).clicked() {
                app.stop();
            }

            // Next track
            let next_enabled = app.has_next_track();
            if theme::icon_button(ui, app.theme_icon(Icon::Next), Icon::Next, 18.0, next_enabled, false).clicked() {
                app.next_track();
            }

            // Shuffle / repeat — lit while active.
            ui.separator();
            if theme::icon_button(ui, app.theme_icon(Icon::Shuffle), Icon::Shuffle, 18.0, true, app.shuffle()).clicked() {
                app.toggle_shuffle();
            }
            if theme::icon_button(ui, app.theme_icon(Icon::Repeat), Icon::Repeat, 18.0, true, app.repeat()).clicked() {
                app.toggle_repeat();
            }

            ui.separator();

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::icon(ui, app.theme_icon(Icon::Volume), Icon::Volume, 15.0);
                let mut volume = app.volume();
                if ui.add(
                    egui::Slider::new(&mut volume, 0.0..=1.0)
                        .show_value(false)
                        .trailing_fill(true),
                ).changed() {
                    app.set_volume(volume);
                }
            });
        });
        });

        // Second row: Balance (L/R) + Gapless/Crossfade toggles
        ui.horizontal(|ui| {
            // Balance slider (left); double-click resets to center
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(meta("Balance", &theme, layout.text_meta));
                let mut balance = app.balance();
                let bal_response = ui.add(
                    egui::Slider::new(&mut balance, -1.0..=1.0)
                        .show_value(false)
                        .trailing_fill(true),
                );
                if bal_response.changed() {
                    app.set_balance(balance);
                }
                if bal_response.double_clicked() {
                    app.set_balance(0.0);
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Crossfade toggle (icon button, lit while active)
                if theme::icon_button(
                    ui,
                    app.theme_icon(Icon::Crossfade),
                    Icon::Crossfade,
                    18.0,
                    true,
                    app.crossfade(),
                ).clicked() {
                    app.toggle_crossfade();
                }
                // Gapless toggle (icon button, lit while active)
                if theme::icon_button(
                    ui,
                    app.theme_icon(Icon::Gapless),
                    Icon::Gapless,
                    18.0,
                    true,
                    app.gapless(),
                ).clicked() {
                    app.toggle_gapless();
                }
            });
        });
    });

    // Record the natural content height (excluding the centering pad): the
    // coordinator floors this pane's split at it, and we read it next frame to
    // compute that pad.
    ui.ctx().data_mut(|d| {
        d.insert_temp(content_h_id, body.response.rect.height());
    });
}