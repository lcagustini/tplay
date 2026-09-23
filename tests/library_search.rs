//! Library sorting edge cases beyond `library_tests.rs` — `sort_key` is the
//! comparator behind the sortable header; these pin the encoding contract:
//! lowercase fold (case-insensitive), DEL prefix for missing values, and
//! zero-padded millisecond durations. (The search text box lives in the
//! Library pane; its matching is plain `contains` over a lowercased
//! title·artist·album haystack built from these same fields.)

use tplay::library::{Entry, TrackInfo, sort_key};
use std::path::PathBuf;
use std::time::Duration;

fn file(name: &str) -> Entry {
    Entry { path: PathBuf::from(name), is_dir: false }
}

fn folder(name: &str) -> Entry {
    Entry { path: PathBuf::from(name), is_dir: true }
}

#[test]
fn sort_key_is_case_insensitive() {
    // Mixed case must not change relative order.
    let z = file("z.wav");
    let a = file("A.wav");
    let z_info = TrackInfo { title: "Zebra".into(), ..Default::default() };
    let a_info = TrackInfo { title: "apple".into(), ..Default::default() };
    assert!(sort_key(&a, Some(&a_info), 0) < sort_key(&z, Some(&z_info), 0));

    // Identical name differing only in case ties.
    let upper = TrackInfo { title: "Star".into(), ..Default::default() };
    let lower = TrackInfo { title: "star".into(), ..Default::default() };
    assert_eq!(sort_key(&file("a.wav"), Some(&upper), 0), sort_key(&file("b.wav"), Some(&lower), 0));
}

#[test]
fn sort_key_year_missing_sinks_last() {
    let tagged = TrackInfo { year: Some("2000".into()), ..Default::default() };
    let untagged = TrackInfo { year: None, ..Default::default() };
    assert!(sort_key(&file("a.wav"), Some(&tagged), 3) < sort_key(&file("b.wav"), Some(&untagged), 3));
    assert!(sort_key(&file("a.wav"), Some(&tagged), 3) < sort_key(&file("b.wav"), None, 3));
}

#[test]
fn sort_key_year_orders_lexicographically() {
    // Four-digit years sort correctly as strings.
    let older = TrackInfo { year: Some("1987".into()), ..Default::default() };
    let newer = TrackInfo { year: Some("2020".into()), ..Default::default() };
    assert!(sort_key(&file("a.wav"), Some(&older), 3) < sort_key(&file("b.wav"), Some(&newer), 3));
}

#[test]
fn sort_key_genre_sorts_alphabetically() {
    let rock = TrackInfo { genre: "Rock".into(), ..Default::default() };
    let jazz = TrackInfo { genre: "jazz".into(), ..Default::default() };
    assert!(sort_key(&file("a.wav"), Some(&jazz), 4) < sort_key(&file("b.wav"), Some(&rock), 4));
    // Folder (no genre) sinks below any tagged file.
    let f = folder("a-dir");
    assert!(sort_key(&file("b.wav"), Some(&rock), 4) < sort_key(&f, None, 4));
}

#[test]
fn sort_key_duration_uses_zero_padded_millis() {
    // 9 s vs 10 s: naive second-comparison puts "9" after "10"; the padded
    // millisecond key must not. 9 s → "000000009000", 10 s → "000000010000".
    let nine = TrackInfo { duration: Some(Duration::from_secs(9)), ..Default::default() };
    let ten = TrackInfo { duration: Some(Duration::from_secs(10)), ..Default::default() };
    assert!(sort_key(&file("a.wav"), Some(&nine), 5) < sort_key(&file("b.wav"), Some(&ten), 5));

    // Sub-second precision is kept: 1.5 s vs 2 s.
    let one_five = TrackInfo { duration: Some(Duration::from_millis(1500)), ..Default::default() };
    let two = TrackInfo { duration: Some(Duration::from_secs(2)), ..Default::default() };
    assert!(sort_key(&file("a.wav"), Some(&one_five), 5) < sort_key(&file("b.wav"), Some(&two), 5));
}

#[test]
fn sort_key_folders_sink_on_tag_columns_below_files() {
    // On Year/Duration a folder is untagged → sorts last, even when the file
    // has the "later" value — folders always trail on tag columns.
    let f = folder("z-dir");
    let info = TrackInfo { year: Some("3000".into()), ..Default::default() };
    assert!(sort_key(&file("a.wav"), Some(&info), 3) < sort_key(&f, None, 3));
}