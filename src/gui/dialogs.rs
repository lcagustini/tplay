//! The app's only dialogs: a Yes/No confirm, a save-name prompt, and a track
//! editor (a 5-star row plus the four text tags). All are in-app `egui::Modal`s — there is no native OS file or
//! message browser, so a target is always a name typed here over a directory the
//! app already knows (the Library's current folder, a share directory on screen,
//! the layouts dir).
//!
//! They follow one shape: a call site hands over plain data with [`ask`] /
//! [`ask_save_name`] / [`ask_edit`] (egui memory, so `TPlayApp` stays
//! UI-state-free and the state survives across frames), and [`show`] draws
//! whichever is armed at the end of the frame and carries the action out once the
//! user answers. Actions are enums, never closures — a closure cannot be stored in
//! egui memory.

use crate::app::{Pane, TPlayApp};
use crate::gui::coordinator;
use crate::gui::theme::{self, ThemeState};
use crate::library;
use crate::library_db::SmartView;
use crate::network;
use crate::tracks;
use eframe::egui;
use egui_dock::DockState;
use std::path::PathBuf;

/// egui memory: the armed Yes/No, `Option<Armed>`.
pub const CONFIRM_ID: &str = "tplay.confirm";
/// egui memory: the armed save-name prompt, `Option<(SaveTarget, String, String)>`.
pub const SAVE_NAME_ID: &str = "tplay.save_name";
/// egui memory: the armed track editor, `Option<EditArmed>`.
pub const EDIT_ID: &str = "tplay.edit";
/// The save-name modal's width, and the field's desired width within it.
const FIELD_W: f32 = 220.0;
/// Minimum width of the confirm modal (its description wraps within this).
const CONFIRM_W: f32 = 260.0;
/// The editor modal's width, and a text field's desired width within it.
const EDIT_W: f32 = 320.0;
/// Width of a field's label, so the four inputs line up.
const FIELD_LABEL_W: f32 = 56.0;
/// The star size in the editor. Bigger than the row's inline star, because this is
/// the one place the rating is *chosen* rather than scanned.
const PICKER_STAR: f32 = 22.0;

/// A state-losing action to confirm. Every arm is a plain value, so the whole
/// pending action round-trips through egui memory.
#[derive(Clone)]
pub enum ConfirmAction {
    RemoveTrack(usize),
    NewPlaylist,
    /// `remote` picks the playlist transport: a share row's path is an
    /// `smb://` URI, and the GUI layer never asks `is_remote` itself (see the
    /// one-surface rule in `tracks.rs`), so the list's caller says which it is.
    LoadPlaylist {
        path: PathBuf,
        remote: bool,
    },
    DeleteLayout(PathBuf),
    /// Replace the playlist with a Smart View's selection. Carries the view by
    /// value because a rule is plain data — an armed one must survive the modal
    /// waiting for an answer, and a view can be deleted meanwhile.
    LoadSmartView(SmartView),
}

/// An armed Yes/No, waiting for an answer.
#[derive(Clone)]
struct Armed {
    action: ConfirmAction,
    title: String,
    desc: String,
}

/// Arm the confirm modal. A caller with nothing at risk (a clean playlist, say)
/// skips this and acts directly — the modal exists for the destructive case.
pub fn ask(ctx: &egui::Context, action: ConfirmAction, title: &str, desc: &str) {
    put(
        ctx,
        CONFIRM_ID,
        Some(Armed {
            action,
            title: title.to_string(),
            desc: desc.to_string(),
        }),
    );
}

/// What a confirmed save-name prompt writes, and how `dir` is joined to it.
#[derive(Clone, Copy, PartialEq)]
pub enum SaveTarget {
    /// A share directory: `dir` is the `smb://…` dir URI the Library is browsing.
    PlaylistShare,
    /// A local directory: `dir` is the Library's current folder.
    PlaylistLocal,
    /// The named-layouts directory.
    Layout,
}

impl SaveTarget {
    fn title(self) -> &'static str {
        match self {
            SaveTarget::Layout => "Save layout",
            _ => "Save playlist",
        }
    }
}

/// Arm the save-name prompt. `dir` is captured here, at press time, so the
/// render needs no browse state and a later navigation can't retarget it.
pub fn ask_save_name(ctx: &egui::Context, target: SaveTarget, dir: String, name: String) {
    put(ctx, SAVE_NAME_ID, Some((target, dir, name)));
}

/// An armed editor. The tags it holds are the *draft* as well as the file's
/// current values, because a typed field has to survive the modal re-arming every
/// frame — the same trick the save-name prompt uses for its one field.
#[derive(Clone)]
struct EditArmed {
    track: PathBuf,
    rating: u8,
    title: String,
    artist: String,
    album: String,
    track_no: String,
    error: Option<String>,
}

/// What the user did in the editor.
enum EditAction {
    /// Clicked a star: rating `n`, and nothing else.
    Stars(u8),
    /// Pressed Save: the four text fields.
    Save,
}

/// Arm the track editor. The tags are captured at press time, so the modal needs
/// no lookup and the draft it opens on is what the file actually says.
pub fn ask_edit(ctx: &egui::Context, track: PathBuf, info: Option<&library::TrackInfo>) {
    let text = |f: fn(&library::TrackInfo) -> &str| info.map(f).unwrap_or_default().to_owned();
    put(
        ctx,
        EDIT_ID,
        Some(EditArmed {
            track,
            rating: info.map_or(0, |i| i.rating),
            title: text(|i| &i.title),
            artist: text(|i| &i.artist),
            album: text(|i| &i.album),
            track_no: info.and_then(|i| i.track_no.clone()).unwrap_or_default(),
            error: None,
        }),
    );
}

/// Draw whichever dialog is armed and run its action. Called once per frame
/// from the coordinator — after the dock area, so a modal always paints on top.
pub fn show(
    app: &mut TPlayApp,
    themes: &ThemeState,
    tree: &mut DockState<Pane>,
    ctx: &egui::Context,
) {
    confirm_modal(app, themes, ctx);
    save_name_modal(app, themes, tree, ctx);
    edit_modal(app, themes, ctx);
}

fn confirm_modal(app: &mut TPlayApp, themes: &ThemeState, ctx: &egui::Context) {
    let Some(armed) = take_state::<Armed>(ctx, CONFIRM_ID) else {
        return;
    };
    let p = themes.current().palette;

    // None = still open, Some(true) = Yes, Some(false) = dismissed.
    let mut answer: Option<bool> = None;
    let resp = egui::Modal::new(egui::Id::new(CONFIRM_ID)).show(ctx, |ui| {
        ui.set_min_width(CONFIRM_W);
        ui.label(
            egui::RichText::new(&armed.title)
                .strong()
                .color(p.text_primary),
        );
        // Wrapped: descriptions carry track and layout names, which can be long.
        ui.add(egui::Label::new(egui::RichText::new(&armed.desc).color(p.text_secondary)).wrap());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Yes").clicked() {
                answer = Some(true);
            }
            if ui.button("No").clicked() {
                answer = Some(false);
            }
        });
    });
    // Backdrop click counts as a dismissal.
    if resp.should_close() {
        answer = Some(false);
    }
    match answer {
        Some(true) => run(app, ctx, armed.action),
        Some(false) => {}
        // Still open: re-arm, so the next frame redraws it.
        None => put(ctx, CONFIRM_ID, Some(armed)),
    }
}

/// Carry out a confirmed action.
fn run(app: &mut TPlayApp, ctx: &egui::Context, action: ConfirmAction) {
    match action {
        ConfirmAction::RemoveTrack(i) => {
            // The armed index outlives the row that made it: the list can change
            // while the modal waits (an in-flight remote playlist can land).
            if i < app.playlist().len() {
                app.remove_track(i);
            }
        }
        ConfirmAction::NewPlaylist => app.new_playlist(),
        ConfirmAction::LoadSmartView(view) => app.load_smart_view(&view),
        ConfirmAction::LoadPlaylist { path, remote } => {
            if remote {
                app.network_mut().fetch(path.to_string_lossy().into_owned());
            } else {
                app.load_playlist_from(path);
            }
        }
        ConfirmAction::DeleteLayout(path) => {
            let _ = std::fs::remove_file(&path);
            // Clear tracking if this was the tracked layout.
            let tracked =
                ctx.data(|d| d.get_temp::<String>(egui::Id::new(coordinator::NAMED_LAYOUT_FILE)));
            if tracked.as_deref() == Some(path.to_string_lossy().as_ref()) {
                ctx.data_mut(|d| {
                    d.insert_temp(egui::Id::new(coordinator::NAMED_LAYOUT_FILE), String::new())
                });
            }
        }
    }
}

/// The save-name prompt: a filename is needed, so a plain Yes/No is not enough.
/// No new app-side state machine either — `save_playlist_to` is non-blocking
/// (a share write goes to the SMB worker).
fn save_name_modal(
    app: &mut TPlayApp,
    themes: &ThemeState,
    tree: &mut DockState<Pane>,
    ctx: &egui::Context,
) {
    let Some((target, dir, mut name)) =
        take_state::<(SaveTarget, String, String)>(ctx, SAVE_NAME_ID)
    else {
        return;
    };

    let p = themes.current().palette;
    // Some(None) = dismissed, Some(Some(name)) = confirmed.
    let mut action: Option<Option<String>> = None;
    let resp = egui::Modal::new(egui::Id::new(SAVE_NAME_ID)).show(ctx, |ui| {
        ui.set_min_width(FIELD_W);
        ui.label(
            egui::RichText::new(target.title())
                .strong()
                .color(p.text_primary),
        );
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
                .desired_width(FIELD_W)
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
        Some(Some(typed)) => write(app, tree, ctx, target, &dir, &typed),
        Some(None) => {}
        // Still open: keep whatever was typed so it survives the next frame.
        None => put(ctx, SAVE_NAME_ID, Some((target, dir, name))),
    }
}

/// The track editor: a 5-star row plus the four text tags, in one modal.
///
/// **One dialog, not two.** A separate text editor would have been a fourth shape
/// with its own arm, its own memory key and its own error path, for a form over the
/// same track the stars already edit. The stars write *immediately* — clicking one
/// is a complete, reversible action, and "the same star again clears" only works if
/// the click lands at once — while the text fields write on Save.
///
/// A third shape rather than a Yes/No because a write can genuinely fail: a
/// read-only mount, a share we refuse to touch. A rating that silently did not
/// change is the failure worth designing against, so the error rides in the armed
/// state and the modal stays open carrying it. There is no Cancel button: a
/// backdrop click dismisses, exactly as it does the confirm.
fn edit_modal(app: &mut TPlayApp, themes: &ThemeState, ctx: &egui::Context) {
    let Some(mut armed) = take_state::<EditArmed>(ctx, EDIT_ID) else {
        return;
    };
    let p = themes.current().palette;
    let star_on = themes.icon(theme::Icon::StarOn).cloned();
    let star_off = themes.icon(theme::Icon::StarOff).cloned();

    let mut stars: Option<u8> = None;
    let mut save = false;
    let resp = egui::Modal::new(egui::Id::new(EDIT_ID)).show(ctx, |ui| {
        ui.set_min_width(EDIT_W);
        ui.label(
            egui::RichText::new("Edit track")
                .strong()
                .color(p.text_primary),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    armed
                        .track
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .small()
                .color(p.text_secondary),
            )
            .truncate(),
        )
        .on_hover_text(armed.track.to_string_lossy());
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            for n in 1..=5u8 {
                let on = n <= armed.rating;
                let tex = if on {
                    star_on.as_ref()
                } else {
                    star_off.as_ref()
                };
                if theme::icon_toggle(ui, tex, theme::Icon::StarOn, PICKER_STAR, on).clicked() {
                    // The same star again clears, which is what makes that
                    // reachable without a separate control.
                    stars = Some(if n == armed.rating { 0 } else { n });
                }
            }
            ui.label(
                egui::RichText::new(if armed.rating == 0 {
                    "Unrated"
                } else {
                    "Rating"
                })
                .small()
                .color(p.text_secondary),
            );
        });
        ui.add_space(4.0);

        for (label, field) in [
            ("Title", &mut armed.title),
            ("Artist", &mut armed.artist),
            ("Album", &mut armed.album),
            ("Track no", &mut armed.track_no),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(FIELD_LABEL_W, ui.spacing().interact_size.y),
                    egui::Label::new(egui::RichText::new(label).small().color(p.text_secondary))
                        .truncate(),
                );
                ui.add(
                    egui::TextEdit::singleline(field)
                        .desired_width(FIELD_W)
                        .clip_text(true),
                );
            });
        }
        ui.add_space(4.0);
        if ui.button("Save").clicked() {
            save = true;
        }
        if let Some(e) = &armed.error {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(e)
                    .small()
                    .color(p.text_secondary)
                    .weak(),
            );
        }
    });

    // A backdrop click is a dismissal, but only when it was not also a click that
    // set an action — the same shape the confirm modal uses.
    if resp.should_close() && stars.is_none() && !save {
        return;
    }
    let action = stars
        .map(EditAction::Stars)
        .or_else(|| save.then_some(EditAction::Save));
    let Some(action) = action else {
        // Still open: re-arm with the typed text, so the draft survives the frame.
        put(ctx, EDIT_ID, Some(armed));
        return;
    };

    let edit = match &action {
        EditAction::Stars(n) => tracks::Edit {
            rating: Some(*n),
            ..Default::default()
        },
        EditAction::Save => tracks::Edit {
            title: Some(&armed.title),
            artist: Some(&armed.artist),
            album: Some(&armed.album),
            track_no: Some(&armed.track_no),
            // The stars already wrote themselves the moment they were clicked;
            // re-sending the rating would be a second write of the same value.
            rating: None,
        },
    };
    let rating_now = stars.unwrap_or(armed.rating);
    match app.apply_edit(&armed.track, edit) {
        Ok(()) => {
            if save {
                return; // done and closed
            }
            armed.rating = rating_now;
        }
        Err(e) => {
            eprintln!("tplay: could not edit {}: {e}", armed.track.display());
            armed.error = Some(e);
        }
    }
    put(ctx, EDIT_ID, Some(armed));
}

/// Write the confirmed filename into the directory captured when it was armed.
/// The modal's state is already cleared by `take_state` in every arm, so a
/// blank or unnameable entry is simply a cancel.
fn write(
    app: &mut TPlayApp,
    tree: &mut DockState<Pane>,
    ctx: &egui::Context,
    target: SaveTarget,
    dir: &str,
    typed: &str,
) {
    match target {
        // `.tplay` is implied and separators are stripped, not sent to the server
        // as a bogus path.
        SaveTarget::PlaylistShare => {
            if let Some(file) = library::playlist_file_name(typed) {
                app.save_playlist_to(PathBuf::from(network::child_uri(dir, &file)));
            }
        }
        SaveTarget::PlaylistLocal => {
            if let Some(file) = library::playlist_file_name(typed) {
                app.save_playlist_to(PathBuf::from(dir).join(file));
            }
        }
        SaveTarget::Layout => {
            let Some(file) = layout_file_name(typed) else {
                return;
            };
            let path = PathBuf::from(dir).join(file);
            coordinator::save_layout(tree, &path);
            ctx.data_mut(|d| {
                d.insert_temp(
                    egui::Id::new(coordinator::NAMED_LAYOUT_FILE),
                    path.to_string_lossy().into_owned(),
                )
            });
        }
    }
}

/// `.json` counterpart of `library::playlist_file_name`: separators stripped
/// rather than walked, extension implied, blank input cancels.
fn layout_file_name(typed: &str) -> Option<String> {
    let cleaned = typed.trim().replace(['/', '\\'], "_");
    if cleaned.is_empty() {
        return None;
    }
    Some(if cleaned.to_ascii_lowercase().ends_with(".json") {
        cleaned
    } else {
        format!("{cleaned}.json")
    })
}

/// Read and clear a modal's state in one step: whatever the modal does this
/// frame, the next one starts unarmed unless the modal puts the state back.
fn take_state<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: &str) -> Option<T> {
    let id = egui::Id::new(key);
    let state = ctx
        .memory_mut(|m| m.data.get_temp::<Option<T>>(id))
        .unwrap_or(None);
    ctx.memory_mut(|m| m.data.insert_temp(id, None::<T>));
    state
}

fn put<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: &str, value: T) {
    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new(key), value));
}
