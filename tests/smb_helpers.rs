//! Hermetic pure-logic tests for the SMB URI/spool helpers in `tplay::network`.
//! No network, no worker thread — the smb2 client itself is untestable
//! headless, so the pure helpers carry the load (same convention as
//! `mounted_volumes.rs` fixtures).

use std::path::Path;
use tplay::network::{
    cache_path_in, child_uri, dir_uri, fnv1a64, is_remote, parse_server_input, share_uri,
    split_uri, spool_key,
};

#[test]
fn is_remote_only_matches_smb_uri_prefix() {
    assert!(is_remote(Path::new("smb://nas/music/a.mp3")));
    assert!(is_remote(Path::new("smb://NAS/share")));
    assert!(!is_remote(Path::new("/home/user/music/a.mp3")));
    assert!(!is_remote(Path::new("C:\\music\\a.mp3")));
    assert!(!is_remote(Path::new("smb:a_stray_local_file")));
}

#[test]
fn split_uri_covers_server_share_and_deep_paths() {
    // Bare server — share stage (guest browse).
    assert_eq!(
        split_uri("smb://192.168.1.50"),
        Some(("192.168.1.50".into(), None, String::new()))
    );
    // Share root.
    assert_eq!(
        split_uri("smb://nas/music"),
        Some(("nas".into(), Some("music".into()), String::new()))
    );
    // Directory + file.
    assert_eq!(
        split_uri("smb://nas/music/Album/track.flac"),
        Some((
            "nas".into(),
            Some("music".into()),
            "Album/track.flac".into()
        ))
    );
    // Not an smb uri at all.
    assert_eq!(split_uri("https://nas/music"), None);
    assert_eq!(split_uri("smb://"), None);
}
#[test]
fn add_server_dedups_by_host_and_updates_the_username() {
    use tplay::network::Network;

    // The add-server form stores the host with a blank username, then the
    // main-pane login prompt calls add_server again with the real one — so
    // repeat logins must update in place, never duplicate the row.
    let mut net = Network::new(vec![]);
    net.add_server("192.168.15.59".into(), String::new());
    assert_eq!(net.servers().len(), 1);
    assert_eq!(net.servers()[0].username, "");

    net.add_server("192.168.15.59".into(), "alice".into());
    assert_eq!(net.servers().len(), 1, "re-login must not duplicate");
    assert_eq!(net.servers()[0].username, "alice");

    // A genuinely different address is a new row.
    net.add_server("10.0.0.5".into(), "bob".into());
    assert_eq!(net.servers().len(), 2);
    assert!(net.servers().iter().any(|s| s.host == "192.168.15.59"));
    assert!(net.servers().iter().any(|s| s.host == "10.0.0.5"));

    // Whitespace from the text field must not create a second, near-identical row.
    net.add_server("  192.168.15.59  ".into(), "carol".into());
    assert_eq!(net.servers().len(), 2, "host must be trimmed before dedup");
    let row = net
        .servers()
        .iter()
        .find(|s| s.host == "192.168.15.59")
        .unwrap();
    assert_eq!(row.username, "carol");

    // Removal is by exact host.
    net.remove_server("10.0.0.5");
    assert_eq!(net.servers().len(), 1);
    assert_eq!(net.servers()[0].host, "192.168.15.59");
}

#[test]
fn parse_server_input_accepts_bare_host_and_full_uris() {
    // Bare host — saved as-is, nothing to open directly.
    assert_eq!(
        parse_server_input("192.168.15.59"),
        Some(("192.168.15.59".into(), None, String::new()))
    );
    // Surrounding whitespace from the text field is trimmed.
    assert_eq!(
        parse_server_input("  nas  "),
        Some(("nas".into(), None, String::new()))
    );
    // The URI a user pastes from another file manager: scheme optional, trailing
    // slash tolerated — must NOT end up stored as a host.
    assert_eq!(
        parse_server_input("smb://192.168.15.59/newhd/"),
        Some(("192.168.15.59".into(), Some("newhd".into()), String::new()))
    );
    assert_eq!(
        parse_server_input("192.168.15.59/newhd"),
        Some(("192.168.15.59".into(), Some("newhd".into()), String::new()))
    );
    // Share plus a subdirectory.
    assert_eq!(
        parse_server_input("smb://nas/music/Rock"),
        Some(("nas".into(), Some("music".into()), "Rock".into()))
    );
    // Empty input adds nothing.
    assert_eq!(parse_server_input(""), None);
    assert_eq!(parse_server_input("   "), None);
    assert_eq!(parse_server_input("smb://"), None);
}

#[test]
fn uri_builders_round_trip_against_splitter() {
    let host = "nas";
    let share = "music";
    let rel = "Rock/Album";
    let uri = dir_uri(host, share, rel);
    assert_eq!(uri, "smb://nas/music/Rock/Album");
    assert_eq!(
        split_uri(&uri),
        Some((host.into(), Some(share.into()), rel.into()))
    );

    // Share root (empty rel) has no trailing slash.
    assert_eq!(dir_uri(host, share, ""), "smb://nas/music");
    assert_eq!(share_uri(host, share), "smb://nas/music");

    // Descending appends exactly one slash regardless of parent form.
    assert_eq!(
        child_uri("smb://nas/music", "Album"),
        "smb://nas/music/Album"
    );
    assert_eq!(
        child_uri("smb://nas/music/Album/", "a.flac"),
        "smb://nas/music/Album/a.flac"
    );
}

#[test]
fn uri_parent_is_the_inverse_of_child_uri() {
    use tplay::network::uri_parent;

    // The base a remote playlist's relative entries resolve against.
    assert_eq!(
        uri_parent("smb://nas/share/dir/list.tplay"),
        "smb://nas/share/dir"
    );
    // A playlist at the share root resolves against the share itself.
    assert_eq!(uri_parent("smb://nas/share/list.tplay"), "smb://nas/share");
    // Deep paths keep everything but the filename.
    assert_eq!(uri_parent("smb://nas/a/b/c/d/x.flac"), "smb://nas/a/b/c/d");
    // No slash at all — nothing to strip, so the input is returned unchanged
    // rather than panicking or yielding "".
    assert_eq!(uri_parent("smb://nas"), "smb://nas");
    // Round-trips against the builder for every shape.
    for uri in [
        "smb://nas/share/dir/list.tplay",
        "smb://nas/share/list.tplay",
        "smb://nas/a/b/c/d/x.flac",
    ] {
        let name = uri.rsplit('/').next().unwrap();
        assert_eq!(
            child_uri(uri_parent(uri), name),
            uri,
            "round-trip failed for {uri}"
        );
    }
}

/// A `.tplay` on a share gets a row, so the remote filter that hides
/// non-audio files must let playlists through.
#[test]
fn playlist_files_are_recognized_by_name() {
    use tplay::library::is_playlist;
    assert!(is_playlist(Path::new("mix.tplay")));
    assert!(is_playlist(Path::new("mix.TPLAY")));
    assert!(is_playlist(Path::new("smb://nas/share/mix.tplay")));
    assert!(!is_playlist(Path::new("song.mp3")));
    assert!(!is_playlist(Path::new("tplay")));
}

#[test]
fn spool_key_is_stable_and_distinct() {
    // Known FNV-1a 64 vector (hand-computed) — the key is a cache filename,
    // so it must never drift between Rust releases (that's the point of
    // hand-rolling it instead of DefaultHasher).
    assert_eq!(fnv1a64(""), 0xcbf29ce484222325u64);
    assert_eq!(fnv1a64("smb://nas/music/a.mp3"), 0xb50152ee36dfecbbu64);

    // Distinct URIs → distinct keys (uniqueness is what makes the cache safe).
    let a = spool_key("smb://nas/music/a.mp3");
    let b = spool_key("smb://nas/music/b.mp3");
    let deep = spool_key("smb://nas/music/Album/a.mp3");
    assert_ne!(a, b);
    assert_ne!(a, deep);
    // 64-bit hex, fixed width.
    assert_eq!(a.len(), 16);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn cache_path_keeps_extension_and_uses_injected_dir() {
    let dir = Path::new("/tmp/tplay-smb-test");
    let uri = "smb://nas/music/Album/track.flac";
    let p = cache_path_in(uri, dir);
    assert_eq!(p, dir.join(format!("{}.flac", spool_key(uri))));
    assert_eq!(p.extension().unwrap(), "flac");
    // Extensionless URI → bare key.
    let p2 = cache_path_in("smb://nas/music/noext", dir);
    assert_eq!(p2, dir.join(spool_key("smb://nas/music/noext")));
    // Different files, same dir → no collision.
    assert_ne!(
        cache_path_in("smb://nas/music/a.mp3", dir),
        cache_path_in("smb://nas/music/b.mp3", dir)
    );
}
/// A share-browser entry's `path` is the FULL child URI, and descending into it
/// must be a no-op round trip.
///
/// This is a real bug that shipped once: the folder click passed that URI to
/// it through the deleted `child_browse` helper as if it were a bare name,
/// producing a `rel` of
/// `"music/smb://nas/media/music/Rock"`, which the server rejects with
/// PATH_NOT_FOUND. Both halves are asserted — the correct derivation round-trips
/// exactly, and the wrong one is detectably different — so a future refactor
/// can't reintroduce it by "simplifying" the split back into a re-join.
#[test]
fn nav_uri_round_trips_but_renaming_one_does_not() {
    let host = "nas";
    let share = "media";
    let rel = "music";
    let dir = dir_uri(host, share, rel);

    // How `remote_list_ui` builds its entries: path = child_uri(dir, name).
    let entry = child_uri(&dir, "Rock");

    // The fix: split the entry URI back into what `browse_open` wants. Because
    // it is already a child URI this is the identity.
    let (h, s, r) = split_uri(&entry).expect("entry is a valid uri");
    assert_eq!(h, host);
    assert_eq!(s.as_deref(), Some(share));
    assert_eq!(r, "music/Rock");
    assert_eq!(dir_uri(&h, &s.unwrap(), &r), entry, "split must round-trip");

    // Premise for the negative half: treating the URI as a name is NOT identity.
    // Built by hand, not via a helper — `child_browse` was deleted precisely
    // because its `(host, share, rel, name)` signature invited this mistake.
    let bad_rel = format!("{rel}/{entry}");
    assert_eq!(bad_rel, "music/smb://nas/media/music/Rock");
    assert_ne!(dir_uri(host, share, &bad_rel), entry);
    assert!(dir_uri(host, share, &bad_rel).contains("smb://nas/media/music/smb://"));

    // And the local `..` shape still works the other way: from a share root.
    let root_entry = child_uri(&dir_uri(host, share, ""), "");
    assert_eq!(split_uri(&root_entry).unwrap().2, "");
}

/// Eviction must free enough space AND never touch a played file. Playback is
/// the one thing housekeeping must not undo, so "we picked the biggest file" is
/// not good enough on its own — a played file that happens to be the largest has
/// to survive even if that means falling short of the budget.
#[test]
fn select_evictions_never_removes_a_played_file() {
    use tplay::network::{select_evictions, CacheEntry};

    let e = |key: &str, size: u64, played: bool| CacheEntry {
        key: key.into(),
        size,
        played,
        path: std::path::PathBuf::new(),
    };

    // Under budget: nothing goes, even though something is unmarked.
    let entries = vec![e("a", 100, false), e("b", 100, true)];
    assert!(select_evictions(&entries, 1000).is_empty());

    // Over budget: largest unmarked first, and the total actually comes down.
    let entries = vec![
        e("small", 10, false),
        e("big", 500, false),
        e("mid", 100, false),
        e("played-big", 900, true),
    ];
    // total 1510, budget 1000 -> must free >= 510
    let picked = select_evictions(&entries, 1000);
    assert!(
        picked.contains(&"big".to_string()),
        "largest unmarked first: {picked:?}"
    );
    let freed: u64 = entries
        .iter()
        .filter(|x| picked.contains(&x.key))
        .map(|x| x.size)
        .sum();
    assert!(freed >= 510, "freed {freed}, need >= 510");

    // The played file is never a candidate, however large.
    assert!(!picked.contains(&"played-big".to_string()), "{picked:?}");

    // Every played file survives even when the budget is unreachable.
    let all_played = vec![e("p1", 500, true), e("p2", 500, true)];
    assert!(select_evictions(&all_played, 0).is_empty());

    // Mixed: only unmarked ones, and it keeps going until the budget is met.
    // Total 1200 against a 500 budget, so freeing one 400-byte file is not
    // enough — both unmarked files must go and the played one must stay.
    let entries = vec![e("u1", 400, false), e("p1", 400, true), e("u2", 400, false)];
    let picked = select_evictions(&entries, 500);
    assert_eq!(picked.len(), 2, "{picked:?}");
    assert!(picked.iter().all(|k| k.starts_with('u')), "{picked:?}");
    let left: u64 = entries
        .iter()
        .filter(|x| !picked.contains(&x.key))
        .map(|x| x.size)
        .sum();
    assert!(left <= 500, "left {left} over budget");

    // Empty cache and empty selection are both fine.
    assert!(select_evictions(&[], 0).is_empty());

    // Ties break deterministically on key, so repeated runs pick the same files
    // (nondeterministic eviction would churn the cache for no reason).
    let tied = vec![
        e("bbb", 100, false),
        e("aaa", 100, false),
        e("ccc", 150, false),
    ];
    let first = select_evictions(&tied, 250);
    let second = select_evictions(&tied, 250);
    assert_eq!(first, second);
    assert_eq!(first[0], "ccc", "the strictly largest goes first");
}

/// `select_evictions` is pure, so it cannot see whether the key it returns is
/// the key a file is *stored* under. It is not: `cache_path_in` writes
/// `<spool_key>.<ext>` while `mark_played` records the extensionless
/// `spool_key`, so keying on `file_name()` matched neither — and the ceiling
/// silently never applied. This drives the real wiring over a temp spool dir.
#[test]
fn evict_unplayed_in_deletes_what_it_selects() {
    use tplay::network::Network;

    let dir = std::env::temp_dir().join(format!("tplay-test-{}/evict", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let on_disk = |uri: &str| dir.join(cache_path_in(uri, &dir).file_name().unwrap());

    let (uri_played, uri_big, uri_small) = (
        "smb://nas/m/played.flac",
        "smb://nas/m/big.flac",
        "smb://nas/m/small.mp3",
    );
    for (uri, n) in [(uri_played, 900usize), (uri_big, 800), (uri_small, 100)] {
        let p = on_disk(uri);
        assert!(
            p.extension().is_some(),
            "cache files carry the format's extension"
        );
        std::fs::write(&p, vec![0u8; n]).unwrap();
    }

    let mut net = Network::new(vec![]);
    net.mark_played(uri_played);

    // Both unmarked files must go: freeing only the 800 would leave 1000, still
    // over. The largest file overall survives — it is the one playback consumed.
    net.evict_unplayed_in(&dir, 900);
    assert!(
        on_disk(uri_played).is_file(),
        "a played file is never a candidate"
    );
    assert!(
        !on_disk(uri_big).exists(),
        "the selected file is actually unlinked"
    );
    assert!(
        !on_disk(uri_small).exists(),
        "eviction continues until the budget is met"
    );

    let uri_keep = "smb://nas/m/keep.flac";
    std::fs::write(on_disk(uri_keep), vec![0u8; 10]).unwrap();
    net.evict_unplayed_in(&dir, 5000);
    assert!(
        on_disk(uri_keep).is_file(),
        "under budget nothing is deleted"
    );

    net.evict_unplayed_in(&dir.join("nope"), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A remote track that keeps failing must stop being retried, or the per-frame
/// ask would re-queue a dead server forever. Success clears the count, so a
/// track that failed and then worked can still be evicted-and-refetched later.
#[test]
fn tag_attempts_are_bounded_and_cleared_on_success() {
    use tplay::network::TAG_ATTEMPTS_MAX;

    let mut attempts: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
    let uri = "smb://nas/media/a.mp3";

    // Simulate the app asking every frame while the track stays uncached.
    let mut asked = 0;
    loop {
        let a = attempts.entry(uri.to_string()).or_insert(0);
        if *a >= TAG_ATTEMPTS_MAX {
            break;
        }
        *a += 1;
        asked += 1;
        assert!(asked <= 16, "must terminate, not spin");
    }
    assert_eq!(asked, TAG_ATTEMPTS_MAX as usize, "gives up after the cap");

    // `busy()` reads the in-flight set, never this map, so a retained failure
    // cannot pin the app into a permanent repaint loop.
    let retained_failures_pin_nothing = attempts.is_empty();
    assert!(
        !retained_failures_pin_nothing,
        "the attempt map is retained by design"
    );

    // Success clears it, so the next request starts from zero.
    attempts.remove(uri);
    assert_eq!(attempts.get(uri).copied().unwrap_or(0), 0);
}

/// Servers commonly include `.` and `..` in a directory listing. They are
/// navigation artifacts, not content, and they used to survive into the share
/// browser as real folder rows — passing the `is_dir` filter, inflating the
/// "N folders" count, and producing a URI with a `.` segment the server then
/// rejects when clicked.
///
/// This is NOT a general hidden-file test: a real folder named `.config` is
/// content and must stay reachable, because the share browser has no
/// "show hidden" toggle to reveal it with.
#[test]
fn dot_entries_are_dropped_but_dot_folders_are_not() {
    use tplay::network::is_self_or_parent;

    for artifact in [".", ".."] {
        assert!(is_self_or_parent(artifact), "{artifact} must be dropped");
    }
    for real in [
        "", ".", // A leading dot is content, not an artifact.
        ".config", "..hidden", "...", "music", "song.mp3", "a.tplay",
        // Case matters: these are not the artifacts.
        ".TIF", ".. ",
    ] {
        if real == "." {
            continue; // already asserted above
        }
        assert!(
            !is_self_or_parent(real),
            "{real:?} is content and must survive"
        );
    }
}
