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

/// Cover art filenames to fall back to when a track has no embedded art.
/// `folder.jpg` is the classic album-folder convention, `cover.jpg` is common
/// from Linux rippers; PNG variants exist too. Checked in the track's own dir.
const COVER_FILES: [&str; 6] = [
    "folder.jpg", "Folder.jpg", "cover.jpg", "Cover.jpg", "folder.png", "cover.png",
];

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

/// Embedded album art for one file, falling back to a cover file beside it
/// (`folder.jpg`/`cover.jpg`/…, see `COVER_FILES`). Returns raw image bytes;
/// `read_info`'s sibling for the Album Cover pane. `None` when the file has
/// neither — the caller draws the themed placeholder.
pub fn read_cover(path: &Path) -> Option<Vec<u8>> {
    if let Ok(tagged) = lofty::read_from_path(path) {
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
        if let Some(pic) = tag.and_then(|t| t.pictures().first()) {
            return Some(pic.data().to_vec());
        }
    }
    let dir = path.parent()?;
    COVER_FILES
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read(p).ok())
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