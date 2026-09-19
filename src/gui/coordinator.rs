//! Coordinator — wires the panes together in one frame using egui_dock.

use crate::app::{TPlayApp, Pane};
use crate::gui::panes;
use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use std::collections::HashSet;
use std::path::PathBuf;

const DOCK_LAYOUT_FILE: &str = "dock_layout.ron";
const DOCK_ID: &str = "tplay.dock_state";
const DOCK_OPEN_ID: &str = "tplay.dock_open";
const DOCK_PENDING_ADD_ID: &str = "tplay.dock_pending_add";

fn default_tree() -> DockState<Pane> {
    let mut d = DockState::new(vec![Pane::NowPlaying]);
    d.main_surface_mut().split_below(NodeIndex::root(), 0.25, vec![Pane::Playlist]);
    d
}

fn pane_title(pane: Pane) -> egui::WidgetText {
    match pane {
        Pane::NowPlaying => "Now Playing".into(),
        Pane::Playlist => "Playlist".into(),
        Pane::Equalizer => "Equalizer".into(),
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
            Pane::Equalizer => panes::equalizer::equalizer_pane(self.app, ui),
        }
    }

    fn closeable(&mut self, _tab: &mut Pane) -> bool {
        true
    }

    // The EQ pane sizes its bands to fit the pane width; hide the tab-level
    // scrollbars (egui_dock wraps tab bodies in a ScrollArea by default).
    fn scroll_bars(&self, tab: &Pane) -> [bool; 2] {
        match tab {
            Pane::Equalizer => [false, false],
            _ => [true, true],
        }
    }

    fn add_popup(&mut self, ui: &mut egui::Ui, _surface: egui_dock::SurfaceIndex, _node: egui_dock::NodeIndex) {
        ui.set_min_width(160.0);
        let ctx = ui.ctx().clone();
        let dock_open = ctx.data_mut(|d| {
            d.get_temp::<HashSet<Pane>>(egui::Id::new(DOCK_OPEN_ID))
                .unwrap_or_else(|| Pane::ALL.into_iter().collect())
        });
        let mut to_add: Option<Pane> = None;
        for pane in Pane::ALL {
            if !dock_open.contains(&pane) {
                if ui.button(pane_title(pane).text()).clicked() {
                    to_add = Some(pane);
                }
            }
        }
        if let Some(pane) = to_add {
            ctx.data_mut(|d| {
                let mut set = d.get_temp::<HashSet<Pane>>(egui::Id::new(DOCK_OPEN_ID))
                    .unwrap_or_else(|| Pane::ALL.into_iter().collect());
                set.insert(pane);
                d.insert_temp(egui::Id::new(DOCK_OPEN_ID), set);
            });
            ctx.data_mut(|d| {
                d.insert_temp(egui::Id::new(DOCK_PENDING_ADD_ID), pane);
            });
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

    // Load/set dock_open from egui memory
    let dock_open = ctx.data_mut(|d| d.get_temp::<HashSet<Pane>>(egui::Id::new(DOCK_OPEN_ID)))
        .unwrap_or_else(|| Pane::ALL.into_iter().collect());
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_OPEN_ID), dock_open.clone()));

    let mut viewer = PaneViewer { app };
    DockArea::new(&mut tree)
        .show_add_buttons(true)
        .show_add_popup(true)
        .style(Style::from_egui(ctx.style().as_ref()))
        .show(ctx, &mut viewer);

    // Apply pending pane addition (from + popup) to the tree
    if let Some(pane) = ctx.data_mut(|d| d.get_temp::<Pane>(egui::Id::new(DOCK_PENDING_ADD_ID))) {
        ctx.data_mut(|d| d.remove::<Pane>(egui::Id::new(DOCK_PENDING_ADD_ID)));
        let mut dock_open = ctx.data_mut(|d| d.get_temp::<HashSet<Pane>>(egui::Id::new(DOCK_OPEN_ID)))
            .unwrap_or_else(|| Pane::ALL.into_iter().collect());
        dock_open.insert(pane);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_OPEN_ID), dock_open));
        tree.main_surface_mut().push_to_first_leaf(pane);
    }

    // Sync dock_open with actual tree state for next frame
    let actual_open: HashSet<Pane> = tree.iter_all_tabs().map(|(_, t)| *t).collect();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_OPEN_ID), actual_open));

    // Persist to egui memory (session) + disk
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_ID), tree.clone()));
    if let Some(path) = layout_path() {
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(path, ron::to_string(&tree).unwrap_or_default());
    }
}