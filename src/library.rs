//! Library — filesystem browsing with per-file audio tags.
//!
//! Pure logic, no UI: directory listing, tag/duration reading, and the
//! background scan thread. `TrackInfo` values are cached in `TPlayApp`, so
//! revisiting a folder is instant.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::ItemKey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// Pseudo/device-tree filesystems to exclude from Volumes.
const PSEUDO_FSTYPES: &[&str] = &[
    "devtmpfs", "devpts", "sysfs", "proc", "tmpfs", "cgroup2", "configfs", "debugfs",
    "tracefs", "bpf", "mqueue", "hugetlbfs", "efivarfs", "securityfs", "pstore",
    "autofs", "rpc_pipefs", "nfsd", "fusectl",
];

/// Boot/ESP mountpoints to exclude — system partitions, not user volumes.
const BOOT_MOUNTPOINTS: &[&str] = &["/efi", "/boot", "/boot/efi"];

/// Build a device -> label map from /dev/disk/by-label/ symlinks.
/// Returns a map of device basename (e.g. "sdc1") -> label (e.g. "25-ssd-2").
fn read_disk_labels() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/dev/disk/by-label") {
        for entry in entries.flatten() {
            let label = entry.file_name().to_string_lossy().into_owned();
            if let Ok(target) = std::fs::read_link(entry.path()) {
                if let Some(dev_name) = target.file_name().and_then(|s| s.to_str()) {
                    map.insert(dev_name.to_string(), label);
                }
            }
        }
    }
    map
}

/// A mountpoint that represents a local volume (block partition).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Human-readable label (mountpoint leaf, or device basename for root).
    pub label: String,
    /// Mountpoint path.
    pub path: PathBuf,
}

impl Volume {
    /// Parse /proc/self/mounts and return local volumes (block partitions on /dev/*).
    pub fn parse_mounts(mounts: &str) -> Vec<Volume> {
        Self::parse_mounts_with_labels(mounts, &read_disk_labels())
    }

    /// Pure core of `parse_mounts` — label map injected, so tests are hermetic.
    pub fn parse_mounts_with_labels(mounts: &str, disk_labels: &std::collections::HashMap<String, String>) -> Vec<Volume> {
        let mut vols = Vec::new();
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            let device = parts[0];
            let mountpoint = parts[1];
            let fstype = parts[2];

            // Only /dev/* block devices, skip pseudo filesystems
            if !device.starts_with("/dev/") {
                continue;
            }
            if PSEUDO_FSTYPES.iter().any(|&p| fstype.starts_with(p)) {
                continue;
            }
            // Skip boot/ESP partitions — system partitions, not user volumes.
            if BOOT_MOUNTPOINTS.contains(&mountpoint) {
                continue;
            }

            let path = PathBuf::from(mountpoint);
            let dev_name = Path::new(device)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(device);
            let label = disk_labels
                .get(dev_name)
                .cloned()
                .unwrap_or_else(|| {
                    path.file_name()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .unwrap_or_else(|| dev_name.to_string())
                });

            vols.push(Volume { label, path });
        }
        // Sort by label, case-insensitive
        vols.sort_by_key(|a| a.label.to_lowercase());
        vols
    }

    /// Read /proc/self/mounts and return mounted local volumes.
    pub fn mounted_volumes() -> Vec<Volume> {
        std::fs::read_to_string("/proc/self/mounts")
            .ok()
            .as_deref()
            .map(Self::parse_mounts)
            .unwrap_or_default()
    }
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("tplay"))
}

/// Fallback filename for a playlist save when the tracked file has no usable
/// stem (a fresh playlist, or one whose name has no extension to strip).
pub const DEFAULT_PLAYLIST_NAME: &str = "playlist.tplay";

/// The name to carry over when re-saving: the file's stem. `None` when nothing
/// is usable — no file at all, or a dotfile like `.tplay`, whose "stem" is the
/// whole name (carrying that over would save `.tplay.tplay`).
fn playlist_stem(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.is_empty() || (stem.starts_with('.') && path.extension().is_none()) {
        return None;
    }
    Some(stem.to_owned())
}

/// Suggested filename when saving a playlist: the tracked file's stem, so
/// saving a loaded playlist somewhere new keeps its name. Falls back to
/// `DEFAULT_PLAYLIST_NAME` when there is no usable stem.
pub fn default_playlist_name(playlist_file: Option<&Path>) -> String {
    match playlist_file.and_then(playlist_stem) {
        Some(stem) => format!("{stem}.tplay"),
        None => DEFAULT_PLAYLIST_NAME.to_string(),
    }
}

/// Normalize a user-typed filename into a safe `.tplay` name: the extension is
/// implied if omitted, and path separators are stripped rather than turned into
/// a bogus server path. `None` for blank input.
pub fn playlist_file_name(typed: &str) -> Option<String> {
    let cleaned = typed.trim().replace(['/', '\\'], "_");
    if cleaned.is_empty() {
        return None;
    }
    let path = Path::new(&cleaned);
    if playlist_stem(path).is_none() {
        return Some(DEFAULT_PLAYLIST_NAME.to_string());
    }
    Some(if is_playlist(path) {
        cleaned
    } else {
        format!("{cleaned}.tplay")
    })
}

/// Playlist file format (shared for read/write).
#[derive(Serialize, Deserialize)]
struct PlaylistData { paths: Vec<String> }

/// Serialize tracks as `.tplay` JSON. The pure half, shared by the local
/// writer and the SMB save (which ships the bytes to the worker instead of
/// writing a local file).
pub fn playlist_json(tracks: &[PathBuf]) -> std::io::Result<String> {
    let data = PlaylistData {
        paths: tracks.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect(),
    };
    Ok(serde_json::to_string_pretty(&data)?)
}

/// Write the given tracks as a `.tplay` playlist file (paths only).
pub fn write_playlist(path: &Path, tracks: &[PathBuf]) -> std::io::Result<()> {
    std::fs::write(path, playlist_json(tracks)?)
}

/// Read a `.tplay` playlist file, resolving relative entries against `base`.
///
/// `base` is explicit rather than `path.parent()` because a remote playlist is
/// read from its **spool cache copy** — resolving against that would silently
/// drop every relative track. Local callers pass the file's own directory;
/// remote callers pass the share directory URI the playlist was browsed at.
///
/// `smb://` entries are kept verbatim and are NOT resolved against `base`.
/// They only *look* relative: `Path::is_relative` is true for anything without
/// a leading `/`, so keying off it would join every remote track onto the base
/// and turn `smb://nas/m/x.mp3` into `smb://nas/m/smb://nas/m/x.mp3`.
pub fn read_playlist(path: &Path, base: &Path) -> Option<Vec<PathBuf>> {
    let json = std::fs::read_to_string(path).ok()?;
    let data: PlaylistData = serde_json::from_str(&json).ok()?;
    Some(
        data.paths
            .into_iter()
            .map(|s| {
                let p = PathBuf::from(s);
                if crate::network::is_remote(&p) || p.is_absolute() {
                    p
                } else {
                    base.join(p)
                }
            })
            .collect(),
    )
}

/// The shared tag/duration cache: track id → what we know about it.
///
/// Named because it is no longer the Library's private business — the Playlist
/// and Now Playing panes read the same map, and `sort_entries` / `LibraryState`
/// take it as a parameter, so the full `HashMap<PathBuf, TrackInfo>` was spelled
/// out in four signatures. The **id** is the key, which for a remote track is an
/// `smb://` URI held in a `PathBuf` (see `tracks.rs`).
pub type TagCache = HashMap<PathBuf, TrackInfo>;

/// Tags + duration read from one audio file. Missing fields stay blank —
/// tag strings are fallible metadata, never a reason to fail the scan.
#[derive(Clone, Debug, Default)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
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
    let mut info = TrackInfo {
        title: get(ItemKey::TrackTitle),
        artist: get(ItemKey::TrackArtist),
        album: get(ItemKey::AlbumTitle),
        track_no: get_opt(ItemKey::TrackNumber),
        duration: {
            let d = tagged.properties().duration();
            if d.is_zero() { None } else { Some(d) }
        },
    };
    // lofty reports Duration::ZERO for streams with unknown length (mainly MP3).
    if info.duration.is_none() {
        info.duration = crate::audio::probe_duration(path);
    }
    Some(info)
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
pub const SORT_OPTIONS: [&str; 4] = ["Title", "Artist", "Album", "Duration"];

/// Sort a file list in place by column `col`, ascending or descending. Shared
/// by the local folder list and the SMB share list — both are `Vec<Entry>`, and
/// for a share `path` is an `smb://` URI, which every key here handles like any
/// other path (its `file_name`/`file_stem` are the remote name).
///
/// Descending is a `reverse()` after an ascending sort rather than a reversed
/// comparator, so equal keys keep their relative order instead of flipping on
/// every toggle.
pub fn sort_entries(
    entries: &mut [Entry],
    cache: &TagCache,
    col: usize,
    asc: bool,
) {
    entries.sort_by_cached_key(|e| sort_key(e, cache.get(e.path()), col));
    if !asc {
        entries.reverse();
    }
}

/// Tagged title, falling back to the file stem — matches what rows display.
pub fn title_or_stem(path: &Path, info: Option<&TrackInfo>) -> String {
    info.and_then(|i| (!i.title.is_empty()).then_some(i.title.clone()))
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string())
}

/// Sort key for a library entry and column. Returns a string that sorts correctly:
/// - Empty/missing values get a prefix that sorts after normal content.
/// - Durations become zero-padded milliseconds.
/// - The Album column is a composite `album \x01 track` key, so one album lists
///   1, 2, 3… rather than alphabetically. `\x01` is below every printable char.
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
            // Album, then track number within it. Unnumbered tracks fall back to
            // the title so they group alphabetically *after* the numbered ones
            // ('0' < '1'). A "3/12" multi-disc number does not parse and takes
            // that same fallback — the album still groups, the order does not.
            // ponytail: parse the leading digits if multi-disc ordering matters.
            if e.is_dir { String::new() } else {
                let album = info.map(|i| i.album.as_str()).unwrap_or_default();
                // Same \x7f sink as the empty case below, so an untagged file
                // ties with a folder instead of sorting above every album.
                let head = if album.is_empty() { "\x7f" } else { album };
                let track = match info.and_then(|i| i.track_no.as_deref()).and_then(|n| n.trim().parse::<u32>().ok()) {
                    Some(n) => format!("{n:06}"),
                    None => title_or_stem(e.path(), info),
                };
                format!("{head}\x01{track}")
            }
        }
        3 => {
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

/// Fixed user-folder shortcuts (Home + XDG user dirs) shown above the
/// Favorites list in the Library pane. Missing dirs are skipped; dupes (e.g.
/// Music == Home) are dropped.
///
/// Free function, not a `TPlayApp` method: it reads no app state, and it
/// belongs next to the rest of the folder-listing code.
pub fn quick_folders() -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(("Home".into(), h));
    }
    for (label, d) in [
        ("Music", dirs::audio_dir()),
        ("Downloads", dirs::download_dir()),
        ("Desktop", dirs::desktop_dir()),
    ] {
        if let Some(p) = d.filter(|p| p.is_dir()) {
            if !v.iter().any(|(_, e)| e == &p) {
                v.push((label.into(), p));
            }
        }
    }
    v
}

/// The Library pane's browsing state: the current folder, its rows, the sort,
/// the bookmarks and the hidden-folder toggle.
///
/// Six `TPlayApp` fields, and the awkward part was not the fields but that the
/// list could not be exercised without an app: `navigate_to` also kicks off a
/// tag scan, so the *state* was welded to the *effect*. Splitting them means
/// `open` is a pure function of a directory — it lists, sorts, and hands back
/// the paths that need scanning — and the caller (`TPlayApp::navigate_to`) does
/// the scanning. That is what makes this testable headless.
///
/// The shared `tag_cache` stays in the app: the Playlist and Now Playing panes
/// read it too, so it is not the Library's to own.
pub struct LibraryState {
    dir: PathBuf,
    entries: Vec<Entry>,
    /// Index into `SORT_OPTIONS` (0 = Title, the default) plus direction.
    sort: usize,
    asc: bool,
    favorites: Vec<PathBuf>,
    show_hidden: bool,
}

impl LibraryState {
    pub fn new(dir: PathBuf, favorites: Vec<PathBuf>, show_hidden: bool) -> Self {
        Self { dir, entries: Vec::new(), sort: 0, asc: true, favorites, show_hidden }
    }

    pub fn dir(&self) -> &Path { &self.dir }
    pub fn entries(&self) -> &[Entry] { &self.entries }
    pub fn sort(&self) -> usize { self.sort }
    pub fn sort_asc(&self) -> bool { self.asc }
    pub fn favorites(&self) -> &[PathBuf] { &self.favorites }
    pub fn show_hidden(&self) -> bool { self.show_hidden }
    pub fn is_favorite(&self, dir: &Path) -> bool { self.favorites.iter().any(|d| d == dir) }

    /// List `dir` into rows and sort them. False if it isn't a directory, in
    /// which case nothing changed.
    ///
    /// Returns the audio files to tag — subfolders and `.tplay` rows excluded,
    /// because a playlist file isn't audio and a folder has no tags of its own.
    /// The caller starts the scan; this only says what to scan.
    pub fn open(&mut self, dir: PathBuf, tags: &TagCache) -> Option<Vec<PathBuf>> {
        if !dir.is_dir() {
            return None;
        }
        self.dir = dir;
        let (dirs, files) = list_dir(&self.dir, self.show_hidden);
        self.entries = dirs
            .into_iter()
            .map(|p| Entry { path: p, is_dir: true })
            .chain(files.into_iter().map(|p| Entry { path: p, is_dir: false }))
            .collect();
        self.apply_sort(tags);
        Some(self.to_scan())
    }

    /// The audio rows, i.e. what a folder listing should have tagged.
    pub fn to_scan(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|e| !e.is_dir() && !is_playlist(e.path()))
            .map(|e| e.path().to_path_buf())
            .collect()
    }

    /// Re-sort the current rows against a tag cache. Missing tags sort last, and
    /// folders are untagged entries, so the tag columns sink them below files.
    pub fn apply_sort(&mut self, tags: &TagCache) {
        sort_entries(&mut self.entries, tags, self.sort, self.asc);
    }

    /// Header click: pick a new column (ascending) or flip the active one and
    /// re-sort in place. False for an out-of-range column.
    pub fn set_sort(&mut self, key: usize, tags: &TagCache) -> bool {
        if key >= SORT_OPTIONS.len() {
            return false;
        }
        if self.sort == key {
            self.asc = !self.asc;
        } else {
            self.sort = key;
            self.asc = true;
        }
        self.apply_sort(tags);
        true
    }

    /// Bookmark/unbookmark a folder, reporting whether it is now a favorite.
    pub fn toggle_favorite(&mut self, dir: PathBuf) -> bool {
        match self.favorites.iter().position(|d| d == &dir) {
            Some(i) => {
                self.favorites.remove(i);
                false
            }
            None => {
                self.favorites.push(dir);
                true
            }
        }
    }

    /// Show or hide dot-prefixed folders, reporting whether it changed. The
    /// caller re-lists on a change; the rows themselves are not touched here.
    pub fn set_show_hidden(&mut self, show: bool) -> bool {
        if self.show_hidden == show {
            return false;
        }
        self.show_hidden = show;
        true
    }
}