//! `tplay::tracks` — the source-agnostic track surface.
//!
//! These are the decisions the app used to make inline with `is_remote`
//! branches, so they are pure and headless-testable here. The two things worth
//! knowing before reading:
//!
//! * The spool dir is **injected**, never the real `dirs::cache_dir()`, so every
//!   test is hermetic and two tests can run in parallel without colliding.
//! * `local_file` and `local_file_now` are separate functions on purpose. They
//!   differ for exactly one case — a local track with no file on disk — and
//!   collapsing them is the mistake these tests exist to prevent.

use tplay::library::TrackInfo;
use tplay::network::{self, Network};
use tplay::tracks::{self, TagReader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
#[path = "common.rs"]
mod common;

/// A remote track resolves to its spool-cache copy, and a local one comes back
/// unchanged. The cache dir is injected, so this never touches the real one.
///
/// (This moved here from `smb_helpers.rs`, which tested it as
/// `network::local_copy`. It is the Album Cover pane's read path: there is no
/// file on disk named `smb://…`, so the art is in the cache or it is nowhere.)
#[test]
fn local_file_resolves_remote_tracks_to_the_spool_cache() {
    let dir = common::test_dir("local_file_resolves_remote_tracks_to_the_spool_cache");

    let uri = "smb://nas/media/song.mp3";
    let cached = network::cache_path_in(uri, &dir);
    assert_eq!(cached.parent().unwrap(), dir, "premise: cache file lands in the dir");

    // Remote, not spooled yet -> nothing local to read.
    assert_eq!(tracks::local_file_in(Path::new(uri), &dir), None);

    // Remote, spooled -> the cache copy, extension preserved so lofty can read
    // the embedded picture.
    std::fs::write(&cached, b"not really audio").unwrap();
    assert_eq!(tracks::local_file_in(Path::new(uri), &dir), Some(cached.clone()));
    assert_eq!(cached.extension().unwrap(), "mp3");

    // A local path comes back unchanged, and does NOT require the file to
    // exist: the caller's own error handling decides what that means, and
    // requiring existence here would silently blank art for a track that is
    // mid-load.
    let local = Path::new("/music/local.flac");
    assert_eq!(tracks::local_file_in(local, &dir), Some(local.to_path_buf()));
    let missing = Path::new("/music/gone.flac");
    assert_eq!(tracks::local_file_in(missing, &dir), Some(missing.to_path_buf()));

    // Extensionless URIs still resolve, by key.
    let no_ext = "smb://nas/media/track";
    assert_eq!(tracks::local_file_in(Path::new(no_ext), &dir), None);
    let no_ext_cached = network::cache_path_in(no_ext, &dir);
    std::fs::write(&no_ext_cached, b"x").unwrap();
    assert_eq!(
        tracks::local_file_in(Path::new(no_ext), &dir),
        Some(no_ext_cached)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// The reason these are two functions. A *local* track with no file on disk
/// resolves for an art read (which reports its own "no art") but does not
/// resolve for a pre-buffer (which has nothing to open and must wait).
///
/// Conflating them would either blank the album art of every track that fails
/// to open, or make `advance()` treat "there is no file to pre-buffer" as
/// "this is a remote track".
#[test]
fn local_file_and_local_file_now_differ_only_for_a_missing_local_file() {
    let dir = common::test_dir("local_file_and_local_file_now_differ_only");
    let present = dir.join("present.wav");
    common::write_wav(&present);
    let missing = dir.join("missing.wav");

    // Local, present: both answer with the path.
    assert_eq!(
        tracks::local_file_in(&present, &dir),
        tracks::local_file_now_in(&present, &dir)
    );
    assert!(tracks::local_file_now_in(&present, &dir).is_some());

    // Local, missing: THE difference. The art read still gets the path so it
    // can try and report its own failure; the pre-buffer correctly gets nothing.
    assert_eq!(
        tracks::local_file_in(&missing, &dir),
        Some(missing.to_path_buf()),
        "art read must still be attempted for a local track that failed to open"
    );
    assert_eq!(
        tracks::local_file_now_in(&missing, &dir),
        None,
        "nothing to pre-buffer when there is no file"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Pins the agreed behaviour change: gapless and crossfade are no longer a
/// source-based rule. `advance()` asks `local_file_now`, so an **already
/// spooled** remote track pre-buffers like any other track. An uncached one
/// cannot, and falls through to a natural-advance gap — the same thing a local
/// track with a missing file already did.
#[test]
fn pre_availability_decides_pre_buffering_not_the_source() {
    let dir = common::test_dir("pre_availability_decides_pre_buffering");

    // Uncached remote: cannot pre-buffer.
    let uri = "smb://nas/media/song.flac";
    assert!(
        tracks::local_file_now_in(Path::new(uri), &dir).is_none(),
        "an uncached remote track has no bytes to pre-buffer"
    );

    // Same track, once spooled: can pre-buffer. This is the change.
    std::fs::write(network::cache_path_in(uri, &dir), b"audio").unwrap();
    assert!(
        tracks::local_file_now_in(Path::new(uri), &dir).is_some(),
        "a cached remote track pre-buffers exactly like a local one"
    );

    // A local file that is genuinely missing still cannot — same rule, same
    // outcome, and that symmetry is the point of the change.
    assert!(tracks::local_file_now_in(&dir.join("nope.wav"), &dir).is_none());

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Playlist entries normalize to the id everything else stores. A local path is
/// canonicalized and dropped when it is gone; a remote URI is kept **verbatim**,
/// because rewriting it would break the spool-cache key that both playback and
/// tag lookup use.
#[test]
fn normalize_keeps_remote_uris_verbatim_and_drops_missing_local_paths() {
    let dir = common::test_dir("normalize_playlist_entries");
    let present = dir.join("present.wav");
    common::write_wav(&present);

    // Local, present: canonicalized, so `dir/./x` and `dir/x` are one entry.
    let messy = dir.join(".").join("present.wav");
    assert_eq!(tracks::normalize(messy.clone()), Some(present.canonicalize().unwrap()));

    // Local, missing: dropped — a playlist must not resurrect a deleted file.
    assert_eq!(tracks::normalize(dir.join("gone.wav")), None);

    // Remote: verbatim, and it survives a cache that has never seen it.
    for uri in [
        "smb://nas/media/song.mp3",
        "smb://nas/share/./song.mp3",
        "smb://nas",
    ] {
        assert_eq!(
            tracks::normalize(PathBuf::from(uri)),
            Some(PathBuf::from(uri)),
            "a remote URI is its own id and must not be rewritten"
        );
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

/// One batch, two transports, order preserved. This split is real — a thread
/// with an mpsc versus a worker command with a reply `Event` — it just is not
/// the caller's business.
#[test]
fn split_for_tags_splits_a_mixed_batch() {
    let batch: Vec<PathBuf> = [
        "/music/a.wav",
        "smb://nas/media/b.mp3",
        "/music/c.flac",
        "smb://nas/media/d.tplay-not-a-track",
        "/music/e.wav",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();

    let (local, remote) = tracks::split_for_tags(batch);
    assert_eq!(
        local,
        vec![PathBuf::from("/music/a.wav"), PathBuf::from("/music/c.flac"), PathBuf::from("/music/e.wav")],
        "local batch keeps its order"
    );
    assert_eq!(
        remote,
        vec![
            "smb://nas/media/b.mp3".to_string(),
            "smb://nas/media/d.tplay-not-a-track".to_string()
        ],
        "remote batch keeps its order"
    );

    // Nothing in, nothing out — no empty batch should reach a transport.
    let (local, remote) = tracks::split_for_tags(vec![]);
    assert!(local.is_empty() && remote.is_empty());
}

/// The local half of the tag pipeline, end to end: `request` skips what the
/// cache already has, spawns one scan, and `drain_into` collects the results
/// and then clears its receiver — which is what makes "Scanning…" disappear.
///
/// `Network::new(vec![])` spawns the worker thread but sends it nothing, so this
/// stays hermetic: no server, no network, no port.
#[test]
fn tag_reader_requests_then_drains_and_clears() {
    let dir = common::test_dir("tag_reader_requests_then_drains");
    let paths: Vec<PathBuf> = ["a.wav", "b.wav", "c.wav"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    for p in &paths {
        common::write_wav(p);
    }

    let mut reader = TagReader::new();
    let mut cache: HashMap<PathBuf, TrackInfo> = HashMap::new();
    let mut net = Network::new(vec![]);

    // An empty request starts nothing.
    assert!(!reader.request(&cache, &mut net, &[]));

    // A real request starts a local scan.
    assert!(reader.request(&cache, &mut net, &paths), "a local scan must be started");

    // Drain until every result lands, bounded so a regression fails the test
    // instead of hanging the suite.
    let deadline = Instant::now() + Duration::from_secs(20);
    while cache.len() < paths.len() {
        assert!(Instant::now() < deadline, "timed out waiting for {} of {}", cache.len(), paths.len());
        reader.drain_into(&mut cache);
        std::thread::yield_now();
    }
    for p in &paths {
        let info = cache.get(p).unwrap_or_else(|| panic!("missing {p:?}"));
        common::assert_duration_approx(info.duration, Duration::from_secs(1), "scanned duration");
    }

    // The scan has produced everything it will ever produce, so the reader must
    // settle to idle: every later drain reports "no change". The drain that
    // collects the last result is usually also the one that sees the thread
    // disconnect and drops the receiver, so "the scan ended" is not a separate
    // signal to wait for — the observable end state is that it goes quiet and
    // stays quiet. (`library_scanning()` keys off the cache being complete, not
    // off this, so this is about the receiver being released, not the label.)
    let deadline = Instant::now() + Duration::from_secs(20);
    while reader.drain_into(&mut cache) {
        assert!(Instant::now() < deadline, "reader kept reporting changes after every result landed");
        std::thread::yield_now();
    }
    assert!(
        !reader.drain_into(&mut cache),
        "a settled reader must stay idle, not busy-spin on a dead receiver"
    );

    // A cached track is never re-requested, so this must not start a scan.
    assert!(!reader.request(&cache, &mut net, &paths), "cached tracks must be skipped");

    std::fs::remove_dir_all(&dir).unwrap();
}

/// `Ok` is cached even when blank; `Err` is not. This asymmetry is the whole
/// reason the two outcomes are not handled the same way, and it is easy to
/// "tidy up" into a bug.
#[test]
fn tag_reader_absorb_caches_ok_and_leaves_err_retryable() {
    let reader = TagReader::new();
    let mut cache: HashMap<PathBuf, TrackInfo> = HashMap::new();

    // A successful read of a genuinely untagged file: the definitive answer.
    // Caching it is what stops an untagged folder re-requesting forever, and
    // `title_or_stem` falls back to the stem so it displays like no entry.
    let blank = PathBuf::from("smb://nas/media/untagged.mp3");
    let real = PathBuf::from("smb://nas/media/tagged.mp3");
    reader.absorb(
        &mut cache,
        vec![
            (blank.to_string_lossy().into_owned(), Ok(TrackInfo::default())),
            (
                real.to_string_lossy().into_owned(),
                Ok(TrackInfo { title: "Real Title".into(), ..Default::default() }),
            ),
        ],
    );
    assert_eq!(cache.len(), 2, "both successes cached, including the blank one");
    // `TrackInfo` has no PartialEq, and the fields are the point: the blank read
    // really is empty, which is what makes it a final answer rather than a
    // missing one.
    let blank_info = &cache[&blank];
    assert!(blank_info.title.is_empty() && blank_info.artist.is_empty(), "a blank read is empty");
    assert_eq!(blank_info.duration, None, "a blank read has no duration either");
    assert_eq!(cache[&real].title, "Real Title");

    // A failure says nothing about the file's tags, so it must NOT be cached:
    // doing so would blank the row for the rest of the session. Leaving it out
    // is what makes it retryable.
    let failed = PathBuf::from("smb://nas/media/failed.mp3");
    let mut cache2: HashMap<PathBuf, TrackInfo> = HashMap::new();
    reader.absorb(
        &mut cache2,
        vec![
            (failed.to_string_lossy().into_owned(), Err("connection reset".to_string())),
            (real.to_string_lossy().into_owned(), Ok(TrackInfo::default())),
        ],
    );
    assert!(!cache2.contains_key(&failed), "a failed transfer must stay retryable");
    assert_eq!(cache2.len(), 1, "the successful half of the same batch still lands");
}
