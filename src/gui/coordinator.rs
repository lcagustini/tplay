//! Coordinator — wires the panes together in one frame using egui_dock.

use crate::app::{TPlayApp, Pane};
use crate::gui::panes;
use crate::gui::theme;
use eframe::egui;
use egui_dock::{DockArea, DockState, Node, NodeIndex, Style, TabIndex, TabViewer};
use serde_json;
use std::path::PathBuf;

const DOCK_LAYOUT_FILE: &str = "dock_layout.json";
const DOCK_ID: &str = "tplay.dock_state";
/// egui memory key: measured natural body height of a pane's content.
const PANE_CONTENT_H: &str = "tplay.pane_content_h";
/// egui memory key: measured minimum body width of a pane's content.
const PANE_CONTENT_W: &str = "tplay.pane_content_w";

/// Enforce minimum pane sizes by rewriting split fractions. Returns true if any fraction changed.
fn apply_min_pane_sizes(
    tree: &mut DockState<Pane>,
    ctx: &egui::Context,
    border_v: f32,
    border_h: f32,
) -> bool {
    let mut changed = false;
    for &pane in &Pane::ALL {
        let h_id = egui::Id::new(PANE_CONTENT_H).with(pane);
        let w_id = egui::Id::new(PANE_CONTENT_W).with(pane);
        let min_h = ctx.data(|d| d.get_temp::<f32>(h_id)).unwrap_or(0.0);
        let min_w = ctx.data(|d| d.get_temp::<f32>(w_id)).unwrap_or(0.0);
        if min_h <= 0.0 && min_w <= 0.0 {
            continue;
        }
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
            if dim <= 0.0 { continue; }
            let leaf_min = if vertical { min_h + border_v } else { min_w + border_h };
            let min_frac = (leaf_min / dim).clamp(0.05, 0.95);
            let (c0, c1) = (parent.left(), parent.right());
            let old = *fraction;
            // NowPlaying is Fixed: pin to exact content height. Others are Fill: floor only.
            let is_fixed = pane == Pane::NowPlaying && vertical;
            if c0 == leaf {
                *fraction = if is_fixed { min_frac } else { fraction.max(min_frac) };
            } else if c1 == leaf {
                *fraction = if is_fixed { 1.0 - min_frac } else { fraction.min(1.0 - min_frac) };
            }
            changed |= *fraction != old;
        }
    }
    changed
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
        Pane::Library => "Library".into(),
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
            Pane::Library => panes::library::library_pane(self.app, ui),
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
    // splits, and their order all live here and persist via dock_layout.json.
    let mut tree = ctx.data_mut(|d| d.get_temp::<DockState<Pane>>(egui::Id::new(DOCK_ID)))
        .or_else(|| {
            layout_path().and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| serde_json::from_str(&s).ok())
        })
        .unwrap_or_else(default_tree);

    // Top-left menu button (app logo per theme): pane checkboxes reflect the
    // tree, toggles edit it; a Skin section (moved here from the Now Playing
    // title row) switches themes.
    egui::TopBottomPanel::top("pane_dropdown_panel")
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            // No native title bar (decorations off — see main.rs), so the whole
            // bar drags the window. Added first, at the bottom of the z-stack:
            // the menu logo and the window controls drawn after still win their
            // own clicks, and only the empty bar area falls through to this.
            let titlebar = ui.max_rect().shrink(0.5);
            let drag = ui.interact(
                titlebar,
                egui::Id::new("tplay.titlebar_drag"),
                egui::Sense::drag(),
            );
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

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
                ui.separator();
                ui.label("Library");
                let mut show_hidden = app.show_hidden();
                if ui.checkbox(&mut show_hidden, "Show hidden folders").changed() {
                    app.set_show_hidden(show_hidden);
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

                // No native title bar (decorations off — see main.rs), so the
                // window controls live here, right-aligned via a right-to-left
                // flush layout. Within it the platform convention holds: the
                // first added (Close) lands rightmost, so reading left-to-right
                // the order is minimize, maximize/restore, close. The glyphs
                // come from the bundled emoji icon font (Ubuntu-Light has no
                // box-drawing set).
                let maximized = ui
                    .ctx()
                    .input(|i| i.viewport().maximized)
                    .unwrap_or(false);
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let close = ui
                            .add(egui::Button::new("🗙").small().frame(false))
                            .on_hover_text("Close");
                        if close.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let maximize = ui
                            .add(egui::Button::new("🗖").small().frame(false))
                            .on_hover_text(if maximized { "Restore" } else { "Maximize" });
                        if maximize.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                        let minimize = ui
                            .add(egui::Button::new("🗕").small().frame(false))
                            .on_hover_text("Minimize");
                        if minimize.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    },
                );
            });
        });

    // Each leaf holds exactly one pane, so its tab (a draggable full-width
    // header strip) doubles as the pane's title bar. No tabbing ever occurs.
    let mut style = Style::from_egui(ctx.style().as_ref());
    style.tab_bar.fill_tab_bar = true;

    let border_v = style.tab_bar.height
        + style.tab.tab_body.inner_margin.top
        + style.tab.tab_body.inner_margin.bottom;
    let border_h = style.tab.tab_body.inner_margin.left
        + style.tab.tab_body.inner_margin.right;

    let mut viewer = PaneViewer { app };
    DockArea::new(&mut tree)
        .show_add_buttons(false)
        .show_add_popup(false)
        .show_close_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .style(style)
        .show(ctx, &mut viewer);

    // Enforce minimum pane sizes after layout (corrects splitter drag / floating window shrink).
    let mut corrected = apply_min_pane_sizes(&mut tree, ctx, border_v, border_h);
    // EQ pane minimum width: 10 bands at their minimum width.
    let eq_min_w = ctx.data(|d| d.get_temp::<f32>(egui::Id::new(PANE_CONTENT_W).with(Pane::Equalizer))).unwrap_or(0.0);
    if eq_min_w > 0.0 {
        let eq_surface = tree.iter_all_tabs().find(|(_, t)| **t == Pane::Equalizer).map(|((s, _), _)| s);
        if let Some(surface) = eq_surface {
            if let Some(ws) = tree.get_window_state_mut(surface) {
                let cur = ws.rect();
                let new_w = cur.width().max(eq_min_w + border_h);
                if cur.width() > 0.0 && new_w > cur.width() {
                    ws.set_size(egui::vec2(new_w, cur.height()));
                    corrected = true;
                }
            }
        }
    }
    if corrected { ctx.request_repaint(); }

    // Persist to egui memory (session) + disk (JSON, not RON)
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_ID), tree.clone()));
    if let Some(path) = layout_path() {
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(path, serde_json::to_string(&tree).unwrap_or_default());
    }
}