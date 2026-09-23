//! Tag-scan concurrency — the mpsc drain pattern `TPlayApp::drain_tag_scan`
//! runs every frame: results arrive out of band from a background
//! `library::scan_files` thread, the drain loop distinguishes Empty (keep
//! waiting) from Disconnected (thread done), and a dropped receiver mid-scan
//! is replaced by the next navigation without corrupting the per-path cache.
//!
//! (The scan thread itself — send + early-exit on a dropped receiver — is
//! already covered in `library_tests::scan_files_sends_results_and_stops_on_drop`.)

use tplay::library::{TrackInfo, scan_files};
use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;
#[path = "common.rs"]
mod common;

#[test]
fn drain_loop_ends_on_disconnect_after_all_results() {
    let dir = common::test_dir("drain_loop_ends_on_disconnect");
    let paths: Vec<PathBuf> = ["a.wav", "b.wav", "c.wav"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    for p in &paths {
        common::write_wav(p);
    }

    let (tx, rx) = mpsc::channel();
    let scan_paths = paths.clone();
    std::thread::spawn(move || scan_files(scan_paths, tx));

    // Mirror drain_tag_scan: try_recv until Disconnected, draining into a map.
    let mut results = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(r) => results.push(r),
            Err(TryRecvError::Empty) => std::thread::yield_now(),
            Err(TryRecvError::Disconnected) => break,
        }
    }
    assert_eq!(results.len(), 3, "all scanned files must arrive before disconnect");

    // Every result is a distinct known path with a real 1 s duration.
    let mut seen = Vec::new();
    for (path, info) in &results {
        assert!(paths.contains(path), "unexpected result path {path:?}");
        assert!(!seen.contains(path), "duplicate result {path:?}");
        seen.push(path.clone());
        common::assert_duration_approx(info.duration, Duration::from_secs(1), "tagged duration");
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn drain_loop_branches_empty_and_disconnected() {
    // Empty: receiver alive, nothing queued → the frame's drain just breaks.
    let (_tx, rx) = mpsc::channel::<(PathBuf, TrackInfo)>();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

    // Disconnected: the sender thread finished → drain marks the scan ended.
    drop(_tx);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
}

#[test]
fn dropped_receiver_is_replaced_by_next_scan() {
    let dir = common::test_dir("dropped_receiver_is_replaced");
    let a = dir.join("a.wav");
    let b = dir.join("b.wav");
    let c = dir.join("c.wav");
    for p in [&a, &b, &c] {
        common::write_wav(p);
    }

    // First navigation starts scanning [a, b]…
    let (tx1, rx1) = mpsc::channel();
    let scan1 = vec![a.clone(), b.clone()];
    std::thread::spawn(move || scan_files(scan1, tx1));
    // …and is abandoned (dropped receiver) before draining.
    drop(rx1);

    // Next navigation replaces it with [b, c]; the cache is per-path, so the
    // dropped scan costs nothing but a rescan of its own files.
    let (tx2, rx2) = mpsc::channel();
    let scan2 = vec![b.clone(), c.clone()];
    std::thread::spawn(move || scan_files(scan2, tx2));

    let mut results = Vec::new();
    loop {
        match rx2.try_recv() {
            Ok(r) => results.push(r),
            Err(TryRecvError::Empty) => std::thread::yield_now(),
            Err(TryRecvError::Disconnected) => break,
        }
    }
    let got: Vec<&PathBuf> = results.iter().map(|(p, _)| p).collect();
    assert_eq!(got.len(), 2, "replacement scan delivers exactly its two files");
    assert!(got.contains(&&b));
    assert!(got.contains(&&c));

    std::fs::remove_dir_all(&dir).unwrap();
}