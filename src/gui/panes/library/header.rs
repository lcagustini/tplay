//! The Library pane's header row: one breadcrumb, agnostic of source.
//!
//! The local folder browser and the SMB browser navigate differently but look
//! the same, so the *only* thing shared is the widget: `breadcrumb` renders
//! whatever `Seg`s it is handed, and the two builders differ solely in which
//! segments they produce and what a click means.

use super::dir_name;
use crate::app::TPlayApp;
use crate::gui::theme::{self, ThemeState};
use crate::network;
use eframe::egui;
use std::path::{Path, PathBuf};

/// Cap on one segment's width, so a long name truncates instead of shoving the
/// ★ toggle (or Add All) off the pane edge.
const SEG_MAX_W: f32 = 70.0;
/// Approx. width of one character in a segment label, plus its padding. The label
/// has a fixed size rather than being measured, which is what the truncation cap
/// depends on.
const SEG_CHAR_W: f32 = 8.0;
const SEG_PAD_W: f32 = 6.0;
/// Height of one segment — the row the whole header sits on.
const SEG_H: f32 = 18.0;

/// What clicking a breadcrumb segment does.
///
/// An enum, not a closure: a closure cannot be stored in egui memory (see
/// `gui/dialogs.rs`) and an enum keeps `apply` a single dispatch point that
/// neither caller has to duplicate.
#[derive(Debug)]
pub enum Action {
    /// A local folder: list it.
    GoTo(PathBuf),
    /// The "Local" segment: leave network mode for the folder browser.
    LeaveNetwork,
    /// A server: re-list its shares.
    BrowseServer(String),
    /// A share, or a directory inside one.
    BrowseOpen { uri: String, share: String, rel: String },
}

/// One breadcrumb segment.
pub struct Seg {
    pub label: String,
    pub action: Action,
    /// Full path/URI, shown on hover. A segment is width-capped and can
    /// truncate to nothing, so this is the only way to see where it points; a
    /// share segment needs none (its label is the whole name).
    pub hover: Option<String>,
}

/// One step of the drawn breadcrumb.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Draw this segment of the source's list.
    Seg(usize),
    /// Draw the `…` standing in for the collapsed ones.
    Ellipsis,
}

/// Which segments a breadcrumb of `n` draws, in order.
///
/// Pure, and the whole of the `…` rule, so it is testable without an `egui::Ui`:
/// the three real cases — a deep local path, the share-list stage, a deep share
/// path — differ only in `n` and `always`.
///
/// `always` is how many leading segments are exempt from collapsing, and it is
/// the one thing that differs per source: 1 locally (just the filesystem root)
/// and 3 on a share (`Local` / host / share), because those three are the way back
/// out to a bigger scope. With `always = 1` for both, a deep share path loses its
/// route to the share root — a dead end, with no `..` row to fall back on. The
/// share-list stage is only 2 segments, so it sits under `always` entirely and the
/// "Local" exit can never collapse away.
pub fn plan(n: usize, always: usize) -> Vec<Step> {
    let mut steps = Vec::with_capacity(n);
    for i in 0..n {
        if i >= always && i + 2 < n {
            if i == always {
                steps.push(Step::Ellipsis);
            }
        } else {
            steps.push(Step::Seg(i));
        }
    }
    steps
}

/// Draws the breadcrumb and returns the segment that was clicked, if any.
///
/// Source-agnostic: no branch here knows about local paths or SMB URIs — the
/// collapsing is `plan`'s, and the segments come from `local_segs` or
/// `remote_segs`. The `…` is a plain label, so the segments it hides are not
/// reachable from it.
fn breadcrumb<'a>(ui: &mut egui::Ui, p: theme::Palette, segs: &'a [Seg], always: usize) -> Option<&'a Seg> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        for step in plan(segs.len(), always) {
            let i = match step {
                Step::Ellipsis => {
                    ui.label(egui::RichText::new("…").small().color(p.text_secondary));
                    continue;
                }
                Step::Seg(i) => i,
            };
            let seg = &segs[i];
            if i + 1 == segs.len() {
                // Where we are: shown in full, not clickable.
                let label = ui.label(egui::RichText::new(&seg.label).strong().color(p.text_primary));
                if let Some(h) = &seg.hover {
                    label.on_hover_text(h);
                }
                continue;
            }
            let w = (seg.label.chars().count() as f32 * SEG_CHAR_W + SEG_PAD_W).min(SEG_MAX_W);
            let resp = ui.add_sized(
                egui::vec2(w, SEG_H),
                egui::Label::new(egui::RichText::new(&seg.label).small().color(p.text_secondary))
                    .truncate()
                    .sense(egui::Sense::click()),
            );
            let resp = match &seg.hover {
                Some(h) => resp.on_hover_text(h.as_str()),
                None => resp,
            };
            if resp.clicked() {
                clicked = Some(seg);
            }
            ui.label(egui::RichText::new("/").small().color(p.text_secondary));
        }
    });
    clicked
}

/// Carries out a clicked segment. One match, so neither builder needs an arm for
/// the other's source.
fn apply(app: &mut TPlayApp, seg: &Seg) {
    match &seg.action {
        Action::GoTo(dir) => app.navigate_to(dir.clone()),
        Action::LeaveNetwork => app.network_mut().leave_network(),
        Action::BrowseServer(host) => app.network_mut().browse_server(host.clone()),
        Action::BrowseOpen { uri, share, rel } => {
            app.network_mut()
                .browse_open(uri.clone(), Some(share.clone()), rel.clone())
        }
    }
}

/// The local browser's header: every ancestor of the current folder, then the
/// ★ that bookmarks it. The star is local-only, which is why it is here and not
/// in `breadcrumb`.
pub fn local_header(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui, theme: &theme::Theme) {
    let p = theme.palette;
    let dir = app.library().dir().to_path_buf();
    let segs = local_segs(&dir);
    // 1 = only the filesystem root is exempt from the `…` collapse.
    if let Some(seg) = breadcrumb(ui, p, &segs, 1) {
        apply(app, seg);
    }

    let fav = app.library().is_favorite(&dir);
    let tex = if fav {
        themes.icon(theme::Icon::StarOn).cloned()
    } else {
        themes.icon(theme::Icon::StarOff).cloned()
    };
    let fav_btn = match tex {
        Some(tex) => egui::Button::image(egui::Image::new(&tex).fit_to_exact_size(egui::vec2(14.0, 14.0)))
            .selected(fav),
        None => egui::Button::new(if fav { "★" } else { "☆" }).selected(fav),
    };
    if ui.add(fav_btn).on_hover_text("Favorite folder").clicked() {
        app.library_mut().toggle_favorite(dir);
    }
}

/// The remote browser's header: Local → host → share → each directory level.
pub fn remote_header(app: &mut TPlayApp, ui: &mut egui::Ui, theme: &theme::Theme, browse: &network::NetworkBrowse) {
    let segs = remote_segs(browse);
    // 3 = Local / host / share, so a deep directory still has a way back to the
    // share root. The share-list stage is only 2 segments, under `always` and so
    // always shown — the exit never disappears.
    if let Some(seg) = breadcrumb(ui, theme.palette, &segs, 3) {
        apply(app, seg);
    }
}

/// Root → current, so "one level up" is the second-to-last segment. The
/// breadcrumb is the only way up: there is deliberately no `..` row, which
/// duplicated navigation, consumed a banded row, and was counted toward the
/// folder total in the composition counts.
pub fn local_segs(dir: &Path) -> Vec<Seg> {
    let mut segs: Vec<PathBuf> = Vec::new();
    let mut cur = Some(dir.to_path_buf());
    while let Some(d) = cur {
        cur = d.parent().map(Path::to_path_buf);
        segs.push(d);
    }
    segs.reverse();
    segs.into_iter()
        .map(|p| Seg {
            hover: Some(p.display().to_string()),
            label: dir_name(&p),
            action: Action::GoTo(p),
        })
        .collect()
}

pub fn remote_segs(browse: &network::NetworkBrowse) -> Vec<Seg> {
    let seg = |label: String, action: Action| Seg { label, action, hover: None };
    // "Local" is the exit back to the folder browser; at the share-list stage it
    // is the only way out, so it is segment 0 and never collapsed away.
    let mut segs = vec![seg(String::from("Local"), Action::LeaveNetwork)];
    segs.push(seg(browse.host.clone(), Action::BrowseServer(browse.host.clone())));
    let Some(share) = &browse.share else { return segs };
    segs.push(seg(
        share.clone(),
        Action::BrowseOpen {
            uri: network::share_uri(&browse.host, share),
            share: share.clone(),
            rel: String::new(),
        },
    ));
    // One segment per directory level, each carrying the walk to that level.
    let mut walk = String::new();
    for part in browse.rel.split('/') {
        if !walk.is_empty() {
            walk.push('/');
        }
        walk.push_str(part);
        segs.push(seg(
            part.to_string(),
            Action::BrowseOpen {
                uri: network::dir_uri(&browse.host, share, &walk),
                share: share.clone(),
                rel: walk.clone(),
            },
        ));
    }
    segs
}
