//! Library logic tests — directory listing, tag reading, sorting, scan thread.

use tplay::library::*;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{ItemKey, Tag, TagType};
#[path = "common.rs"]
mod common;
use crate::common::{test_dir, write_tagged_mp3, write_wav};

/// `library_scanning()` reads as "any row missing from the tag cache", so a file
/// the scan drops pins `Scanning…` on screen for the whole session. A
/// permanently unparseable file is what used to do it: lofty fails, nothing is
/// sent, and nothing ever clears the label. One result per file asked about is
/// the invariant that prevents it.
#[test]
fn the_scan_reports_a_result_for_every_file_asked_about() {
    let dir = test_dir("scan_reports_all");
    let good = dir.join("good.wav");
    write_wav(&good);
    // Named so nothing upstream would filter it as non-audio, but not audio.
    let bad = dir.join("bad.mp3");
    fs::write(&bad, b"not audio at all").unwrap();
    assert!(read_info(&bad).is_none(), "premise: lofty cannot read this one");

    let (tx, rx) = mpsc::channel();
    let asked = vec![good, bad.clone()];
    scan_files(asked.clone(), tx);

    let got: Vec<PathBuf> = rx.into_iter().map(|(p, _)| p).collect();
    assert_eq!(got.len(), asked.len(), "a file was dropped, so its row never caches");
    for p in &asked {
        assert!(got.contains(p), "{} never reported", p.display());
    }

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn read_info_reads_tags_written_by_lofty() {
    let dir = test_dir("read_info_reads_tags_written_by_lofty");
    let path = dir.join("test.wav");
    write_wav(&path);

    // Write tags into the WAV via lofty itself, then parse them back through
    // our reader — exercises the real tag path, not just blank fields.
    let mut tagged = lofty::read_from_path(&path).unwrap();
    let mut tag = Tag::new(TagType::RiffInfo);
    tag.insert_text(ItemKey::TrackTitle, "Test Title".to_string());
    tag.insert_text(ItemKey::TrackArtist, "Test Artist".to_string());
    tag.insert_text(ItemKey::AlbumTitle, "Test Album".to_string());
    tag.insert_text(ItemKey::TrackNumber, "3".to_string());
    tagged.insert_tag(tag);
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();

    let info = read_info(&path).expect("read_info");
    assert_eq!(info.title, "Test Title");
    assert_eq!(info.artist, "Test Artist");
    assert_eq!(info.album, "Test Album");
    // The Album sort's second half — read as the raw tag string, parsed there.
    assert_eq!(info.track_no.as_deref(), Some("3"));
    assert_eq!(info.duration, Some(Duration::from_secs(1)));

    // Untagged files give blank fields, not failure.
    let plain = dir.join("plain.wav");
    write_wav(&plain);
    let info = read_info(&plain).unwrap();
    assert!(info.title.is_empty());
    assert_eq!(info.duration, Some(Duration::from_secs(1)));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cmp_entries_sorts_by_tag_fields() {
    let a = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let b = Entry { path: PathBuf::from("b.wav"), is_dir: false };
    let dir = Entry { path: PathBuf::from("z-folder"), is_dir: true };
    // Tags deliberately cross the filename order (a.wav < b.wav) so each
    // key proves it sorts by tags, not names.
    let info_a = TrackInfo {
        title: "Alpha".into(),
        artist: "Zed".into(),
        album: "Zeta".into(),
        track_no: Some("2".into()),
        duration: Some(Duration::from_secs(300)),
        ..Default::default()
    };
    let info_b = TrackInfo {
        title: "Bravo".into(),
        artist: "Amy".into(),
        album: "Alpha".into(),
        track_no: Some("1".into()),
        duration: None,
        ..Default::default()
    };

    // Title: Alpha (b.wav) sorts before Bravo (a.wav) — tags beat names.
    assert!(sort_key(&b, Some(&info_a), 0) < sort_key(&a, Some(&info_b), 0));
    // Artist / Album.
    assert!(sort_key(&a, Some(&info_b), 1) < sort_key(&b, Some(&info_a), 1)); // Amy < Zed
    assert!(sort_key(&a, Some(&info_b), 2) < sort_key(&b, Some(&info_a), 2)); // Alpha < Zeta
    // Duration: present sorts before missing.
    assert!(sort_key(&b, Some(&info_a), 3) < sort_key(&a, Some(&info_b), 3));
    // Missing tags sort last (empty artist after "Zed", not before).
    assert!(sort_key(&b, Some(&info_a), 1) < sort_key(&a, None, 1));
    // Folders are untagged: their name is the Title key, and they sort
    // last on the tag columns (no artist / no duration).
    assert!(sort_key(&dir, None, 0) > sort_key(&a, Some(&info_b), 0)); // "z-folder" > "Bravo"
    assert!(sort_key(&a, Some(&info_b), 1) < sort_key(&dir, None, 1)); // Amy < (no artist)
    assert!(sort_key(&a, Some(&info_a), 3) < sort_key(&dir, None, 3)); // present < missing
}

/// The Album column is `album \x01 track`: an album's tracks list in track
/// order, not alphabetically, and the numbering is numeric rather than textual
/// (so track 2 precedes track 10, which a plain string pad is what buys).
#[test]
fn sort_key_album_orders_tracks_within_an_album() {
    let a = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let b = Entry { path: PathBuf::from("b.wav"), is_dir: false };
    let c = Entry { path: PathBuf::from("c.wav"), is_dir: false };
    let two = TrackInfo { album: "Set".into(), track_no: Some("2".into()), title: "Zebra".into(), ..Default::default() };
    let ten = TrackInfo { album: "Set".into(), track_no: Some("10".into()), title: "Aardvark".into(), ..Default::default() };
    let unnumbered = TrackInfo { album: "Set".into(), title: "Bonus".into(), ..Default::default() };
    let other = TrackInfo { album: "Tangent".into(), track_no: Some("1".into()), title: "First".into(), ..Default::default() };

    // 2 before 10, even though "Aardvark" < "Zebra" alphabetically.
    assert!(sort_key(&a, Some(&two), 2) < sort_key(&b, Some(&ten), 2));
    // Unnumbered tracks sort after the numbered ones, still inside the album.
    assert!(sort_key(&b, Some(&ten), 2) < sort_key(&c, Some(&unnumbered), 2));
    // Album still groups first: everything in "Set" precedes "Tangent".
    assert!(sort_key(&c, Some(&unnumbered), 2) < sort_key(&b, Some(&other), 2));
}

/// An untagged file must not sort above every album, and must not jump ahead of
/// a folder either — both belong in the "no value" sink region.
#[test]
fn sort_key_album_untagged_sinks_with_folders() {
    let file = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let dir = Entry { path: PathBuf::from("z-folder"), is_dir: true };
    let untagged = TrackInfo { title: "Plain".into(), ..Default::default() };
    let album = TrackInfo { album: "Set".into(), track_no: Some("1".into()), ..Default::default() };
    // Last: no album, so behind every tagged file.
    assert!(sort_key(&file, Some(&untagged), 2) > sort_key(&file, Some(&album), 2));
    // Same sink region as a folder, not in front of it.
    assert!(sort_key(&file, Some(&untagged), 2) > sort_key(&dir, None, 2));
}

#[test]
fn is_audio_filters_extensions() {
    assert!(is_audio(PathBuf::new().join("track.mp3").as_path()));
    assert!(is_audio(PathBuf::new().join("track.WAV").as_path()));
    assert!(is_audio(PathBuf::new().join("track.ogg").as_path()));
    assert!(is_audio(PathBuf::new().join("track.flac").as_path()));
    assert!(is_audio(PathBuf::new().join("track.m4a").as_path()));
    assert!(!is_audio(PathBuf::new().join("track.txt").as_path()));
    assert!(!is_audio(PathBuf::new().join("track").as_path()));
}

#[test]
fn is_playlist_detects_tplay_extension() {
    assert!(is_playlist(PathBuf::new().join("list.tplay").as_path()));
    assert!(is_playlist(PathBuf::new().join("list.TPLAY").as_path()));
    assert!(!is_playlist(PathBuf::new().join("list.txt").as_path()));
    assert!(!is_playlist(PathBuf::new().join("list.mp3").as_path()));
}

#[test]
fn list_dir_separates_dirs_and_files() {
    let dir = test_dir("list_dir_separates_dirs_and_files");
    fs::create_dir_all(dir.join("zzz")).unwrap();
    fs::create_dir_all(dir.join("AAA")).unwrap();
    fs::create_dir_all(dir.join(".hidden")).unwrap();
    write_wav(&dir.join("track1.wav"));
    write_wav(&dir.join("track2.mp3"));
    fs::write(dir.join("notes.txt"), "x").unwrap();

    let (dirs, files) = list_dir(&dir, false);
    assert_eq!(dirs.len(), 2);
    assert_eq!(files.len(), 2);
    // list_dir does not sort; it returns filesystem order.
    // Just verify the correct entries are present.
    let dir_names: std::collections::HashSet<_> = dirs.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
    assert!(dir_names.contains("zzz"));
    assert!(dir_names.contains("AAA"));
    assert!(!dir_names.contains(".hidden")); // hidden by default

    let file_names: std::collections::HashSet<_> = files.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
    assert!(file_names.contains("track1.wav"));
    assert!(file_names.contains("track2.mp3"));
    assert!(!file_names.contains("notes.txt")); // not audio

    let (dirs_hidden, _) = list_dir(&dir, true);
    assert_eq!(dirs_hidden.len(), 3);
    let hidden_names: std::collections::HashSet<_> = dirs_hidden.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
    assert!(hidden_names.contains(".hidden"));
    assert!(hidden_names.contains("zzz"));
    assert!(hidden_names.contains("AAA"));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn read_info_extracts_tags_and_duration() {
    let dir = test_dir("read_info_extracts_tags_and_duration");
    let path = dir.join("test.wav");
    write_wav(&path);

    // Test that read_info extracts duration from a basic WAV file
    // (Tag writing via lofty requires specific RIFF chunk structure that our minimal WAV lacks)
    let info = read_info(&path).expect("read_info");
    assert_eq!(info.duration, Some(Duration::from_secs(1)));
    assert!(info.title.is_empty()); // no tags in minimal WAV

    // Untagged file gives blank fields
    let plain = dir.join("plain.wav");
    write_wav(&plain);
    let info = read_info(&plain).unwrap();
    assert!(info.title.is_empty());
    assert_eq!(info.duration, Some(Duration::from_secs(1)));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn read_info_returns_none_for_non_audio() {
    let dir = test_dir("read_info_returns_none_for_non_audio");
    let path = dir.join("not_audio.txt");
    fs::write(&path, "hello").unwrap();
    assert!(read_info(&path).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

/// The remote tag path is now "spool the file, then `read_info` the copy", so
/// the load-bearing claim is that a **full** read yields both tags and a real
/// duration. (The prefix-read version this replaced could not: lofty derives
/// duration from the bytes it is handed, so a truncated read reported a
/// duration proportional to the prefix.)
#[test]
fn read_info_gives_tags_and_a_true_duration_from_a_whole_file() {
    let dir = test_dir("read_info_gives_tags_and_a_true_duration_from_a_whole_file");
    let path = dir.join("tagged.mp3");
    let bytes = write_tagged_mp3(&path, "Ne-Yo", "Test Artist", "Test Album");

    let info = read_info(&path).expect("whole file parses");
    assert_eq!(info.title, "Ne-Yo");
    assert_eq!(info.artist, "Test Artist");
    assert_eq!(info.album, "Test Album");

    // 300 frames of 1152 samples at 44.1 kHz ≈ 7.84 s. The point is that it is
    // the FILE's length, not a function of how much was read: a prefix of any
    // size would have produced a smaller number.
    let secs = info.duration.expect("whole file has a duration").as_secs_f64();
    assert!(
        (secs - 7.8).abs() < 0.5,
        "duration {secs:.2}s should match the file's 300 frames (~7.8s), not a prefix"
    );
    assert!(bytes.len() > 100_000, "premise: the fixture is much larger than any prefix");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sort_entries_orders_both_sources_the_same_way() {
    // One sorter for the local folder list and the share list. The share case is
    // the interesting one: `path` is an `smb://` URI, and its `file_name` /
    // `file_stem` must still yield the remote name so untitled rows and folders
    // sort where a local folder's would.
    let mut cache: HashMap<PathBuf, TrackInfo> = HashMap::new();
    let tagged = |t: &str, a: &str| TrackInfo {
        title: t.into(),
        artist: a.into(),
        ..Default::default()
    };
    let z = Entry { path: PathBuf::from("z.mp3"), is_dir: false };
    let a = Entry { path: PathBuf::from("a.mp3"), is_dir: false };
    let folder = Entry { path: PathBuf::from("mid"), is_dir: true };
    cache.insert(z.path.clone(), tagged("Alpha", "Zed"));
    cache.insert(a.path.clone(), tagged("Zulu", "Abe"));

    let mut local = vec![z.clone(), a.clone(), folder.clone()];
    sort_entries(&mut local, &cache, 0, true);
    let titles: Vec<String> = local
        .iter()
        .map(|e| title_or_stem(e.path(), cache.get(e.path())))
        .collect();
    // Title sorts the folder by its OWN name, so it interleaves rather than
    // sinking — "mid" belongs between "Alpha" and "Zulu".
    assert_eq!(titles, vec!["Alpha", "mid", "Zulu"]);

    // Descending is a reverse of the ascending order, so ties don't reshuffle.
    sort_entries(&mut local, &cache, 0, false);
    let rev: Vec<String> = local
        .iter()
        .map(|e| title_or_stem(e.path(), cache.get(e.path())))
        .collect();
    assert_eq!(rev, vec!["Zulu", "mid", "Alpha"]);

    // Artist column: the folder sinks below both files, same as locally.
    sort_entries(&mut local, &cache, 1, true);
    assert_eq!(local[0].path, a.path);
    assert_eq!(local[1].path, z.path);
    assert!(local[2].is_dir, "folder sinks last on a tag column");

    // The same three rows as smb:// URIs sort identically — the URI is just a
    // path whose last segment is the filename.
    let mut remote: Vec<Entry> = vec![
        Entry { path: PathBuf::from("smb://nas/music/z.mp3"), is_dir: false },
        Entry { path: PathBuf::from("smb://nas/music/a.mp3"), is_dir: false },
        Entry { path: PathBuf::from("smb://nas/music/mid"), is_dir: true },
    ];
    let mut rcache: HashMap<PathBuf, TrackInfo> = HashMap::new();
    rcache.insert(remote[0].path.clone(), tagged("Alpha", "Zed"));
    rcache.insert(remote[1].path.clone(), tagged("Zulu", "Abe"));
    sort_entries(&mut remote, &rcache, 0, true);
    let rnames: Vec<String> = remote
        .iter()
        .map(|e| e.path().file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(rnames, vec!["z.mp3", "mid", "a.mp3"], "titles Alpha, folder, Zulu");
}

#[test]
fn sort_key_title_prefers_tag_over_filename() {
    let a = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let b = Entry { path: PathBuf::from("b.wav"), is_dir: false };
    let info_a = TrackInfo { title: "Zebra".into(), ..Default::default() };
    let info_b = TrackInfo { title: "Alpha".into(), ..Default::default() };
    // b.wav has title "Alpha", a.wav has title "Zebra" -> b sorts before a
    assert!(sort_key(&b, Some(&info_b), 0) < sort_key(&a, Some(&info_a), 0));
}

#[test]
fn sort_key_tag_columns_sink_missing_values() {
    let a = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let info = TrackInfo { artist: "Zed".into(), ..Default::default() };
    // Track with artist "Zed" sorts before track with no artist
    assert!(sort_key(&a, Some(&info), 1) < sort_key(&a, None, 1));
}

#[test]
fn sort_key_folders_untagged() {
    let dir = Entry { path: PathBuf::from("z-folder"), is_dir: true };
    let file = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let info = TrackInfo { title: "Alpha".into(), artist: "Amy".into(), duration: Some(Duration::from_secs(100)), ..Default::default() };

    // Title: folder name "z-folder" > "Alpha"
    assert!(sort_key(&file, Some(&info), 0) < sort_key(&dir, None, 0));
    // Artist: file has "Amy", folder has none -> file first
    assert!(sort_key(&file, Some(&info), 1) < sort_key(&dir, None, 1));
    // Duration: file has duration, folder has none -> file first
    assert!(sort_key(&file, Some(&info), 3) < sort_key(&dir, None, 3));
}

#[test]
fn sort_key_duration_missing_sorts_last() {
    let a = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let b = Entry { path: PathBuf::from("b.wav"), is_dir: false };
    let info_a = TrackInfo { duration: Some(Duration::from_secs(100)), ..Default::default() };
    let info_b = TrackInfo { duration: None, ..Default::default() };
    assert!(sort_key(&a, Some(&info_a), 3) < sort_key(&b, Some(&info_b), 3));
}

#[test]
fn title_or_stem_fallbacks() {
    let path = PathBuf::from("song.mp3");
    let info = TrackInfo { title: "Tagged Title".into(), ..Default::default() };
    assert_eq!(title_or_stem(&path, Some(&info)), "Tagged Title");

    let info_empty = TrackInfo { title: "".into(), ..Default::default() };
    assert_eq!(title_or_stem(&path, Some(&info_empty)), "song");

    assert_eq!(title_or_stem(&path, None), "song");
}

#[test]
fn scan_files_sends_results_and_stops_on_drop() {
    let dir = test_dir("scan_files_sends_results_and_stops_on_drop");
    let f1 = dir.join("a.wav");
    let f2 = dir.join("b.wav");
    write_wav(&f1);
    write_wav(&f2);

    let (tx, rx) = mpsc::channel();
    let files = vec![f1.clone(), f2.clone()];
    thread::spawn(move || scan_files(files, tx));

    let mut results = Vec::new();
    for _ in 0..2 {
        results.push(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }
    assert_eq!(results.len(), 2);

    // Drop receiver, next scan should exit early
    let (tx2, rx2) = mpsc::channel();
    let files2 = vec![f1, f2];
    thread::spawn(move || scan_files(files2, tx2));
    drop(rx2);
    // Thread should exit cleanly (no panic)
    thread::sleep(Duration::from_millis(50));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sort_options_constant_has_expected_columns() {
    assert_eq!(SORT_OPTIONS, ["Title", "Artist", "Album", "Duration"]);
}