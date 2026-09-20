//! Library — filesystem browsing with per-file audio tags.
//!
//! Pure logic, no UI: directory listing, tag/duration reading, and the
//! background scan thread. `TrackInfo` values are cached in `TPlayApp`, so
//! revisiting a folder is instant.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::ItemKey;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// The audio extensions this player can play (mirrors the file dialog filter).
const AUDIO_EXTENSIONS: [&str; 5] = ["mp3", "wav", "ogg", "flac", "m4a"];
/// Playlist files: JSON (`{"paths": [...]}`), saved/loaded from the Library
/// like any other file. Shuffle/repeat are appwide settings, not content.
const PLAYLIST_EXTENSIONS: [&str; 1] = ["tplay"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PLAYLIST_EXTENSIONS.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

/// Write the given tracks as a `.tplay` playlist file (paths only). Returns
/// false when the file couldn't be written or serialized.
pub fn write_playlist(path: &Path, tracks: &[PathBuf]) -> bool {
    #[derive(Serialize)]
    struct PlaylistData {
        paths: Vec<String>,
    }
    let data = PlaylistData {
        paths: tracks
            .iter()
            .filter_map(|p| p.to_str().map(str::to_owned))
            .collect(),
    };
    match serde_json::to_string_pretty(&data) {
        Ok(json) => std::fs::write(path, json).is_ok(),
        Err(_) => false,
    }
}

/// Read a `.tplay` playlist file. `None` = not a parseable playlist (callers
/// leave the current playlist alone); relative paths resolve against the
/// playlist's own directory, like real players. Existence filtering happens
/// in `TPlayApp::load_playlist_from`.
pub fn read_playlist(path: &Path) -> Option<Vec<PathBuf>> {
    let json = std::fs::read_to_string(path).ok()?;
    #[derive(Deserialize)]
    struct PlaylistData {
        paths: Vec<String>,
    }
    let data: PlaylistData = serde_json::from_str(&json).ok()?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    Some(
        data.paths
            .into_iter()
            .map(|s| {
                let p = PathBuf::from(s);
                if p.is_relative() { dir.join(p) } else { p }
            })
            .collect(),
    )
}

/// Tags + duration read from one audio file. Missing fields stay blank —
/// tag strings are fallible metadata, never a reason to fail the scan.
#[derive(Clone, Debug, Default)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub year: Option<String>,
    pub track_no: Option<String>,
    pub duration: Option<Duration>,
}

/// One row of the Library file list: a subfolder or an audio file, in a single
/// sortable list. Folders have no tags — sorting treats them as untagged
/// entries whose title is their name (the Title column interleaves them with
/// files; the tag columns sink them last).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Dir(PathBuf),
    File(PathBuf),
}

impl Entry {
    pub fn path(&self) -> &Path {
        match self {
            Entry::Dir(p) | Entry::File(p) => p,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, Entry::Dir(_))
    }
}

fn str_field(tag: &lofty::tag::Tag, key: ItemKey) -> String {
    tag.get_string(key).map(str::to_owned).unwrap_or_default()
}

/// Read tags + duration for one file. `None` only when the file isn't a
/// parseable audio file at all.
pub fn read_info(path: &Path) -> Option<TrackInfo> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    Some(TrackInfo {
        title: tag.map(|t| str_field(t, ItemKey::TrackTitle)).unwrap_or_default(),
        artist: tag.map(|t| str_field(t, ItemKey::TrackArtist)).unwrap_or_default(),
        album: tag.map(|t| str_field(t, ItemKey::AlbumTitle)).unwrap_or_default(),
        genre: tag.map(|t| str_field(t, ItemKey::Genre)).unwrap_or_default(),
        year: tag.and_then(|t| t.get_string(ItemKey::Year).map(str::to_owned)),
        track_no: tag.and_then(|t| t.get_string(ItemKey::TrackNumber).map(str::to_owned)),
        // lofty reports Duration::ZERO for streams with unknown length.
        duration: {
            let d = tagged.properties().duration();
            if d.is_zero() { crate::audio::probe_duration(path) } else { Some(d) }
        },
    })
}

/// Entries in a directory: subfolders first then audio files, each sorted
/// case-insensitively by file name. Dot-prefixed (hidden) subfolders are
/// skipped unless `show_hidden`.
pub fn list_dir(dir: &Path, show_hidden: bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let hidden = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'));
                if !hidden || show_hidden {
                    dirs.push(path);
                }
            } else if is_audio(&path) || is_playlist(&path) {
                files.push(path);
            }
        }
    }
    let by_name = |a: &PathBuf, b: &PathBuf| {
        a.file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.file_name().unwrap_or_default().to_ascii_lowercase())
    };
    dirs.sort_by(by_name);
    files.sort_by(by_name);
    (dirs, files)
}

/// Tag-read every file and send `(path, info)` results. Runs on a background
/// thread; stops early when the receiver is dropped (a new navigation).
pub fn scan_files(files: Vec<PathBuf>, tx: Sender<(PathBuf, TrackInfo)>) {
    for path in files {
        if let Some(info) = read_info(&path) {
            if tx.send((path, info)).is_err() {
                return;
            }
        }
    }
}

/// Sortable columns for the Library list, in header display order. Index 0
/// (Title) is the default order: untagged files fall back to their name and
/// folders to theirs, so it reads as the classic name sort everything mixes
/// into.
pub const SORT_OPTIONS: [&str; 6] = ["Title", "Artist", "Album", "Year", "Genre", "Duration"];

fn name_ord(a: &Path, b: &Path) -> Ordering {
    a.file_name()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .cmp(&b.file_name().unwrap_or_default().to_ascii_lowercase())
}

/// String ordering with empty/missing values sorting last in both directions.
fn str_ord(a: &str, b: &str) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// Missing durations sort last; present ones by length.
fn dur_ord(a: Option<Duration>, b: Option<Duration>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Tagged title, falling back to the file stem — matches what rows display.
fn title_or_stem<'a>(path: &'a Path, info: Option<&'a TrackInfo>) -> &'a str {
    info.and_then(|i| (!i.title.is_empty()).then_some(i.title.as_str()))
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or_default())
}

/// Sort value for the Title column: tagged title or file stem for files, the
/// folder's own name for dirs.
fn title_or_name<'a>(e: &'a Entry, info: Option<&'a TrackInfo>) -> &'a str {
    if let Entry::Dir(p) = e {
        return p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    }
    title_or_stem(e.path(), info)
}

/// Sort value for the tag columns: empty for folders (they're untagged).
fn field<'a>(e: &'a Entry, info: Option<&'a TrackInfo>, f: fn(&TrackInfo) -> &str) -> &'a str {
    if e.is_dir() {
        return "";
    }
    info.map(f).unwrap_or_default()
}

fn duration_of(e: &Entry, info: Option<&TrackInfo>) -> Option<Duration> {
    if e.is_dir() {
        None
    } else {
        info.and_then(|i| i.duration)
    }
}

/// Compare two Library rows by a header sort key (index into `SORT_OPTIONS`).
/// Unknown keys sort by file name; untagged fields sort last.
pub fn cmp_entries(
    a: &Entry,
    b: &Entry,
    key: usize,
    a_info: Option<&TrackInfo>,
    b_info: Option<&TrackInfo>,
) -> Ordering {
    match key {
        0 => str_ord(title_or_name(a, a_info), title_or_name(b, b_info)),
        1 => str_ord(
            field(a, a_info, |i| i.artist.as_str()),
            field(b, b_info, |i| i.artist.as_str()),
        ),
        2 => str_ord(
            field(a, a_info, |i| i.album.as_str()),
            field(b, b_info, |i| i.album.as_str()),
        ),
        3 => str_ord(
            field(a, a_info, |i| i.year.as_deref().unwrap_or_default()),
            field(b, b_info, |i| i.year.as_deref().unwrap_or_default()),
        ),
        4 => str_ord(
            field(a, a_info, |i| i.genre.as_str()),
            field(b, b_info, |i| i.genre.as_str()),
        ),
        5 => dur_ord(duration_of(a, a_info), duration_of(b, b_info)),
        _ => name_ord(a.path(), b.path()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;
    use lofty::tag::{Tag, TagType};

    /// Minimal, valid 8 kHz mono PCM WAV — exactly 1 second of audio.
    fn write_wav(path: &Path) {
        let rate = 8000u32;
        let n = 8000usize;
        let mut data = Vec::with_capacity(44 + n * 2);
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&(36u32 + (n as u32) * 2).to_le_bytes());
        data.extend_from_slice(b"WAVE");
        data.extend_from_slice(b"fmt ");
        data.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        data.extend_from_slice(&1u16.to_le_bytes()); // PCM
        data.extend_from_slice(&1u16.to_le_bytes()); // mono
        data.extend_from_slice(&rate.to_le_bytes());
        data.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
        data.extend_from_slice(&2u16.to_le_bytes()); // block align
        data.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        data.extend_from_slice(b"data");
        data.extend_from_slice(&((n as u32) * 2).to_le_bytes());
        for i in 0..n {
            data.extend_from_slice(&(i as i16).to_le_bytes());
        }
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn read_info_extracts_tags_and_duration() {
        let dir = std::env::temp_dir().join(format!("tplay-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.wav");
        write_wav(&path);

        // To exercise the tag path: write tags into the WAV via lofty itself,
        // then parse it back through our reader.
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

        // Extension filter + dirs-first case-insensitive sort; hidden dirs are
        // hidden by default and shown when toggled.
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.join("zzz")).unwrap();
        std::fs::create_dir_all(dir.join("AAA")).unwrap();
        std::fs::create_dir_all(dir.join(".hid")).unwrap();
        let (dirs, files) = list_dir(&dir, false);
        assert_eq!(dirs, vec![dir.join("AAA"), dir.join("zzz")]);
        assert_eq!(files, vec![plain, path]); // plain < test
        assert!(!is_audio(&dir.join("notes.txt")));
        let (dirs, _) = list_dir(&dir, true);
        // '.' sorts before 'a', so .hid leads.
        assert_eq!(dirs, vec![dir.join(".hid"), dir.join("AAA"), dir.join("zzz")]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmp_entries_sorts_by_tag_fields() {
        let a = Entry::File(PathBuf::from("a.wav"));
        let b = Entry::File(PathBuf::from("b.wav"));
        let dir = Entry::Dir(PathBuf::from("z-folder"));
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
        assert!(cmp_entries(&b, &a, 0, Some(&info_a), Some(&info_b)).is_lt());
        // Artist / Album / Year / Genre.
        assert!(cmp_entries(&a, &b, 1, Some(&info_b), Some(&info_a)).is_lt()); // Amy < Zed
        assert!(cmp_entries(&a, &b, 2, Some(&info_b), Some(&info_a)).is_lt()); // Alpha < Zeta
        assert!(cmp_entries(&a, &b, 3, Some(&info_b), Some(&info_a)).is_lt()); // 1995 < 2000
        assert!(cmp_entries(&a, &b, 4, Some(&info_b), Some(&info_a)).is_lt()); // Jazz < Rock
        // Duration: present sorts before missing.
        assert!(cmp_entries(&b, &a, 5, Some(&info_a), Some(&info_b)).is_lt());
        // Missing tags sort last (empty artist after "Zed", not before).
        assert!(cmp_entries(&b, &a, 1, Some(&info_a), None).is_lt());
        // Folders are untagged: their name is the Title key, and they sort
        // last on the tag columns (no artist / no duration).
        assert!(cmp_entries(&dir, &a, 0, None, Some(&info_b)).is_gt()); // "z-folder" > "Bravo"
        assert!(cmp_entries(&a, &dir, 1, Some(&info_b), None).is_lt()); // Amy < (no artist)
        assert!(cmp_entries(&a, &dir, 5, Some(&info_a), None).is_lt()); // present < missing
    }

    #[test]
    fn playlist_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("tplay-pl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("chill.tplay");
        assert!(is_playlist(&file));
        assert!(!is_playlist(&dir.join("chill.mp3")));

        let tracks = vec![dir.join("a.mp3"), PathBuf::from("sub/b.ogg")];
        assert!(write_playlist(&file, &tracks));
        let back = read_playlist(&file).expect("read playlist");
        // Absolute paths come back as-is; relative ones resolve against the
        // playlist's own directory.
        assert_eq!(back, vec![dir.join("a.mp3"), dir.join("sub/b.ogg")]);

        // Garbage files read as None — callers leave the current playlist
        // alone, never panic or wipe.
        std::fs::write(&file, "not json").unwrap();
        assert!(read_playlist(&file).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}