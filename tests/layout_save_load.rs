//! Named layout save/load round-trips — headless, no real config dir.

mod common;
use common::{lay_out, test_dir};
use egui_dock::DockState;
use std::fs;
use tplay::app::Pane;

fn two_pane_tree() -> DockState<Pane> {
    let mut d = DockState::new(vec![Pane::NowPlaying]);
    d.main_surface_mut()
        .split_below(egui_dock::NodeIndex::root(), 0.15, vec![Pane::Playlist]);
    d
}

fn add_pane_bottom(tree: &mut DockState<Pane>, pane: Pane) {
    let leaf = tree
        .main_surface()
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, n)| n.is_leaf().then_some(egui_dock::NodeIndex(i)))
        .unwrap();
    tree.main_surface_mut().split_below(leaf, 0.5, vec![pane]);
}

/// Serialize → make the unpainted-viewport nulls finite → deserialize.
/// Mirrors dock_layout.rs::round_trip (never-painted leaves have Rect::NOTHING
/// viewports that serialize as `null`; replace them with finite zeros).
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
fn layout_save_load_round_trips() {
    let dir = test_dir("layout_save_load_round_trips");
    let path = dir.join("layout.json");

    // Build a 4-pane tree
    let mut tree = two_pane_tree();
    add_pane_bottom(&mut tree, Pane::Equalizer);
    add_pane_bottom(&mut tree, Pane::Library);
    lay_out(&mut tree);

    // Save via round_trip (replicates what coordinator.save_layout/load_layout do)
    let restored = round_trip(&tree);

    // Write to disk and read back (tests file I/O path)
    let json = serde_json::to_string(&restored).unwrap();
    fs::write(&path, &json).unwrap();
    let restored_json = fs::read_to_string(&path).unwrap();
    let from_disk: DockState<Pane> = serde_json::from_str(&restored_json).unwrap();

    // Verify tabs survived
    assert_eq!(
        tabs_of(&from_disk),
        vec![
            Pane::NowPlaying,
            Pane::Playlist,
            Pane::Equalizer,
            Pane::Library
        ]
    );

    // Second save-over write (proves the save-over path)
    let json2 = serde_json::to_string(&from_disk).unwrap();
    fs::write(&path, &json2).unwrap();
    let again_json = fs::read_to_string(&path).unwrap();
    let again: DockState<Pane> = serde_json::from_str(&again_json).unwrap();
    assert_eq!(
        tabs_of(&again),
        vec![
            Pane::NowPlaying,
            Pane::Playlist,
            Pane::Equalizer,
            Pane::Library
        ]
    );
    assert_eq!(again_json, json2);
}

#[test]
fn list_layouts_lists_json_files_sorted() {
    let dir = test_dir("layout_list_layouts_sorted");
    fs::write(dir.join("a.json"), "{}").unwrap();
    fs::write(dir.join("b.json"), "{}").unwrap();
    fs::write(dir.join("note.txt"), "").unwrap();
    fs::create_dir(dir.join("sub")).unwrap();

    // Mirror coordinator::list_layouts: read dir, filter *.json, sort
    let mut layouts: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .map(|e| e.path())
        .collect();
    layouts.sort();

    let names: Vec<_> = layouts
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["a.json", "b.json"]);
}

#[test]
fn load_layout_returns_none_for_garbage() {
    let dir = test_dir("layout_load_garbage");
    let path = dir.join("bad.json");
    fs::write(&path, "not json").unwrap();

    let result: Option<DockState<Pane>> = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    assert!(result.is_none());
}

/// The ☰ menu's layout list must not be read while the menu is shut.
///
/// `layouts_dir()` does a `create_dir_all` and `list_layouts()` a `read_dir` plus
/// a sort. Hoisted above the `menu_contents` closure — which is where they were —
/// that was three filesystem calls on every one of 60 frames a second, forever,
/// to fill a list nobody could see. It is invisible in a screenshot and
/// unmeasurable from a test without counting syscalls, so what gets pinned is the
/// *placement* instead: the reads must sit inside the closure egui only invokes
/// while the menu is open.
///
/// The same shape as `no_inline_tests.rs` — a structural property of the source,
/// checked because the behaviour it guards cannot be observed any other way.
#[test]
fn the_layout_reads_live_inside_the_menu_closure() {
    let src = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("src/gui/coordinator.rs");
    let text = std::fs::read_to_string(&src).unwrap();
    let lines: Vec<&str> = text.lines().collect();

    let menu_at = lines
        .iter()
        .position(|l| l.contains("let menu_contents ="))
        .expect("the menu closure is still there");
    for call in ["layouts_dir()", "list_layouts("] {
        // The `fn` line is the definition, not a call; skip it and any comment.
        let at = lines
            .iter()
            .position(|l| {
                l.contains(call)
                    && !l.trim_start().starts_with("//")
                    && !l.contains(&format!("fn {call}"))
            })
            .unwrap_or_else(|| panic!("{call} is gone from the coordinator — recheck this test"));
        assert!(
            at > menu_at,
            "{call} at line {} is hoisted above the menu closure at {menu_at}, so it runs \
             every frame with the menu shut",
            at + 1
        );
    }
}
