//! Taking things **off** the iPod: a track, and the database rows that name it.
//!
//! # What a track actually is
//!
//! Four things, and removing the file is the easy one:
//!
//! 1. the audio file under `iPod_Control/Music/Fnn/`,
//! 2. its `mhit` record in the track list,
//! 3. its `mhip` entry in **every** playlist that lists it, and
//! 4. the `mhia` album, once nothing else in that album is left.
//!
//! Dropping only (2) is the failure worth naming: the device keeps showing a
//! track in three menus and plays nothing when it is picked, because the bytes
//! are gone. Dropping only (1) leaves a row that cannot play. So this module
//! removes all four, and counts what it did so a caller can say so out loud.
//!
//! # Matching a file to its record
//!
//! By the **location string** — the `mhod` at field 2, which holds
//! `:iPod_Control:Music:F07:ABCD.mp3`. Not by filename alone: iTunes names
//! pushed files by track id, but a file copied in by hand has no record at all,
//! and a filename match would then delete a file the database still lists.
//!
//! # Album art is left alone, deliberately
//!
//! Removing an artwork record would renumber every record after it, and record
//! *N* is slot *N* in each of the three `.ithmb` stores — so one deleted image
//! would put the wrong cover on every image added after it. The bytes stay and
//! the record stays; nothing points at them any more. Reclaiming that space is a
//! whole-device rewrite rather than a per-track edit, and it is the only thing
//! that can free it safely.

use crate::ipod::db::field;
use crate::ipod::db::id;
use crate::ipod::db::{self};
use crate::ipod::device::Device;
use crate::ipod::hash58;
use crate::ipod::push::{commit, kind_of};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// What one removal took off the device.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// `mhit` records dropped.
    pub tracks: usize,
    /// `mhip` playlist entries dropped.
    pub playlist_items: usize,
    /// `mhia` albums dropped for having no tracks left.
    pub albums: usize,
    /// Files actually unlinked.
    pub files: usize,
}

impl std::ops::AddAssign for Removed {
    fn add_assign(&mut self, other: Self) {
        self.tracks += other.tracks;
        self.playlist_items += other.playlist_items;
        self.albums += other.albums;
        self.files += other.files;
    }
}

/// `mhit`'s track id.
const M_ID: usize = 16;
/// `mhit`'s album id, which is what has to be checked afterwards.
const M_ALBUM: usize = 288;
/// `mhia`'s album id, in its own header.
const A_ID: usize = 16;
/// `mhip`'s reference to the track it plays.
const H_TRACK: usize = 24;

/// Every kind of playlist list holder. The device carries three — the classic
/// one, podcasts and the new-playlist kind — and a track can sit in any of them,
/// so the sweep is over the kinds rather than over "the" playlist list.
const PLAYLIST_KINDS: [u32; 3] = [db::kind::PLAYLISTS, 3, 5];

/// Every playable file under `dir`, at any depth.
///
/// This is the half that makes a removal recursive, and it lives here rather than
/// in the pane because "which files does this action cover" is the question the
/// removal owns — the pane asks it and then shows the count in its confirm.
///
/// **`DirEntry::file_type`, not `Path::is_dir`**: the latter follows symlinks, so
/// a symlink loop inside a music folder would hang the walk forever. A symlinked
/// folder is skipped instead, which is also the honest answer — its tracks are
/// somewhere else on the device and belong to whatever the link names.
pub fn audio_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => out.extend(audio_files_under(&path)),
            Ok(_) if kind_of(&path).is_some() => out.push(path),
            _ => {}
        }
    }
    out
}

/// Remove every one of `paths` from the device, in **one** database write.
///
/// This is the entry point both callers use, and the batching is the point rather
/// than an optimisation. Each `commit` is a whole 1.9 MB re-serialise, sign,
/// back-up and rename, so the obvious "call `remove_track` in a loop" cost a
/// megabyte-and-a-half per track: removing a real 1169-track library that way is
/// over two gigabytes of writes onto a FAT device, and the per-track backup would
/// be the same file overwritten 1169 times so that a failure halfway through
/// loses 1168 removals' worth of work.
///
/// So the whole set is edited in one pass — every matching `mhit`, every `mhip`
/// naming any of them, every album left empty — and written once. One backup, one
/// signature, one atomic rename.
pub fn remove_paths(device: &Device, paths: &[PathBuf]) -> Result<Removed, String> {
    if paths.is_empty() {
        return Ok(Removed::default());
    }

    // Every path is checked before anything is opened or unlinked. The one thing
    // this must never do is delete a file the caller passed that is not on the
    // iPod, and the first version of this branched *into* the unlink instead of
    // out of it.
    for path in paths {
        if path.strip_prefix(&device.root).is_err() {
            return Err(format!(
                "{} is not on this iPod, so there is nothing to remove",
                path.display()
            ));
        }
    }

    // Files inside the music folder have records; anything else on the device is
    // a file the track list never named — a playlist, anything iTunes put
    // elsewhere — and the file is all there is.
    let (mut music, mut loose) = (Vec::new(), Vec::new());
    for path in paths {
        if path.starts_with(device.music_dir()) {
            music.push(path);
        } else {
            loose.push(path);
        }
    }

    let mut removed = Removed::default();
    for path in &loose {
        removed.files += usize::from(unlink_file(path));
    }

    if music.is_empty() {
        return Ok(removed);
    }

    // The location strings, normalised once. Every match from here is against
    // this set, so the comparison is a hash lookup rather than a walk per track.
    let wanted: HashSet<String> = music
        .iter()
        .filter_map(|p| location_of(device, p))
        .map(|l| normalise(&l))
        .collect();

    let bytes = std::fs::read(device.database())
        .map_err(|e| format!("cannot read the iPod's database: {e}"))?;
    let mut tree = db::parse(&bytes)?;

    let mut emptied: Vec<u32> = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    if let Some(list) = db::list_holder_mut(&mut tree, db::kind::TRACKS) {
        let mut keep = Vec::with_capacity(list.children_of(id::MHIT).count());
        for track in list.children_mut().drain(..) {
            let matched = track
                .mhod(field::LOCATION)
                .and_then(|m| m.mhod_text())
                .is_some_and(|stored| wanted.contains(&normalise(&stored)));
            if matched {
                removed.tracks += 1;
                ids.extend(track.u32(M_ID));
                emptied.extend(track.u32(M_ALBUM));
            } else {
                keep.push(track);
            }
        }
        *list.children_mut() = keep;
    }

    if removed.tracks == 0 {
        // Every file asked for is one the database does not list — hand-copied
        // in, or left by a half-finished sync. They still go: it is the one
        // unambiguous half of the request. Refusing would leave files the user
        // cannot remove at all, which is the same as not offering the feature.
        for path in &music {
            log::warn!("{} is on the iPod but has no track record", path.display());
            removed.files += usize::from(unlink_file(path));
        }
        return Ok(removed);
    }

    // Every playlist entry naming any of them, in every playlist of every kind.
    for kind in PLAYLIST_KINDS {
        let Some(playlists) = db::list_holder_mut(&mut tree, kind) else {
            continue;
        };
        for playlist in playlists.children_mut() {
            let before = playlist.children_of(id::MHIP).count();
            playlist
                .children_mut()
                .retain(|item| item.u32(H_TRACK).is_none_or(|track| !ids.contains(&track)));
            removed.playlist_items += before - playlist.children_of(id::MHIP).count();
        }
    }

    // An album with nothing left in it, and only one the removal could have
    // emptied — so a track that was never albumed cannot take a populated album
    // with it. The surviving albums are collected before the album list is
    // borrowed, because that check reads the track list this tree also owns.
    let surviving: Vec<u32> = db::list_holder(&tree, db::kind::TRACKS)
        .map(|list| {
            list.children_of(id::MHIT)
                .filter_map(|t| t.u32(M_ALBUM))
                .collect()
        })
        .unwrap_or_default();
    if let Some(albums) = db::list_holder_mut(&mut tree, db::kind::ALBUMS) {
        let before = albums.children_of(id::MHIA).count();
        // Keep an album unless this removal emptied it.
        albums.children_mut().retain(|album| match album.u32(A_ID) {
            Some(album_id) if emptied.contains(&album_id) => surviving.contains(&album_id),
            _ => true,
        });
        removed.albums = before - albums.children_of(id::MHIA).count();
    }

    let mut finished = tree.to_bytes();
    hash58::sign(device.guid, &mut finished)?;
    // The database first, because `commit` keeps the previous one in the cache
    // directory and the files are the only part here that cannot be got back.
    commit(device, &finished)?;

    for path in &music {
        removed.files += usize::from(unlink_file(path));
    }

    // One record per removal, not per track — the batch is one event and a
    // thousand-track removal must not be a thousand lines. **`Info` because this
    // is a state change on the user's only music device**, and the line it
    // complements says nothing useful on its own: `commit` reports the file it
    // wrote and its size, which is byte-for-byte the same line a push produces.
    // Without this, the log could not tell a removal of one track from a removal
    // of the whole library, or tell either from a push.
    log::info!(
        "took {} off the iPod: {} file, {} playlist entr{}, {} album",
        removed.tracks,
        removed.files,
        removed.playlist_items,
        if removed.playlist_items == 1 {
            "y"
        } else {
            "ies"
        },
        removed.albums,
    );
    Ok(removed)
}

/// The location string the database would hold for `path`.
///
/// `:` because that is what iTunes writes, and `/` because that is what other
/// tools write. Only one side is produced and one comparison is made, so the
/// choice here and the tolerance in [`same_location`] are two halves of one rule
/// — and a comparison that assumed the wrong separator would find nothing at
/// all and report "not in your library" over a library that plainly has it.
fn location_of(device: &Device, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(&device.root).ok()?;
    // The leading `:` is the device's own spelling (HFS-style, as iTunes writes
    // it), so what this builds is exactly what a record for this file would hold.
    let mut out = String::from(":iPod_Control:Music");
    for part in rel.components() {
        let text = part.as_os_str().to_string_lossy();
        if text == "iPod_Control" || text == "Music" {
            continue;
        }
        out.push(':');
        out.push_str(&text);
    }
    Some(out)
}

/// One spelling of a location: `:` separated, no leading one, lower case.
///
/// Both sides of every comparison go through this, and **both sides are
/// normalised at the point they are read** — the wanted strings once when the
/// set is built, a stored one per record. Case-insensitive because the filesystem
/// is and the two disagree about it.
fn normalise(location: &str) -> String {
    location
        .replace('/', ":")
        .trim_start_matches(':')
        .to_lowercase()
}

/// Delete `path`, saying whether there was one.
fn unlink_file(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            log::warn!("could not remove {}: {e}", path.display());
            false
        }
    }
}
