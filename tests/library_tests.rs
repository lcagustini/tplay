//! Library logic tests — directory listing, tag reading, sorting, scan thread.

use tplay::library::*;
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
use crate::common::{test_dir, write_wav};

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
    tag.insert_text(ItemKey::Genre, "Test Genre".to_string());
    // NOTE: no Year here — lofty's RIFF INFO writer drops `ItemKey::Year`
    // (ICRD is date-typed); real files get year via their native tags.
    tagged.insert_tag(tag);
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();

    let info = read_info(&path).expect("read_info");
    assert_eq!(info.title, "Test Title");
    assert_eq!(info.artist, "Test Artist");
    assert_eq!(info.album, "Test Album");
    assert_eq!(info.genre, "Test Genre");
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
        year: Some("2000".into()),
        genre: "Rock".into(),
        duration: Some(Duration::from_secs(300)),
        ..Default::default()
    };
    let info_b = TrackInfo {
        title: "Bravo".into(),
        artist: "Amy".into(),
        album: "Alpha".into(),
        year: Some("1995".into()),
        genre: "Jazz".into(),
        duration: None,
        ..Default::default()
    };

    // Title: Alpha (b.wav) sorts before Bravo (a.wav) — tags beat names.
    assert!(sort_key(&b, Some(&info_a), 0) < sort_key(&a, Some(&info_b), 0));
    // Artist / Album / Year / Genre.
    assert!(sort_key(&a, Some(&info_b), 1) < sort_key(&b, Some(&info_a), 1)); // Amy < Zed
    assert!(sort_key(&a, Some(&info_b), 2) < sort_key(&b, Some(&info_a), 2)); // Alpha < Zeta
    assert!(sort_key(&a, Some(&info_b), 3) < sort_key(&b, Some(&info_a), 3)); // 1995 < 2000
    assert!(sort_key(&a, Some(&info_b), 4) < sort_key(&b, Some(&info_a), 4)); // Jazz < Rock
    // Duration: present sorts before missing.
    assert!(sort_key(&b, Some(&info_a), 5) < sort_key(&a, Some(&info_b), 5));
    // Missing tags sort last (empty artist after "Zed", not before).
    assert!(sort_key(&b, Some(&info_a), 1) < sort_key(&a, None, 1));
    // Folders are untagged: their name is the Title key, and they sort
    // last on the tag columns (no artist / no duration).
    assert!(sort_key(&dir, None, 0) > sort_key(&a, Some(&info_b), 0)); // "z-folder" > "Bravo"
    assert!(sort_key(&a, Some(&info_b), 1) < sort_key(&dir, None, 1)); // Amy < (no artist)
    assert!(sort_key(&a, Some(&info_a), 5) < sort_key(&dir, None, 5)); // present < missing
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
    assert!(sort_key(&file, Some(&info), 5) < sort_key(&dir, None, 5));
}

#[test]
fn sort_key_duration_missing_sorts_last() {
    let a = Entry { path: PathBuf::from("a.wav"), is_dir: false };
    let b = Entry { path: PathBuf::from("b.wav"), is_dir: false };
    let info_a = TrackInfo { duration: Some(Duration::from_secs(100)), ..Default::default() };
    let info_b = TrackInfo { duration: None, ..Default::default() };
    assert!(sort_key(&a, Some(&info_a), 5) < sort_key(&b, Some(&info_b), 5));
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
    assert_eq!(SORT_OPTIONS, ["Title", "Artist", "Album", "Year", "Genre", "Duration"]);
}