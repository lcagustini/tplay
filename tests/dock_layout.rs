//! Dock layout persistence — pin the egui_dock API surface that
//! `gui::coordinator` depends on: build a tree, save to JSON, restore, and
//! verify tabs/splits/floating windows survive.
//!
//! Headless caveat: leaves that were never painted carry `Rect::NOTHING`
//! viewports, and serde_json serializes f32::INFINITY as `null` — such JSON
//! can't parse back. Sessions only save *after* painting, so every on-disk
//! layout has finite rects; `lay_out` + `finite` replicate that state.

use eframe::egui::{pos2, vec2, Rect};
use egui_dock::{DockState, NodeIndex, SurfaceIndex};
use tplay::app::Pane;

/// Mirror of `coordinator::default_tree` (NowPlaying above Playlist).
fn two_pane_tree() -> DockState<Pane> {
    let mut d = DockState::new(vec![Pane::NowPlaying]);
    d.main_surface_mut()
        .split_below(NodeIndex::root(), 0.15, vec![Pane::Playlist]);
    d
}

/// Mirror of `coordinator::add_pane`: split below the bottom-most leaf.
fn add_pane_bottom(tree: &mut DockState<Pane>, pane: Pane) {
    let leaf = tree
        .main_surface()
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, n)| n.is_leaf().then_some(NodeIndex(i)))
        .unwrap();
    tree.main_surface_mut().split_below(leaf, 0.5, vec![pane]);
}

/// Give every node a finite layout rect, like a rendered frame does.
fn lay_out(tree: &mut DockState<Pane>) {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
    for i in 0..tree.surfaces_count() {
        let surface = SurfaceIndex(i);
        for node in tree[surface].iter_mut() {
            node.set_rect(rect);
        }
    }
}

/// Serialize → make the unpainted-viewport nulls finite → deserialize.
/// Returns the restored tree; asserts serialization is stable after restore
/// (what the coordinator's save/load cycle relies on).
fn round_trip(tree: &DockState<Pane>) -> DockState<Pane> {
    let json = serde_json::to_string(tree).unwrap();
    let json = json.replace(
        r#"{"min":{"x":null,"y":null},"max":{"x":null,"y":null}}"#,
        r#"{"min":{"x":0.0,"y":0.0},"max":{"x":0.0,"y":0.0}}"#,
    );
    let restored: DockState<Pane> = serde_json::from_str(&json).unwrap();
    assert_eq!(
        serde_json::to_string(&restored).unwrap(),
        json,
        "JSON must be stable"
    );
    restored
}

fn tabs_of(tree: &DockState<Pane>) -> Vec<Pane> {
    tree.iter_all_tabs().map(|(_, t)| *t).collect()
}

#[test]
fn layout_round_trips_through_json() {
    let mut tree = two_pane_tree();
    lay_out(&mut tree);
    let restored = round_trip(&tree);
    assert_eq!(tabs_of(&restored), vec![Pane::NowPlaying, Pane::Playlist]);
}

#[test]
fn all_four_panes_survive_serialization() {
    let mut tree = two_pane_tree();
    add_pane_bottom(&mut tree, Pane::Equalizer);
    add_pane_bottom(&mut tree, Pane::Library);
    lay_out(&mut tree);

    let restored = round_trip(&tree);
    let tabs = tabs_of(&restored);
    assert_eq!(tabs.len(), 4);
    for p in [
        Pane::NowPlaying,
        Pane::Playlist,
        Pane::Equalizer,
        Pane::Library,
    ] {
        assert!(tabs.contains(&p), "pane {p:?} lost in round trip");
    }
}

#[test]
fn removed_pane_is_gone_and_stays_gone() {
    let mut tree = two_pane_tree();
    // remove_tab is how the coordinator closes a pane from the ☰ menu. Since
    // egui_dock 0.19 the path comes back whole from `iter_all_tabs`, so there
    // is no tab index to guess.
    let target = tree
        .iter_all_tabs()
        .find(|(_, t)| **t == Pane::Playlist)
        .map(|(path, _)| path)
        .unwrap();
    tree.remove_tab(target);
    assert_eq!(tree.iter_all_tabs().count(), 1);
    lay_out(&mut tree);

    let restored = round_trip(&tree);
    assert_eq!(
        restored.iter_all_tabs().count(),
        1,
        "removed pane must not resurrect"
    );
    assert!(restored
        .iter_all_tabs()
        .all(|(_, t)| *t == Pane::NowPlaying));
}

#[test]
fn floating_window_round_trips() {
    let mut tree = two_pane_tree();
    // Tearing a tab off creates a separate (windowed) surface.
    tree.add_window(vec![Pane::Equalizer]);
    lay_out(&mut tree);

    let restored = round_trip(&tree);

    let surface = restored
        .iter_all_tabs()
        .find(|(_, t)| **t == Pane::Equalizer)
        .map(|(path, _)| path.surface)
        .unwrap();
    assert_eq!(
        restored.iter_all_tabs().count(),
        3,
        "2 docked panes + 1 floating pane"
    );
    assert!(
        !surface.is_main(),
        "floating pane must not live on the main surface"
    );
    // Re-addressing the floating surface keeps its tab list.
    let surface_tabs: Vec<Pane> = restored[surface]
        .iter()
        .filter_map(|n| n.tabs().map(|ts| ts.to_vec()))
        .flatten()
        .collect();
    assert_eq!(surface_tabs, vec![Pane::Equalizer]);
}

#[test]
fn floating_window_size_hint_round_trips() {
    let mut tree = two_pane_tree();
    tree.add_window(vec![Pane::Equalizer]);
    lay_out(&mut tree);

    let mut restored = round_trip(&tree);
    let surface = restored
        .iter_all_tabs()
        .find(|(_, t)| **t == Pane::Equalizer)
        .map(|(path, _)| path.surface)
        .unwrap();

    // set_size is what the coordinator applies to the EQ window's width; it
    // must serialize alongside the layout without breaking the round trip.
    restored
        .get_window_state_mut(surface)
        .unwrap()
        .set_size(vec2(240.0, 180.0));
    let json = serde_json::to_string(&restored).unwrap();
    let again: DockState<Pane> = serde_json::from_str(&json).unwrap();
    assert_eq!(again.iter_all_tabs().count(), 3);
    assert_eq!(serde_json::to_string(&again).unwrap(), json);
}
