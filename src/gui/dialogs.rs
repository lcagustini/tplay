//! The app's only dialogs: a Yes/No confirm and a save-name prompt. Both are
//! in-app `egui::Modal`s — there is no native OS file or message browser, so a
//! target is always a name typed here over a directory the app already knows
//! (the Library's current folder, a share directory on screen, the layouts dir).
//!
//! Both follow one shape: a call site hands over plain data with [`ask`] /
//! [`ask_save_name`] (egui memory, so `TPlayApp` stays UI-state-free and the state
//! survives across frames), and [`show`] draws the modal at the end of the frame
//! and carries the action out once the user answers. Actions are enums, not
//! closures — a closure cannot be stored in egui memory.

use crate::app::{Pane, TPlayApp};
use crate::gui::coordinator;
use crate::library;
use crate::network;
use eframe::egui;
use egui_dock::DockState;
use std::path::PathBuf;

/// egui memory: the armed Yes/No, `Option<Armed>`.
pub const CONFIRM_ID: &str = "tplay.confirm";
/// egui memory: the armed save-name prompt, `Option<(SaveTarget, String, String)>`.
pub const SAVE_NAME_ID: &str = "tplay.save_name";
/// Width of the save-name text field.
const FIELD_W: f32 = 220.0;
/// Minimum width of the confirm modal (its description wraps within this).
const CONFIRM_W: f32 = 260.0;

/// A state-losing action to confirm. Every arm is a plain value, so the whole
/// pending action round-trips through egui memory.
#[derive(Clone)]
pub enum ConfirmAction {
    RemoveTrack(usize),
    NewPlaylist,
    /// `remote` picks the playlist transport: a share row's path is an
    /// `smb://` URI, and the GUI layer never asks `is_remote` itself (see the
    /// one-surface rule in `tracks.rs`), so the list's caller says which it is.
    LoadPlaylist { path: PathBuf, remote: bool },
    DeleteLayout(PathBuf),
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

/// Draw whichever dialog is armed and run its action. Called once per frame
/// from the coordinator — after the dock area, so a modal always paints on top.
pub fn show(app: &mut TPlayApp, tree: &mut DockState<Pane>, ctx: &egui::Context) {
    confirm_modal(app, ctx);
    save_name_modal(app, tree, ctx);
}

fn confirm_modal(app: &mut TPlayApp, ctx: &egui::Context) {
    let Some(armed) = take_state::<Armed>(ctx, CONFIRM_ID) else {
        return;
    };
    let p = app.theme().palette;

    // None = still open, Some(true) = Yes, Some(false) = dismissed.
    let mut answer: Option<bool> = None;
    let resp = egui::Modal::new(egui::Id::new(CONFIRM_ID)).show(ctx, |ui| {
        ui.set_min_width(CONFIRM_W);
        ui.label(egui::RichText::new(&armed.title).strong().color(p.text_primary));
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
        ConfirmAction::LoadPlaylist { path, remote } => {
            if remote {
                app.fetch_remote_playlist(path.to_string_lossy().into_owned());
            } else {
                app.load_playlist_from(path);
            }
        }
        ConfirmAction::DeleteLayout(path) => {
            let _ = std::fs::remove_file(&path);
            // Clear tracking if this was the tracked layout.
            let tracked = ctx.data(|d| d.get_temp::<String>(egui::Id::new(coordinator::NAMED_LAYOUT_FILE)));
            if tracked.as_deref() == Some(path.to_string_lossy().as_ref()) {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new(coordinator::NAMED_LAYOUT_FILE), String::new()));
            }
        }
    }
}

/// The save-name prompt: a filename is needed, so a plain Yes/No is not enough.
/// No new app-side state machine either — `save_playlist_to` is non-blocking
/// (a share write goes to the SMB worker).
fn save_name_modal(app: &mut TPlayApp, tree: &mut DockState<Pane>, ctx: &egui::Context) {
    let Some((target, dir, mut name)) = take_state::<(SaveTarget, String, String)>(ctx, SAVE_NAME_ID) else {
        return;
    };

    let p = app.theme().palette;
    // Some(None) = dismissed, Some(Some(name)) = confirmed.
    let mut action: Option<Option<String>> = None;
    let resp = egui::Modal::new(egui::Id::new(SAVE_NAME_ID)).show(ctx, |ui| {
        ui.set_min_width(FIELD_W);
        ui.label(egui::RichText::new(target.title()).strong().color(p.text_primary));
        // Full target, truncated to the modal but complete on hover.
        ui.add(egui::Label::new(egui::RichText::new(&dir).small().color(p.text_secondary)).truncate())
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
            let Some(file) = layout_file_name(typed) else { return };
            let path = PathBuf::from(dir).join(file);
            coordinator::save_layout(tree, &path);
            ctx.data_mut(|d| d.insert_temp(
                egui::Id::new(coordinator::NAMED_LAYOUT_FILE),
                path.to_string_lossy().into_owned(),
            ));
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
    let state = ctx.memory_mut(|m| m.data.get_temp::<Option<T>>(id)).unwrap_or(None);
    ctx.memory_mut(|m| m.data.insert_temp(id, None::<T>));
    state
}

fn put<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: &str, value: T) {
    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new(key), value));
}
