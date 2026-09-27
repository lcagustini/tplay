//! Tag-scan concurrency — the state `tracks::TagReader` keeps between frames.
//!
//! Two behaviours that are only observable *across* scans, so they cannot be
//! tested from inside one:
//!
//! * A drain with no scan running is a no-op, and stays a no-op. This is the
//!   `Empty` branch (receiver alive, nothing queued) and the no-receiver branch
//!   at once — without it, a wedged reader would busy-spin every frame.
//! * A new request **replaces** the in-flight scan. That is safe only because
//!   the cache is per-track, so an abandoned scan costs a rescan of its own
//!   files and nothing more. This is why the Library can navigate away
//!   mid-scan without corrupting anything.
//!
//! These used to drive a raw `mpsc` channel and mirror `TPlayApp`'s drain loop
//! by hand, which tested the stdlib more than it tested the app. The drain now
//! lives on `TagReader`, so they drive that directly.
//!
//! The end-to-end path (a real scan, its results, and the scan ending) is in
//! `tracks_tests::tag_reader_requests_then_drains_and_clears`; the scan thread
//! itself is in `library_tests::scan_files_sends_results_and_stops_on_drop`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tplay::library::TrackInfo;
use tplay::network::Network;
use tplay::tracks::TagReader;
#[path = "common.rs"]
mod common;

#[test]
fn an_idle_reader_never_reports_a_change() {
    // No scan has ever been requested: there is no receiver at all.
    let mut reader = TagReader::new();
    let mut cache: HashMap<PathBuf, TrackInfo> = HashMap::new();
    assert!(
        !reader.drain_into(&mut cache),
        "a reader with no scan reports no change"
    );
    assert!(!reader.drain_into(&mut cache), "and keeps reporting none");
    assert!(cache.is_empty());
}

#[test]
fn a_dropped_scan_is_replaced_by_the_next_request() {
    let dir = common::test_dir("dropped_scan_is_replaced");
    let a = dir.join("a.wav");
    let b = dir.join("b.wav");
    let c = dir.join("c.wav");
    for p in [&a, &b, &c] {
        common::write_wav(p);
    }

    let mut reader = TagReader::new();
    let mut cache: HashMap<PathBuf, TrackInfo> = HashMap::new();
    let mut net = Network::new(vec![]);

    // First navigation starts scanning [a, b]…
    assert!(reader.request(&cache, &mut net, &[a.clone(), b.clone()]));
    // …and is abandoned before draining — the receiver is replaced mid-flight.
    // Nothing is drained from it, so `a` may or may not have been cached by the
    // time it was dropped, which is exactly the nondeterminism this rule
    // tolerates.
    assert!(reader.request(&cache, &mut net, &[b.clone(), c.clone()]));

    let deadline = Instant::now() + Duration::from_secs(20);
    while !cache.contains_key(&c) {
        assert!(
            Instant::now() < deadline,
            "the replacement scan never delivered c"
        );
        reader.drain_into(&mut cache);
        std::thread::yield_now();
    }

    // The cache is per-track, so the abandoned scan costs only a rescan of its
    // own files — the replacement delivered its track and nothing is corrupted.
    // If `a` did land, its value is real; if not, the next request will re-scan
    // it. Either way it must not be a blank or partial entry.
    if let Some(info) = cache.get(&a) {
        common::assert_duration_approx(
            info.duration,
            Duration::from_secs(1),
            "abandoned scan's result",
        );
    }
    common::assert_duration_approx(
        cache[&c].duration,
        Duration::from_secs(1),
        "replacement scan",
    );

    // And the reader still settles — replacing a scan must not wedge it.
    let deadline = Instant::now() + Duration::from_secs(20);
    while reader.drain_into(&mut cache) {
        assert!(
            Instant::now() < deadline,
            "reader wedged after replacing a scan"
        );
        std::thread::yield_now();
    }

    std::fs::remove_dir_all(&dir).unwrap();
}
