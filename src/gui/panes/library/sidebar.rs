//! The Library pane's left column: Places, Volumes, Network servers, Favorites.
//!
//! Fixed-width by construction — see the `allocate_space` note below. Every row
//! is a click target that jumps the browser, so the click is deferred to the end
//! of the scroll closure (one `jump` for the whole column) rather than acted on
//! inside it.

use super::dir_name;
use crate::app::TPlayApp;
use crate::gui::theme;
use crate::library;
use crate::network;
use eframe::egui;
use std::path::{Path, PathBuf};

/// Fixed width of the Places/Favorites column. Allocated as an exact rect (not
/// `set_min_width`) so no child can resize it — see the note in `sidebar_ui`.
const SIDEBAR_W: f32 = 120.0;
/// Width of one row's label, leaving room for the ✕ on removable rows.
const ROW_W: f32 = 100.0;
/// Width of a row's label in the Network section, where the ✕ sits inside the
/// same 120px and the host strings are longer than the friendly labels.
const SERVER_ROW_W: f32 = 88.0;
/// Height of one row.
const ROW_H: f32 = 18.0;
/// Width of the add-server form's text fields, inside the fixed-width sidebar.
const FORM_W: f32 = 100.0;
/// Height of one add-server form text field.
const FORM_FIELD_H: f32 = 18.0;

/// What a sidebar row's click asked for.
enum RowClick {
    /// Navigate to the row's path.
    Jump,
    /// Remove it (Favorites drops the bookmark; a server row drops the entry).
    Remove,
}

/// One sidebar row: a truncating label, accent-tinted when it is the current
/// folder, plus an optional ✕. Places, Volumes and Favorites are the same row
/// with different data, so they share this rather than three near-identical
/// copies. Returns the row's click, if any.
///
/// `app` is borrowed immutably (for the ✕ icon) because the *action* is the
/// caller's: ✕ means "unbookmark" on a Favorite and "forget server" on a server
/// row, so this only reports it.
fn sidebar_row(
    ui: &mut egui::Ui,
    app: &TPlayApp,
    theme: &theme::Theme,
    layout: &theme::Layout,
    label: &str,
    path: &Path,
    active: bool,
    removable: bool,
) -> Option<RowClick> {
    let p = theme.palette;
    let mut out = None;
    ui.horizontal(|ui| {
        let resp = ui
            .add_sized(
                egui::vec2(ROW_W, ROW_H),
                egui::Label::new(
                    egui::RichText::new(label)
                        .color(if active { p.accent } else { p.text_secondary })
                        .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
                )
                .truncate()
                .sense(egui::Sense::click()),
            )
            .on_hover_text_at_pointer(path.display().to_string());
        if resp.clicked() {
            out = Some(RowClick::Jump);
        }
        if removable
            && theme::icon_button(ui, app.theme_icon(theme::Icon::Remove), theme::Icon::Remove, 13.0, true, false)
                .clicked()
        {
            out = Some(RowClick::Remove);
        }
    });
    out
}

/// A section heading: "Places" / "Volumes" / "Network" / "Favorites".
fn section_label(ui: &mut egui::Ui, p: theme::Palette, text: &str) {
    ui.label(egui::RichText::new(text).small().strong().color(p.text_secondary));
    ui.add_space(4.0);
}

pub fn sidebar_ui(
    app: &mut TPlayApp,
    ui: &mut egui::Ui,
    theme: &theme::Theme,
    form: &mut Option<String>,
    network_mode: bool,
) {
    let p = theme.palette;
    let layout = theme.layout.with_defaults();

    // The Places/Favorites column is a FIXED 120px sidebar. Two egui facts force
    // this exact shape:
    //
    // 1. Not `ui.vertical` + `set_min_width`: a `ui.vertical` child is sized by
    //    its own `min_rect`, and `TextEdit` deliberately grows that by the text
    //    overflow ("allocate additional space … so a ScrollArea can properly
    //    scroll to the cursor"). This ScrollArea is vertical-only, so its width
    //    *is* the content width and the overflow propagated up — typing a long
    //    address widened the whole sidebar. `set_max_width`/`clip_text` cannot
    //    stop it: caps bound painting, but the overflow grows `min_rect`, and
    //    `min_rect` wins the layout.
    // 2. `ui.new_child` alone does NOT advance this horizontal cursor (only
    //    `allocate_new_ui` does), so the file-list sibling landed at the same x
    //    and drew over the sidebar. `allocate_space` reserves the rect *and*
    //    moves the cursor — both halves needed.
    let (_, sidebar_rect) = ui.allocate_space(egui::vec2(SIDEBAR_W, ui.available_height()));
    let mut sidebar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(sidebar_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    sidebar.vertical(|ui| {
        let scroll_h = (ui.available_height() - 8.0).max(40.0);
        // id_salt: without it this shares the default "scroll_area" persistent id
        // with the file list's ScrollArea (sibling column uis resolve to the same
        // ui.id) → egui's ID-clash debug overlay.
        egui::ScrollArea::vertical()
            .id_salt("places_favorites")
            // auto_shrink x=true: a vertical scroll area must not claim the whole
            // row width (false expands it to fill), which would starve the
            // file-list column beside it.
            .auto_shrink([true, false])
            .max_height(scroll_h)
            .show(ui, |ui| {
                let mut jump: Option<PathBuf> = None;

                // The local row to highlight — and `None` while a share is open.
                // Entering network mode does NOT change `library_dir`, so
                // comparing against it directly kept the last local folder lit
                // *alongside* the server row. Exactly one row is active at a
                // time: the server when browsing, else the local folder. (The
                // other direction already worked — clicking a local folder calls
                // `leave_network`, so the server row un-highlights.)
                let current_local = (!network_mode).then(|| app.library_dir().to_path_buf());

                // Places: Home + XDG shortcuts — the favorites row style, no ✕.
                let places = app.quick_folders();
                if !places.is_empty() {
                    section_label(ui, p, "Places");
                    for (label, path) in places {
                        let active = current_local.as_deref() == Some(path.as_path());
                        if matches!(
                            sidebar_row(ui, app, theme, &layout, &label, &path, active, false),
                            Some(RowClick::Jump)
                        ) {
                            jump = Some(path);
                        }
                    }
                    ui.add_space(8.0);
                }

                // Volumes: local block partitions from /proc/self/mounts.
                let volumes = library::Volume::mounted_volumes();
                if !volumes.is_empty() {
                    section_label(ui, p, "Volumes");
                    for vol in volumes {
                        let active = current_local.as_deref() == Some(vol.path.as_path());
                        if matches!(
                            sidebar_row(ui, app, theme, &layout, &vol.label, &vol.path, active, false),
                            Some(RowClick::Jump)
                        ) {
                            jump = Some(vol.path);
                        }
                    }
                    ui.add_space(8.0);
                }

                // Network: built-in SMB browsing (no mount required).
                {
                    let servers = app.network().servers().to_vec();
                    let active_host = app.network().browse().map(|b| b.host.clone());
                    // Same LTR row style as the section labels above: a
                    // right_to_left header would fill the scroll area's
                    // (unbounded) content width and push the + past the 120px
                    // sidebar.
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Network").small().strong().color(p.text_secondary));
                        if ui.add(egui::Button::new("+").small()).on_hover_text("Add server").clicked() {
                            // is_none + assign: `Option::or` would move `form` out of this scope.
                            if form.is_none() {
                                *form = Some(String::new());
                            }
                        }
                    });
                    if let Some(host) = form.as_mut() {
                        // Enter submits (singleline TextEdit surrenders focus
                        // on Enter — the standard pattern).
                        let mut enter = false;
                        let mut submit: Option<bool> = None; // Some(true) = Add, Some(false) = Cancel
                        // The form lives in a FIXED-RECT child so egui's TextEdit
                        // overflow allocation ("allocate additional space … so a
                        // ScrollArea can properly scroll to the cursor") cannot
                        // widen the scroll content — that growth is what kept
                        // dragging the sidebar's scrollbar while typing.
                        //
                        // Both halves are load-bearing and *different*:
                        // `allocate_space` reserves the rect AND advances the
                        // layout cursor (a bare `new_child` would leave the next
                        // row drawn on top of the form), while the raw
                        // `new_child` does NOT propagate its min_rect to the
                        // scroll content, which is what contains the overflow.
                        // `clip_text` alone is not enough — it pins the field rect
                        // but the overflow allocation still grows the parent.
                        let gap = ui.spacing().item_spacing.y;
                        let form_h = FORM_FIELD_H + gap + ui.spacing().interact_size.y;
                        let (_, form_rect) = ui.allocate_space(egui::vec2(FORM_W, form_h));
                        let mut form_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(form_rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        form_ui.vertical(|ui| {
                            let field = ui
                                .add_sized(
                                    egui::vec2(FORM_W, FORM_FIELD_H),
                                    egui::TextEdit::singleline(host)
                                        .hint_text("host or smb://host/share")
                                        .clip_text(true),
                                )
                                .on_hover_text("like 192.168.1.50 — ask for the login in the main pane");
                            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                enter = true;
                            }
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new("Add").small()).clicked() {
                                    submit = Some(true);
                                }
                                if ui.add(egui::Button::new("Cancel").small()).clicked() {
                                    submit = Some(false);
                                }
                            });
                        });
                        if submit.is_none() && enter {
                            submit = Some(true);
                        }
                        match submit {
                            Some(true) => {
                                // The field takes a bare host OR a full
                                // `smb://host/share[/dir]` URI. Parsing keeps the
                                // full URI out of the saved-server list and drops
                                // us straight into the share when one is named —
                                // the GNOME-Files path, which never enumerates
                                // shares first. Only the host is stored;
                                // credentials are asked for in the main pane, so a
                                // saved server is reusable without retyping.
                                if let Some((h, share, rel)) = network::parse_server_input(host) {
                                    app.add_network_server(h.clone(), String::new());
                                    *form = None;
                                    if let Some(share) = share.filter(|s| !s.is_empty()) {
                                        app.network_mut().browse_open(
                                            network::dir_uri(&h, &share, &rel),
                                            Some(share),
                                            rel,
                                        );
                                    }
                                }
                            }
                            Some(false) => *form = None,
                            None => {}
                        }
                        ui.add_space(4.0);
                    }
                    for s in servers {
                        ui.horizontal(|ui| {
                            let active = active_host.as_ref() == Some(&s.host);
                            if ui
                                .add_sized(
                                    egui::vec2(SERVER_ROW_W, ROW_H),
                                    egui::Label::new(
                                        egui::RichText::new(&s.host)
                                            .color(if active { p.accent } else { p.text_secondary })
                                            .font(egui::FontId::new(
                                                layout.text_meta,
                                                theme.metadata_font.clone(),
                                            )),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text_at_pointer("Browse shares")
                                .clicked()
                            {
                                app.network_mut().browse_server(s.host.clone());
                            }
                            if theme::icon_button(ui, app.theme_icon(theme::Icon::Remove), theme::Icon::Remove, 13.0, true, false)
                                .clicked()
                            {
                                app.remove_network_server(&s.host);
                            }
                        });
                    }
                    ui.add_space(8.0);
                }

                section_label(ui, p, "Favorites");
                for dir in app.favorite_dirs().to_vec() {
                    let name = dir_name(&dir);
                    let active = current_local.as_deref() == Some(dir.as_path());
                    match sidebar_row(ui, app, theme, &layout, &name, &dir, active, true) {
                        Some(RowClick::Jump) => jump = Some(dir),
                        Some(RowClick::Remove) => app.toggle_favorite(dir),
                        None => {}
                    }
                }
                if let Some(dir) = jump {
                    // Entering a local folder exits network browse mode.
                    app.network_mut().leave_network();
                    app.navigate_to(dir);
                }
            });
    });
}
