//! Hermetic pure-logic tests for the SMB URI/spool helpers in `tplay::network`.
//! No network, no worker thread — the smb2 client itself is untestable
//! headless, so the pure helpers carry the load (same convention as
//! `mounted_volumes.rs` fixtures).

use std::path::Path;
use tplay::network::{
    cache_path_in, child_uri, dir_uri, fnv1a64, is_remote, parse_server_input,
    server_uri, share_uri, spool_key, split_uri,
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
    assert_eq!(split_uri("smb://192.168.1.50"), Some(("192.168.1.50".into(), None, String::new())));
    // Share root.
    assert_eq!(split_uri("smb://nas/music"), Some(("nas".into(), Some("music".into()), String::new())));
    // Directory + file.
    assert_eq!(
        split_uri("smb://nas/music/Album/track.flac"),
        Some(("nas".into(), Some("music".into()), "Album/track.flac".into()))
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
    let row = net.servers().iter().find(|s| s.host == "192.168.15.59").unwrap();
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
    assert_eq!(split_uri(&uri), Some((host.into(), Some(share.into()), rel.into())));

    // Share root (empty rel) has no trailing slash.
    assert_eq!(dir_uri(host, share, ""), "smb://nas/music");
    assert_eq!(share_uri(host, share), "smb://nas/music");
    assert_eq!(server_uri(host), "smb://nas");

    // Descending appends exactly one slash regardless of parent form.
    assert_eq!(child_uri("smb://nas/music", "Album"), "smb://nas/music/Album");
    assert_eq!(child_uri("smb://nas/music/Album/", "a.flac"), "smb://nas/music/Album/a.flac");
}

#[test]
fn uri_parent_is_the_inverse_of_child_uri() {
    use tplay::network::uri_parent;

    // The base a remote playlist's relative entries resolve against.
    assert_eq!(uri_parent("smb://nas/share/dir/list.tplay"), "smb://nas/share/dir");
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
        assert_eq!(child_uri(uri_parent(uri), name), uri, "round-trip failed for {uri}");
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
