//! Coordinator — wires the panes together in one frame using egui_dock.

use crate::app::{Pane, TPlayApp};
use crate::config::MAX_CROSSFADE_SECS;
use crate::gui::dialogs;
use crate::gui::panes;
use crate::gui::theme;
use crate::gui::theme::ThemeState;
use eframe::egui;
use egui_dock::{DockArea, DockState, Node, NodeIndex, Style, TabIndex, TabViewer};
use serde_json;
use std::path::PathBuf;

const DOCK_LAYOUT_FILE: &str = "dock_layout.json";
const DOCK_ID: &str = "tplay.dock_state";
/// egui memory key: serialized layout JSON currently on disk (seeded at startup).
const DOCK_SAVED_JSON: &str = "tplay.dock_layout_saved";
/// egui memory key: tracked named layout file (for save-over, like playlist_file).
pub(crate) const NAMED_LAYOUT_FILE: &str = "tplay.named_layout_file";
/// Directory name under config for named layout files.
const LAYOUTS_DIR: &str = "layouts";
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
        let Some(leaf) = main.find_tab(&pane).map(|(node, _)| node) else {
            continue;
        };
        for (i, node) in main.iter_mut().enumerate() {
            let parent = NodeIndex(i);
            let (vertical, fraction, rect) = match node {
                Node::Vertical { fraction, rect, .. } => (true, fraction, rect),
                Node::Horizontal { fraction, rect, .. } => (false, fraction, rect),
                _ => continue,
            };
            let dim = if vertical {
                rect.height()
            } else {
                rect.width()
            };
            if dim <= 0.0 {
                continue;
            }
            let leaf_min = if vertical {
                min_h + border_v
            } else {
                min_w + border_h
            };
            let min_frac = (leaf_min / dim).clamp(0.05, 0.95);
            let (c0, c1) = (parent.left(), parent.right());
            let old = *fraction;
            // All panes are Fill: floor at the minimum size, never pin to it.
            if c0 == leaf {
                *fraction = fraction.max(min_frac);
            } else if c1 == leaf {
                *fraction = fraction.min(1.0 - min_frac);
            }
            changed |= *fraction != old;
        }
    }
    changed
}

fn default_tree() -> DockState<Pane> {
    let mut d = DockState::new(vec![Pane::NowPlaying]);
    // 0.15 ≈ Now Playing's content height on a default-size window. All panes are
    // Fill, so this is just the starting split — resizable from there.
    d.main_surface_mut()
        .split_below(NodeIndex::root(), 0.15, vec![Pane::Playlist]);
    d
}

fn pane_title(pane: Pane) -> egui::WidgetText {
    match pane {
        Pane::NowPlaying => "Now Playing".into(),
        Pane::Playlist => "Playlist".into(),
        Pane::Equalizer => "Equalizer".into(),
        Pane::Library => "Library".into(),
        Pane::Visualizer => "Visualizer".into(),
        Pane::AlbumCover => "Album Cover".into(),
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
    let target = tree
        .iter_all_tabs()
        .find(|(_, t)| **t == pane)
        .map(|((s, n), _)| (s, n));
    if let Some((surface, node)) = target {
        tree.remove_tab((surface, node, TabIndex(0)));
    }
}

struct PaneViewer<'a> {
    app: &'a mut TPlayApp,
    themes: &'a ThemeState,
}

impl TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, pane: &mut Pane) -> egui::WidgetText {
        let accent = self.themes.current().palette.accent;
        pane_title(*pane).color(accent)
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &mut Pane) {
        // Reborrow rather than move: `self` is `&mut PaneViewer`, so the
        // destructuring below would otherwise move the `&mut` out of it.
        let (app, themes) = (&mut *self.app, self.themes);
        match pane {
            Pane::NowPlaying => panes::now_playing::now_playing_pane(app, themes, ui),
            Pane::Playlist => panes::playlist::playlist_pane(app, themes, ui),
            Pane::Equalizer => panes::equalizer::equalizer_pane(app, themes, ui),
            Pane::Library => panes::library::library_pane(app, themes, ui),
            Pane::Visualizer => panes::visualizer::visualizer_pane(app, themes, ui),
            Pane::AlbumCover => panes::album_cover::album_cover_pane(app, themes, ui),
        }
    }

    fn closeable(&mut self, _tab: &mut Pane) -> bool {
        false // Hide via dropdown only
    }

    // The EQ pane sizes its bands to fit the pane width; hide the tab-level
    // scrollbars (egui_dock wraps tab bodies in a ScrollArea by default).
    // Visualizer and Album Cover do the same (full-rect painting, no scroll).
    fn scroll_bars(&self, tab: &Pane) -> [bool; 2] {
        match tab {
            Pane::Equalizer | Pane::Visualizer | Pane::AlbumCover => [false, false],
            _ => [true, true],
        }
    }
}

fn layout_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tplay").join(DOCK_LAYOUT_FILE))
}

pub(crate) fn layouts_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| {
        let dir = d.join("tplay").join(LAYOUTS_DIR);
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
}

/// Save a DockState to a JSON file at the given path (creates parent dirs).
pub(crate) fn save_layout(tree: &DockState<Pane>, path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serde_json::to_string(tree).unwrap_or_default());
}

/// Load a DockState from a JSON file. Returns None if the file is missing,
/// unreadable, or contains invalid JSON.
fn load_layout(path: &std::path::Path) -> Option<DockState<Pane>> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// List all *.json files in the layouts dir, sorted by file name.
fn list_layouts(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

/// Draw one frame. Called from the `TPlay` shell in `main.rs`, which owns
/// `themes` — the app deliberately does not, so a theme switch re-decodes
/// icons with a `Context` the state layer never holds.
pub fn update_ui(app: &mut TPlayApp, themes: &mut ThemeState, ctx: &egui::Context) {
    // Theme tokens -> egui visuals, every frame so a mid-session switch lands
    // instantly.
    theme::apply(ctx, themes.current());
    // egui selects label text on drag by default; the playlist reorders by
    // dragging track titles, so kill text selection app-wide.
    ctx.style_mut(|s| s.interaction.selectable_labels = false);

    // Load DockState from egui memory (per-session) or disk (first run). The
    // tree is the single source of truth: which panes are open, their splits and
    // their order live here and persist via dock_layout.json.
    let mut tree = ctx
        .data_mut(|d| d.get_temp::<DockState<Pane>>(egui::Id::new(DOCK_ID)))
        .or_else(|| {
            layout_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| {
                    // Seed the on-disk JSON string so we only write when it changes.
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_SAVED_JSON), s.clone()));
                    serde_json::from_str(&s).ok()
                })
        })
        .unwrap_or_else(default_tree);

    // Top-left menu button (app logo per theme): pane checkboxes reflect the
    // tree and toggles edit it, and the Theme section switches themes.
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

            let logo = themes.icon(theme::Icon::Logo).cloned();
            // Window-chrome textures are cloned before the closures below so they
            // don't have to capture `app` (menu_contents already does).
            let win_close_tex = themes.icon(theme::Icon::Remove).cloned();
            let win_max_tex = themes.icon(theme::Icon::Maximize).cloned();
            let win_min_tex = themes.icon(theme::Icon::Minimize).cloned();

            // Precompute layouts dir and tracked layout once per frame for the menu.
            let layouts_dir = layouts_dir();
            let layout_files = layouts_dir
                .as_ref()
                .map(|d| list_layouts(d.as_ref()))
                .unwrap_or_default();
            let tracked_layout =
                ctx.data(|d| d.get_temp::<String>(egui::Id::new(NAMED_LAYOUT_FILE)));

            let menu_contents = |ui: &mut egui::Ui| {
                ui.label("Theme");
                let mut sel = themes.current().id.clone();
                for t in themes.list() {
                    if ui.selectable_label(t.id == sel, t.name.clone()).clicked() {
                        sel = t.id.clone();
                    }
                }
                if sel != themes.current().id {
                    // `set` owns the icon re-decode a switch needs, and the
                    // config compare persists the new id — nothing to flag here.
                    themes.set(ctx, &sel);
                    ui.close_menu();
                }
                ui.separator();
                ui.label("Library");
                let mut show_hidden = app.library().show_hidden();
                if ui
                    .checkbox(&mut show_hidden, "Show hidden folders")
                    .changed()
                {
                    app.set_show_hidden(show_hidden);
                }
                ui.separator();
                ui.label("Crossfade");
                // Crossfade duration always visible and editable (constant menu height).
                let mut cf_secs = app.prefs().crossfade_secs();
                if ui
                    .add(
                        egui::Slider::new(&mut cf_secs, 0.0..=MAX_CROSSFADE_SECS)
                            .suffix("s")
                            .trailing_fill(true),
                    )
                    .changed()
                {
                    app.prefs_mut().set_crossfade_secs(cf_secs);
                }
                ui.separator();
                ui.label("Layouts");
                let open_count = Pane::ALL
                    .iter()
                    .filter(|p| pane_is_open(&tree, **p))
                    .count();
                for pane in Pane::ALL {
                    let mut open = pane_is_open(&tree, pane);
                    let last_one = open && open_count == 1;
                    if ui
                        .add_enabled(
                            !last_one,
                            egui::Checkbox::new(&mut open, pane_title(pane).text()),
                        )
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
                for path in &layout_files {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("layout");
                    ui.horizontal(|ui| {
                        if ui.selectable_label(false, stem).clicked() {
                            if let Some(loaded) = load_layout(path) {
                                tree = loaded;
                                ctx.data_mut(|d| {
                                    d.insert_temp(
                                        egui::Id::new(NAMED_LAYOUT_FILE),
                                        path.to_string_lossy().into_owned(),
                                    )
                                });
                                ui.close_menu();
                            }
                        }
                        // Delete button (✕) — uses the same remove icon as window close
                        if theme::icon_button(
                            ui,
                            win_close_tex.as_ref(),
                            theme::Icon::Remove,
                            13.0,
                            true,
                            false,
                        )
                        .on_hover_text(format!("Delete saved layout \"{stem}\""))
                        .clicked()
                        {
                            dialogs::ask(
                                ctx,
                                dialogs::ConfirmAction::DeleteLayout(path.clone()),
                                "Delete layout",
                                &format!("Remove saved layout \"{stem}\"?"),
                            );
                            // The modal asks before anything happens, so the menu
                            // can close now — it would otherwise sit open behind it.
                            ui.close_menu();
                        }
                    });
                }
                // Save current layout… — a name over the layouts dir, in-app.
                // (Loading one is the list above: every saved layout is listed.)
                let save_hover = tracked_layout
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|p| format!("Overwrite layout: {}", p))
                    .unwrap_or_else(|| "Save the current dock layout".into());
                if ui
                    .button("Save current layout…")
                    .on_hover_text(save_hover)
                    .clicked()
                {
                    if let Some(dir) = &layouts_dir {
                        dialogs::ask_save_name(
                            ctx,
                            dialogs::SaveTarget::Layout,
                            dir.to_string_lossy().into_owned(),
                            "layout.json".into(),
                        );
                    }
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

                // No native title bar (decorations off — see main.rs), so the window
                // controls live here, right-aligned via a right-to-left flush
                // layout. Within it the platform convention holds: the first added
                // (Close) lands rightmost, so left-to-right the order is minimize,
                // maximize/restore, close. Icons are per-theme files
                // (text_primary chrome, ✕ close art).
                let maximized = ui.ctx().input(|i| i.viewport().maximized).unwrap_or(false);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = theme::icon_button(
                        ui,
                        win_close_tex.as_ref(),
                        theme::Icon::Remove,
                        13.0,
                        true,
                        false,
                    )
                    .on_hover_text("Close");
                    if close.clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    let maximize = theme::icon_button(
                        ui,
                        win_max_tex.as_ref(),
                        theme::Icon::Maximize,
                        13.0,
                        true,
                        false,
                    )
                    .on_hover_text(if maximized {
                        "Restore"
                    } else {
                        "Maximize"
                    });
                    if maximize.clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                    let minimize = theme::icon_button(
                        ui,
                        win_min_tex.as_ref(),
                        theme::Icon::Minimize,
                        13.0,
                        true,
                        false,
                    )
                    .on_hover_text("Minimize");
                    if minimize.clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });
        });

    // Each leaf holds exactly one pane, so its tab (a draggable full-width header
    // strip) doubles as the pane's title bar. No tabbing ever occurs.
    let mut style = Style::from_egui(ctx.style().as_ref());
    style.tab_bar.fill_tab_bar = true;

    let border_v = style.tab_bar.height
        + style.tab.tab_body.inner_margin.top
        + style.tab.tab_body.inner_margin.bottom;
    let border_h = style.tab.tab_body.inner_margin.left + style.tab.tab_body.inner_margin.right;

    let mut viewer = PaneViewer {
        app,
        themes: &*themes,
    };
    DockArea::new(&mut tree)
        .show_add_buttons(false)
        .show_add_popup(false)
        .show_close_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .style(style)
        .show(ctx, &mut viewer);

    // Enforce minimum pane sizes after layout: the splitter drag and floating
    // window resize both happen inside `show()`, so this is the only pass that
    // can see the result and nothing floors a pane before its opening frame.
    let mut corrected = apply_min_pane_sizes(&mut tree, ctx, border_v, border_h);
    // EQ pane minimum width: `BANDS` bands at their minimum width. This block is
    // the floating-window path — the split-fraction pass above cannot floor a
    // torn-off pane's width.
    let eq_min_w = ctx
        .data(|d| d.get_temp::<f32>(egui::Id::new(PANE_CONTENT_W).with(Pane::Equalizer)))
        .unwrap_or(0.0);
    if eq_min_w > 0.0 {
        let eq_surface = tree
            .iter_all_tabs()
            .find(|(_, t)| **t == Pane::Equalizer)
            .map(|((s, _), _)| s);
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
    if corrected {
        ctx.request_repaint();
    }

    // Modals last: a dialog is armed by a click anywhere in the frame (a pane,
    // this menu) and drawn + carried out here, above everything.
    dialogs::show(app, &*themes, &mut tree, ctx);

    // Persist to egui memory (session) + disk — only on change or close.
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_ID), tree.clone()));
    if let Some(path) = layout_path() {
        let json = serde_json::to_string(&tree).unwrap_or_default();
        let saved = ctx
            .data(|d| d.get_temp::<String>(egui::Id::new(DOCK_SAVED_JSON)))
            .unwrap_or_default();
        let closing = ctx.input(|i| i.viewport().close_requested());
        if json != saved || closing {
            save_layout(&tree, &path);
            ctx.data_mut(|d| d.insert_temp(egui::Id::new(DOCK_SAVED_JSON), json));
        }
    }
}
