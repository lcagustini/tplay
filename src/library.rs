//! Library — filesystem browsing with per-file audio tags.
//!
//! Pure logic, no UI: directory listing, tag/duration reading, and the
//! background scan thread. `TrackInfo` values are cached in `TPlayApp`, so
//! revisiting a folder is instant.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::ItemKey;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// The audio extensions this player can play (mirrors the file dialog filter).
const AUDIO_EXTENSIONS: [&str; 5] = ["mp3", "wav", "ogg", "flac", "m4a"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("tplay"))
}

/// Playlist file format (shared for read/write).
#[derive(Serialize, Deserialize)]
struct PlaylistData { paths: Vec<String> }

/// Write the given tracks as a `.tplay` playlist file (paths only).
pub fn write_playlist(path: &Path, tracks: &[PathBuf]) -> std::io::Result<()> {
    let data = PlaylistData {
        paths: tracks.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect(),
    };
    let json = serde_json::to_string_pretty(&data)?;
    std::fs::write(path, json)
}

/// Read a `.tplay` playlist file. Returns None if not parseable.
/// Relative paths resolve against the playlist's own directory.
pub fn read_playlist(path: &Path) -> Option<Vec<PathBuf>> {
    let json = std::fs::read_to_string(path).ok()?;
    let data: PlaylistData = serde_json::from_str(&json).ok()?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    Some(
        data.paths
            .into_iter()
            .map(|s| { let p = PathBuf::from(s); if p.is_relative() { dir.join(p) } else { p } })
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
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Entry {
    pub fn path(&self) -> &Path { &self.path }
    pub fn is_dir(&self) -> bool { self.is_dir }
}

/// Read tags + duration for one file. `None` only when the file isn't a
/// parseable audio file at all.
pub fn read_info(path: &Path) -> Option<TrackInfo> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let get = |k: ItemKey| tag.and_then(|t| t.get_string(k)).map(str::to_owned).unwrap_or_default();
    let get_opt = |k: ItemKey| tag.and_then(|t| t.get_string(k)).map(str::to_owned);
    Some(TrackInfo {
        title: get(ItemKey::TrackTitle),
        artist: get(ItemKey::TrackArtist),
        album: get(ItemKey::AlbumTitle),
        genre: get(ItemKey::Genre),
        year: get_opt(ItemKey::Year),
        track_no: get_opt(ItemKey::TrackNumber),
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
    // No sorting here - navigate_to will sort via apply_library_sort
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

/// Tagged title, falling back to the file stem — matches what rows display.
pub fn title_or_stem(path: &Path, info: Option<&TrackInfo>) -> String {
    info.and_then(|i| (!i.title.is_empty()).then_some(i.title.clone()))
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string())
}

/// Sort key for a library entry and column. Returns a string that sorts correctly:
/// - Empty/missing values get a prefix that sorts after normal content.
/// - Durations become zero-padded milliseconds.
pub fn sort_key(e: &Entry, info: Option<&TrackInfo>, col: usize) -> String {
    let s: String = match col {
        0 => {
            // Title: folder name for dirs, tagged title or file stem for files
            if e.is_dir {
                e.path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string()
            } else {
                title_or_stem(e.path(), info).to_string()
            }
        }
        1 => {
            // Artist
            if e.is_dir { String::new() } else { info.map(|i| i.artist.as_str()).unwrap_or_default().to_string() }
        }
        2 => {
            // Album
            if e.is_dir { String::new() } else { info.map(|i| i.album.as_str()).unwrap_or_default().to_string() }
        }
        3 => {
            // Year
            if e.is_dir { String::new() } else { info.and_then(|i| i.year.as_deref()).unwrap_or_default().to_string() }
        }
        4 => {
            // Genre
            if e.is_dir { String::new() } else { info.map(|i| i.genre.as_str()).unwrap_or_default().to_string() }
        }
        5 => {
            // Duration: zero-padded milliseconds, or ~ for missing (sorts last)
            if e.is_dir {
                "~".to_string()
            } else if let Some(d) = info.and_then(|i| i.duration) {
                format!("{:012}", d.as_millis())
            } else {
                "~".to_string()
            }
        }
        _ => e.path.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string(),
    };
    // Prefix empty strings with \x7f (DEL) so they sort after normal content
    if s.is_empty() { format!("\x7f{}", s) } else { s.to_lowercase() }
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
    fn playlist_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("tplay-pl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("chill.tplay");
        assert!(is_playlist(&file));
        assert!(!is_playlist(&dir.join("chill.mp3")));

        let tracks = vec![dir.join("a.mp3"), PathBuf::from("sub/b.ogg")];
        assert!(write_playlist(&file, &tracks).is_ok());
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