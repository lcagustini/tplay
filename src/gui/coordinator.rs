//! Coordinator — wires the panes together in one frame using egui_dock.

use crate::app::{TPlayApp, Pane};
use crate::gui::panes;
use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use std::path::PathBuf;

const DOCK_LAYOUT_FILE: &str = "dock_layout.ron";
const DOCK_ID: &str = "tplay.dock_state";

fn default_tree() -> DockState<Pane> {
    let mut d = DockState::new(vec![Pane::NowPlaying]);
    d.main_surface_mut().split_below(NodeIndex::root(), 0.25, vec![Pane::Playlist]);
    d
}

fn pane_title(pane: Pane) -> egui::WidgetText {
    match pane {
        Pane::NowPlaying => "Now Playing".into(),
        Pane::Playlist => "Playlist".into(),
    }
}

struct PaneViewer<'a> {
    app: &'a mut TPlayApp,
}

impl TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, pane: &mut Pane) -> egui::WidgetText {
        pane_title(*pane)
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &mut Pane) {
        match pane {
            Pane::NowPlaying => panes::now_playing::now_playing_pane(self.app, ui),
            Pane::Playlist => panes::playlist::playlist_pane(self.app, ui),
        }
    }

    fn closeable(&mut self, _tab: &mut Pane) -> bool {
        true
    }

    fn add_popup(&mut self, ui: &mut egui::Ui, _surface: egui_dock::SurfaceIndex, _node: egui_dock::NodeIndex) {
        ui.set_min_width(160.0);
        let mut to_add: Option<Pane> = None;
        for pane in Pane::ALL {
            if !self.app.dock_open.contains(&pane) {
                if ui.button(pane_title(pane).text()).clicked() {
                    to_add = Some(pane);
                }
            }
        }
        if let Some(pane) = to_add {
            self.app.dock_open.insert(pane);
            self.app.dock_state.main_surface_mut().push_to_first_leaf(pane);
            ui.close_menu();
        }
    }
}

fn layout_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(DOCK_LAYOUT_FILE))
}

/// Update the UI for one frame. Called from TPlayApp::update().
pub fn update_ui(app: &mut TPlayApp, ctx: &egui::Context) {
    // Load DockState from egui memory (per-session) or disk (first run)
    let mut tree = ctx.data_mut(|d| d.get_temp::<DockState<Pane>>(egui::Id::new(DOCK_ID)))
        .or_else(|| {
            layout_path().and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| ron::from_str(&s).ok())
        })
        .unwrap_or_else(default_tree);

    // Sync hidden panes with actual tree state
    app.dock_open = tree.iter_all_tabs().map(|(_, t)| *t).collect();

    let mut viewer = PaneViewer { app };
    DockArea::new(&mut tree)
        .show_add_buttons(true)
        .show_add_popup(true)
        .style(Style::from_egui(ctx.style().as_ref()))
        .show(ctx, &mut viewer);

    // Persist to egui memory (session) + disk
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_ID), tree.clone()));
    if let Some(path) = layout_path() {
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(path, ron::to_string(&tree).unwrap_or_default());
    }
}