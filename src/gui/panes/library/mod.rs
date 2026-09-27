//! Library pane — file browser + favorite folders + tag-scanning track list.
//!
//! Three parts, drawn by `library_pane` below:
//! - [`header`] — the breadcrumb row, one widget agnostic of local vs SMB
//! - [`sidebar`] — Places / Volumes / Network servers / Favorites
//! - [`listing`] — search, sortable column header, rows, composition counts,
//!   Add All, and the remote browser's half of the main column
//!
//! Left: favorite folders. Right: the current folder's subfolders and audio
//! files, each row showing title + artist/album + duration from the tag scan
//! (cached in `TPlayApp`, so revisits are instant). Click a row to play it
//! directly; the `+` button adds it to the playlist.

pub mod header;
pub mod listing;
pub mod sidebar;

use crate::app::TPlayApp;
use crate::gui::theme::ThemeState;
use eframe::egui;
use std::path::Path;

/// egui memory: has the pane listed the browsed dir at least once this session.
const LIB_INIT: &str = "tplay.library.init";
/// egui memory: the SMB add-server form state (the host field).
/// `None` = closed; `Some` = open with the field being edited.
const NET_FORM: &str = "tplay.network.form";

/// Leaf name of a dir (`/` at the filesystem root).
fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

pub fn library_pane(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui) {
    let theme = themes.current().clone();
    let browse = app.network().browse().cloned();
    let network_mode = browse.is_some();

    // Network mode swaps the main column for the remote browser, and with it the
    // header: the local breadcrumb + ★ applies only to the folder browser (the
    // sidebar below stays in both modes). Everything under the header — search,
    // sort header, rows, counts, Add All — is the shared `listing`, so both
    // browsers get it.
    if !network_mode {
        // First frame this session: list the saved/current dir, start the scan.
        if !ui.ctx().memory_mut(|m| {
            m.data
                .get_temp::<bool>(egui::Id::new(LIB_INIT))
                .unwrap_or(false)
        }) {
            ui.ctx()
                .memory_mut(|m| m.data.insert_temp(egui::Id::new(LIB_INIT), true));
            app.navigate_to(app.library().dir().to_path_buf());
        }
        header::local_header(app, themes, ui, &theme);
    }

    ui.add_space(4.0);

    // Add-server form state (just the host) lives in egui memory, carried across
    // frames. Credentials are prompted for in the main pane instead, so a saved
    // address is reusable without retyping. Read here and written back at the end
    // because the sidebar's "+" and the form's own Add/Cancel both move it, and
    // the value has to outlive the closure either way.
    let mut form = ui.ctx().memory_mut(|m| {
        m.data
            .get_temp::<Option<String>>(egui::Id::new(NET_FORM))
            .unwrap_or(None)
    });

    // Places + Favorites column, file browser column.
    ui.horizontal_top(|ui| {
        sidebar::sidebar_ui(app, themes, ui, &theme, &mut form, network_mode);
        ui.separator();
        ui.vertical(|ui| match &browse {
            // The remote breadcrumb is drawn inside the main column, below the
            // separator and level with the list, rather than above the sidebar
            // where the local one is. Both go through the same `breadcrumb`.
            Some(browse) => {
                header::remote_header(app, ui, &theme, browse);
                ui.add_space(4.0);
                listing::remote_list_ui(app, themes, ui, &theme, browse);
            }
            None => listing::local_list_ui(app, themes, ui, &theme),
        });
    });
    ui.ctx()
        .memory_mut(|m| m.data.insert_temp(egui::Id::new(NET_FORM), form));
}
