//! Coordinator — wires the panes together in one frame using egui_dock.

use crate::app::{TPlayApp, Pane};
use crate::gui::panes;
use crate::gui::theme;
use eframe::egui;
use egui_dock::{DockArea, DockState, Node, NodeIndex, Style, TabIndex, TabViewer};
use std::path::PathBuf;

const DOCK_LAYOUT_FILE: &str = "dock_layout.ron";
const DOCK_ID: &str = "tplay.dock_state";
/// egui memory key: measured natural body height of a pane's content.
const PANE_CONTENT_H: &str = "tplay.pane_content_h";

/// How a pane is sized inside the dock.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneSizing {
    /// Fills its dock share; the separator can drag it, but never below the
    /// pane's recorded minimum content height (none recorded = scrollable
    /// content, so it can shrink freely).
    Fill,
    /// Pinned to the pane's natural content height — the split fraction is
    /// recomputed from the pane's measured content every frame, so the
    /// separator snaps back and the pane can't grow an empty dead zone.
    Fixed,
}

/// Per-pane sizing policy.
fn pane_sizing(pane: Pane) -> PaneSizing {
    match pane {
        Pane::NowPlaying => PaneSizing::Fixed,
        Pane::Playlist | Pane::Equalizer => PaneSizing::Fill,
    }
}

/// Enforce the per-pane size policy by rewriting split fractions before the
/// dock lays out (egui_dock has no per-pane size limits of its own):
fn apply_pane_sizes(tree: &mut DockState<Pane>, ctx: &egui::Context, style: &Style) {
    for &pane in &Pane::ALL {
        let id = egui::Id::new(PANE_CONTENT_H).with(pane);
        let Some(content_h) = ctx.data(|d| d.get_temp::<f32>(id)) else { continue };
        if content_h <= 0.0 {
            continue;
        }
        // A leaf is the tab bar plus the body frame (its inner margin sits
        // outside the pane content we measured).
        let leaf_h = content_h
            + style.tab_bar.height
            + style.tab.tab_body.inner_margin.top
            + style.tab.tab_body.inner_margin.bottom;

        let main = tree.main_surface_mut();
        let Some(leaf) = main.find_tab(&pane).map(|(node, _)| node) else { continue };

        for (i, node) in main.iter_mut().enumerate() {
            let parent = NodeIndex(i);
            let (vertical, fraction, rect) = match node {
                Node::Vertical { fraction, rect, .. } => (true, fraction, rect),
                Node::Horizontal { fraction, rect, .. } => (false, fraction, rect),
                _ => continue,
            };
            let dim = if vertical { rect.height() } else { rect.width() };
            if dim <= 0.0 {
                continue; // not laid out yet — skip until the tree has rects
            }
            // `fraction` is always the top/left child's share.
            let (c0, c1) = (parent.left(), parent.right());
            match (pane_sizing(pane), c0 == leaf, c1 == leaf) {
                // Pinned: the leaf takes exactly its content height.
                (PaneSizing::Fixed, true, _) => {
                    *fraction = (leaf_h / dim).clamp(0.05, 0.95);
                }
                (PaneSizing::Fixed, _, true) => {
                    *fraction = ((dim - leaf_h) / dim).clamp(0.05, 0.95);
                }
                // Floor only: never smaller than its content, may grow.
                (PaneSizing::Fill, true, _) => {
                    *fraction = fraction.max((leaf_h / dim).clamp(0.05, 0.95));
                }
                (PaneSizing::Fill, _, true) => {
                    *fraction = fraction.min(1.0 - (leaf_h / dim).clamp(0.05, 0.95));
                }
                _ => {}
            }
        }
    }
}

fn default_tree() -> DockState<Pane> {
    let mut d = DockState::new(vec![Pane::NowPlaying]);
    // 0.15 ≈ Now Playing's content height on a default-size window; the
    // Fixed-pane override corrects it to the exact height after frame one.
    d.main_surface_mut().split_below(NodeIndex::root(), 0.15, vec![Pane::Playlist]);
    d
}

fn pane_title(pane: Pane) -> egui::WidgetText {
    match pane {
        Pane::NowPlaying => "Now Playing".into(),
        Pane::Playlist => "Playlist".into(),
        Pane::Equalizer => "Equalizer".into(),
    }
}

/// Whether a pane is currently in the dock tree (any surface).
fn pane_is_open(tree: &DockState<Pane>, pane: Pane) -> bool {
    tree.iter_all_tabs().any(|(_, t)| *t == pane)
}

/// Add a pane by splitting the bottom-most leaf of the main surface.
fn add_pane(tree: &mut DockState<Pane>, pane: Pane) {
    let main_surface = tree.main_surface();
    let bottom_leaf = main_surface
        .iter()
        .enumerate()
        .rev()
        .find_map(|(idx, node)| node.is_leaf().then_some(NodeIndex(idx)));
    if let Some(leaf) = bottom_leaf {
        tree.main_surface_mut().split_below(leaf, 0.5, vec![pane]);
    }
}

/// Remove a pane from the dock tree (wherever it lives — main or floating).
fn remove_pane(tree: &mut DockState<Pane>, pane: Pane) {
    let target = tree.iter_all_tabs().find(|(_, t)| **t == pane).map(|((s, n), _)| (s, n));
    if let Some((surface, node)) = target {
        tree.remove_tab((surface, node, TabIndex(0)));
    }
}

struct PaneViewer<'a> {
    app: &'a mut TPlayApp,
}

impl TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, pane: &mut Pane) -> egui::WidgetText {
        let accent = self.app.theme().palette.accent;
        pane_title(*pane).color(accent)
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &mut Pane) {
        match pane {
            Pane::NowPlaying => panes::now_playing::now_playing_pane(self.app, ui),
            Pane::Playlist => panes::playlist::playlist_pane(self.app, ui),
            Pane::Equalizer => panes::equalizer::equalizer_pane(self.app, ui),
        }
    }

    fn closeable(&mut self, _tab: &mut Pane) -> bool {
        false // Hide via dropdown only
    }

    // The EQ pane sizes its bands to fit the pane width; hide the tab-level
    // scrollbars (egui_dock wraps tab bodies in a ScrollArea by default).
    fn scroll_bars(&self, tab: &Pane) -> [bool; 2] {
        match tab {
            Pane::Equalizer => [false, false],
            _ => [true, true],
        }
    }
}

fn layout_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(DOCK_LAYOUT_FILE))
}

/// Update the UI for one frame. Called from TPlayApp::update().
pub fn update_ui(app: &mut TPlayApp, ctx: &egui::Context) {
    // Theme tokens -> egui visuals, every frame so a mid-session switch lands instantly.
    theme::apply(ctx, app.theme());
    // egui selects label text on drag by default; the playlist reorders by
    // dragging track titles, so kill text selection app-wide.
    ctx.style_mut(|s| s.interaction.selectable_labels = false);

    // Load DockState from egui memory (per-session) or disk (first run).
    // The tree is the single source of truth: which panes are open, their
    // splits, and their order all live here and persist via dock_layout.ron.
    let mut tree = ctx.data_mut(|d| d.get_temp::<DockState<Pane>>(egui::Id::new(DOCK_ID)))
        .or_else(|| {
            layout_path().and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| ron::from_str(&s).ok())
        })
        .unwrap_or_else(default_tree);

    // Top-left menu button (app logo per theme): pane checkboxes reflect the
    // tree, toggles edit it; a Skin section (moved here from the Now Playing
    // title row) switches themes.
    egui::TopBottomPanel::top("pane_dropdown_panel")
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            let logo = app.theme_icon(theme::Icon::Logo).cloned();
            let menu_contents = |ui: &mut egui::Ui| {
                let open_count = Pane::ALL.iter().filter(|p| pane_is_open(&tree, **p)).count();
                for pane in Pane::ALL {
                    let mut open = pane_is_open(&tree, pane);
                    let last_one = open && open_count == 1;
                    if ui.add_enabled(!last_one, egui::Checkbox::new(&mut open, pane_title(pane).text()))
                        .changed()
                    {
                        if open {
                            add_pane(&mut tree, pane);
                        } else {
                            remove_pane(&mut tree, pane);
                        }
                    }
                }
                ui.separator();
                ui.label("Skin");
                let mut sel = app.theme().id.clone();
                for t in app.themes() {
                    if ui.selectable_label(t.id == sel, t.name.clone()).clicked() {
                        sel = t.id.clone();
                    }
                }
                if sel != app.theme().id {
                    app.set_theme(&sel);
                    ui.close_menu();
                }
            };
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                match logo {
                    Some(tex) => {
                        ui.menu_image_button(
                            egui::Image::new(&tex).fit_to_exact_size(egui::vec2(18.0, 18.0)),
                            menu_contents,
                        );
                    }
                    None => {
                        ui.menu_button("☰", menu_contents);
                    }
                }
            });
        });

    // Each leaf holds exactly one pane, so its tab (a draggable full-width
    // header strip) doubles as the pane's title bar. No tabbing ever occurs.
    let mut style = Style::from_egui(ctx.style().as_ref());
    style.tab_bar.fill_tab_bar = true;

    // Pin fixed panes to their content height and floor fill panes (the EQ)
    // to their minimum before the dock lays out (see apply_pane_sizes).
    apply_pane_sizes(&mut tree, ctx, &style);

    let mut viewer = PaneViewer { app };
    DockArea::new(&mut tree)
        .show_add_buttons(false)
        .show_add_popup(false)
        .show_close_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .style(style)
        .show(ctx, &mut viewer);

    // Persist to egui memory (session) + disk
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_ID), tree.clone()));
    if let Some(path) = layout_path() {
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(path, ron::to_string(&tree).unwrap_or_default());
    }
}