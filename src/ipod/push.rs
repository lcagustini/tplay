//! Copying albums and playlists onto an iPod.
//!
//! # The order, and why it is this order
//!
//! 1. Copy the audio to `iPod_Control/Music/Fxx/NAME.ext`.
//! 2. Add one `mhit` to the track list carrying the track's own tags.
//! 3. Add that track to the **Library playlist**, the master `mhyp` — or it is in
//!    the database and in no playlist, and nothing on the device can reach it.
//! 4. Make sure the album has an entry in the album list.
//! 5. Write the database, once, signed.
//!
//! Step 1 before step 5 is deliberate. A file with no database row is invisible
//! to the device and harmless: the firmware never rebuilds `iTunesDB`, so it
//! never notices a file it was not told about. The reverse — a row naming a file
//! that is not there — is a track that appears in a menu and will not play. So
//! if a copy fails halfway, the half-finished state is the safe one, and the
//! database is not rewritten to mention files that are not there.
//!
//! # Every record is cloned, never constructed
//!
//! The `mhit` header is 624 bytes and this module sets about sixteen of them.
//! The rest hold constants that vary by file format, by iTunes version, and by
//! fields nobody has documented: `type1`/`type2` differ between CBR and VBR MP3,
//! `unk9` is `0xFFFF` for compressed audio, and there are two more eight-byte
//! fields whose derivation is unknown.
//!
//! So [`template_for`] finds a track already on the device with the file type
//! being added, and the new record starts as a copy of it. Every field this
//! module cannot name keeps a value a real iTunes wrote for a real file of that
//! type, and the code only has to be right about the fields it can.
//!
//! The album list is the exception, because it has nothing to copy *from*: a new
//! album has no precedent. Its `mhia` carries three varying words that are
//! almost certainly iTunes' content hashes. The firmware cannot check them — it
//! has no key for them, and the key it would need is absent from this device's
//! empty `SysInfo` — so they are written as zero, which is the only thing a
//! tool outside iTunes can honestly do.

use crate::config;
use crate::ipod::aac::encode_file;
use crate::ipod::db::{self, Chunk};
use crate::ipod::db::{field as f, id};
use crate::ipod::hash58;
use crate::ipod::Device;
use crate::library::TrackInfo;
use crate::tracks;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds between the Unix epoch and the Mac epoch, which is what every
/// timestamp in this database counts from. Measured: the fixture's Library
/// playlist was created at 3 874 501 490, and that minus 2 082 844 800 is a date
/// in June 2026.
const MAC_EPOCH_OFFSET: u64 = 2_082_844_800;

/// `F00` through `F49`. The number is not load-bearing — the location string
/// below is what the device follows — but a directory that holds every track is
/// a directory that eventually runs out of FAT32 entries.
const MUSIC_FOLDERS: u32 = 50;

// ── Field offsets ─────────────────────────────────────────────────────────────
//
// Measured against `tests/fixtures/ipod/iTunesDB`, not taken from wikiPodLinux:
// that page's `mhit` table describes the iTunes 7.1 layout and this database is
// version 0x75, where fields past roughly 0x100 have moved. Three constants
// below disagree with the published table, and each one cost a measurement.

/// `mhit`.
const M_MHODS: usize = 12;
const M_ID: usize = 16;
const M_VISIBLE: usize = 20;
const M_FILETYPE: usize = 24;
const M_RATING: usize = 31;
const M_MODIFIED: usize = 32;
const M_SIZE: usize = 36;
const M_LENGTH: usize = 40;
const M_ADDED: usize = 104;
const M_DBID: usize = 112;
/// `has_artwork`. **2 means "none"** and 1 means "there is some", so a record
/// for a track we gave no picture to must say 2 — the other way round is the
/// device showing an empty frame.
const M_HAS_ARTWORK: usize = 164;
const M_MEDIATYPE: usize = 208;
/// The artwork image id, which is what joins a track to a picture. Measured:
/// it repeats across the tracks of one album and matches `ArtworkDB`'s `mhii`
/// ids, and 170 of the fixture's 1169 tracks carry zero.
const M_IMAGE_ID: usize = 352;
/// The album this track belongs to. **Not 314**, where the published table puts
/// it: offset 314 holds a value matching no album for any of the fixture's 1169
/// tracks, while 288 matches 1165 of them.
const M_ALBUM: usize = 288;

/// `mhia`.
const A_CHILDREN: usize = 12;
const A_ID: usize = 16;
const A_HASH_A: usize = 20;
const A_HASH_B: usize = 24;
const A_HASH_C: usize = 32;

/// `mhip`, and the two counts a `mhyp` keeps in its own header.
const P_TRACK: usize = 24;
const P_STAMP: usize = 32;
const P_MASTER: usize = 20;
const P_PERSISTENT: usize = 28;
const H_MHODS: usize = 12;
const H_MHIPS: usize = 16;

/// The manual sort position stored in the `mhod` that follows a playlist item.
const ORDER_POSITION: usize = 24;

fn now_mac() -> u32 {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    unix.saturating_add(MAC_EPOCH_OFFSET) as u32
}

/// A content key for a cover, so one image is written per picture rather than
/// per track.
///
/// `DefaultHasher` because it is the stdlib's and it is only ever compared
/// against itself — this is a dictionary key inside one push, never persisted,
/// so it does not have to be stable across runs or across machines.
fn cover_key(cover: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cover.hash(&mut hasher);
    hasher.finish()
}

/// A file type an iPod Classic can be asked to play.
///
/// **`code` is the four bytes in the order the file holds them, which is the
/// readable name spelled backwards.** `mhit` offset 24 carries `MP3 ` as the
/// bytes `20 33 50 4D`; the published table calls it "an integer field and is
/// reversed". Read it forwards and every template lookup comes back empty,
/// which is what this bit first cost.
///
/// FLAC, OGG and WAV are refused by name rather than transcoded: a transcoder is
/// a dependency, a licensing question and a second thing that can fail, for
/// files the device would refuse anyway.
pub struct Kind {
    code: [u8; 4],
    extension: &'static str,
}

/// The formats this writes. AAC and ALAC reach the device as `.m4a`.
pub const KINDS: [Kind; 2] = [
    Kind {
        code: *b" 3PM",
        extension: "mp3",
    },
    Kind {
        code: *b" A4M",
        extension: "m4a",
    },
];

/// The kind of file at `path`, or `None` when the device cannot play it.
pub fn kind_of(path: &Path) -> Option<&'static Kind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    KINDS.iter().find(|k| k.extension == extension)
}

/// What one push did.
#[derive(Debug, Default)]
pub struct Summary {
    pub added: usize,
    /// Files the device cannot hold, named so the user can see what was left out.
    pub refused: Vec<String>,
    /// Tracks already on the device, left alone rather than copied a second time.
    pub already: usize,
}

/// What one job did, whichever direction it went.
///
/// An enum rather than one struct with two sets of zeroed fields: a removal
/// reports counts that a push never has, and a caller that forgets to check
/// which one it is holding gets a number that means nothing instead of a type
/// error.
#[derive(Debug)]
pub enum Outcome {
    Added(Summary),
    Removed(crate::ipod::remove::Removed),
    /// Something with nothing to count: a mount, an eject, a discovery. A
    /// sentence rather than a payload, because these operations report a fact
    /// and nothing that follows reads their numbers.
    Note(String),
}

impl Outcome {
    /// The line the pane shows under the button.
    pub fn describe(&self) -> String {
        match self {
            Self::Added(summary) => {
                // "0 on the iPod, 14 already there" is a true sentence and a
                // useless one: pressing the button twice is the success case, and
                // it should read as having worked.
                if summary.added == 0 && summary.already > 0 {
                    return format!("{} already on the iPod", plural(summary.already, "track"));
                }
                let mut line = format!("{} on the iPod", plural(summary.added, "track"));
                if summary.already > 0 {
                    line.push_str(&format!(", {} already there", summary.already));
                }
                if !summary.refused.is_empty() {
                    line.push_str(&format!(", {} refused", summary.refused.len()));
                }
                line
            }
            Self::Removed(gone) => {
                let mut line = format!("{} off the iPod", plural(gone.tracks, "track"));
                if gone.albums > 0 {
                    line.push_str(&format!(", {} album", plural(gone.albums, "")));
                }
                line
            }
            Self::Note(text) => text.clone(),
        }
    }

    /// The hover detail: the full paths a push refused, and for a removal the
    /// parts a user checking the result would want to know.
    pub fn detail(&self) -> String {
        match self {
            Self::Added(summary) => summary.refused.join("\n"),
            Self::Removed(gone) => format!(
                "{} file, {} playlist entr{}, {} album",
                plural(gone.files, ""),
                gone.playlist_items,
                if gone.playlist_items == 1 { "y" } else { "ies" },
                plural(gone.albums, ""),
            ),
            Self::Note(_) => String::new(),
        }
    }
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// One open push. Built by [`Push::open`], finished by [`Push::finish`].
pub struct Push<'a> {
    device: &'a Device,
    root: Chunk,
    next_id: u32,
    next_dbid: u64,
    next_album_id: u32,
    next_playlist_id: u64,
    /// On-device filenames already in use, so a new one cannot overwrite one.
    taken: HashSet<String>,
    /// Cover images already added in this push, so one album's twelve tracks
    /// produce one picture rather than twelve. Keyed on the cover's own bytes:
    /// two tracks of one album carry byte-identical art, and two albums that
    /// happen to share a cover file should share an image on the device too.
    covers: HashMap<u64, u32>,
    /// Track id by song identity — see [`identity`] — so pushing a folder twice
    /// finds the songs already there instead of copying each one again.
    present: HashMap<String, u32>,
    /// How many `add_track` calls found one, for the summary.
    already: usize,
}

impl<'a> Push<'a> {
    /// Read `device`'s database and take stock of the ids a new record needs.
    ///
    /// The scheme check comes first, before a single record is read. The
    /// alternative is a write that reports success to a database the device will
    /// reject, which reads as "the sync worked" right up to the moment the music
    /// does not appear.
    pub fn open(device: &'a Device) -> Result<Self, String> {
        let bytes = std::fs::read(device.database())
            .map_err(|e| format!("cannot read the iPod's database: {e}"))?;
        match hash58::scheme(&bytes) {
            Some(1) => {}
            Some(other) => {
                return Err(format!(
                    "this iPod signs its database with scheme {other}; this app writes HASH58 only"
                ))
            }
            None => return Err("the iPod's database records no hashing scheme".into()),
        }
        let root = db::parse(&bytes)?;
        let mut push = Self {
            device,
            root,
            next_id: 0,
            next_dbid: 0,
            next_album_id: 0,
            next_playlist_id: 0,
            taken: HashSet::new(),
            covers: HashMap::new(),
            present: HashMap::new(),
            already: 0,
        };
        push.take_stock()?;
        Ok(push)
    }

    fn take_stock(&mut self) -> Result<(), String> {
        let tracks = db::list_holder(&self.root, db::kind::TRACKS)
            .ok_or("this iPod's database has no track list")?;
        for record in tracks.children_of(id::MHIT) {
            self.next_id = self
                .next_id
                .max(record.u32(M_ID).unwrap_or_default().saturating_add(1));
            self.next_dbid = self
                .next_dbid
                .max(record.u64(M_DBID).unwrap_or_default().saturating_add(1));
            if let Some(stem) = record
                .mhod(f::LOCATION)
                .and_then(|m| m.mhod_text())
                .and_then(|l| l.rsplit(':').next().map(str::to_string))
            {
                self.taken.insert(stem);
            }
            // What is already on the device, so a second push of the same folder
            // recognises it. Only records the device actually holds **and will
            // show** — a hidden row is not one the user can see duplicated, so
            // skipping it would lose the song rather than avoid a copy.
            if record.u32(M_VISIBLE).unwrap_or(1) == 1 {
                self.present
                    .insert(identity_of(record), record.u32(M_ID).unwrap_or(0));
            }
        }
        if let Some(albums) = db::list_holder(&self.root, db::kind::ALBUMS) {
            for album in albums.children_of(id::MHIA) {
                self.next_album_id = self
                    .next_album_id
                    .max(album.u32(A_ID).unwrap_or_default().saturating_add(1));
            }
        }
        let playlists = db::list_holder(&self.root, db::kind::PLAYLISTS)
            .ok_or("this iPod's database has no playlist list")?;
        self.master()
            .ok_or("this iPod's database has no Library playlist")?;
        self.next_playlist_id = playlists
            .children_of(id::MHYP)
            .filter_map(|p| p.u64(P_PERSISTENT))
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        Ok(())
    }

    fn master(&self) -> Option<&Chunk> {
        db::list_holder(&self.root, db::kind::PLAYLISTS)?
            .children_of(id::MHYP)
            .find(|p| p.u8(P_MASTER) == Some(1))
    }

    /// Add `path`, and return the track id the device will know it by.
    ///
    /// The cover is read from the track itself — embedded art, or a
    /// `cover.jpg` beside it, by the same `tracks::cover` the Album Cover pane
    /// uses — so no album grouping is needed and a playlist spanning four albums
    /// gets each track's own.
    pub fn add_track(&mut self, path: &Path, aac_bps: Option<u32>) -> Result<u32, String> {
        let Some(kind) = kind_of(path) else {
            return Err("this iPod cannot play that format".into());
        };
        let info = tracks::info(path)
            .ok_or_else(|| format!("{}: the file could not be read", path.display()))?;

        // Already on the device? Then this is the second push of the same song,
        // and the graceful answer is to leave the one that is there. Checked
        // before anything is copied, because the file copy is the expensive part
        // and the record is the part the user notices.
        let key = identity(
            &info.title,
            &info.artist,
            &info.album,
            &String::from_utf8_lossy(&kind.code),
        );
        if let Some(existing) = self.present.get(&key).copied() {
            self.already += 1;
            log::info!(
                "'{}' is already on the iPod as track {existing}; not copying it again",
                info.title
            );
            return Ok(existing);
        }

        let track_id = self.next_id;
        let (location, size, encoded_duration) =
            self.place(track_id, path, kind.extension, aac_bps)?;
        let album_id = self.album_id(&info);

        let template = {
            let list = db::list_holder(&self.root, db::kind::TRACKS)
                .ok_or("this iPod's database has no track list")?;
            template_for(list, kind.code)
                .ok_or_else(|| {
                    format!(
                        "this iPod holds no {} file to take the record's shape from",
                        kind.extension
                    )
                })?
                .clone()
        };

        let mut record = Chunk::leaf(id::MHIT, template.header().to_vec());
        record.set_u32(M_MHODS, 0);
        record.set_u32(M_ID, track_id);
        record.set_u32(M_VISIBLE, 1);
        record.set_u32(M_SIZE, size);
        // Use encoded duration if available (more accurate than lofty's estimate on the source)
        record.set_u32(
            M_LENGTH,
            encoded_duration.unwrap_or_else(|| info.duration.map_or(0, |d| d.as_millis() as u32)),
        );
        record.set_u32(M_MODIFIED, now_mac());
        record.set_u32(M_ADDED, now_mac());
        record.set_u64(M_DBID, self.next_dbid);
        // 2 is "no artwork", not 0: the published table says the device will
        // not show artwork for a track that has none, and this app does not
        // write artwork yet. One is "there is some", which would be a lie.
        record.set_u8(M_HAS_ARTWORK, 2);
        record.set_u32(M_MEDIATYPE, 1);
        record.set_u8(M_RATING, info.rating.min(5).saturating_mul(20));
        record.set_u32(M_ALBUM, album_id);

        for (field, text) in strings_for(&info, &location) {
            let mut mhod = template
                .children_of(id::MHOD)
                .find(|m| m.mhod_field() == Some(field))
                .cloned()
                .unwrap_or_else(new_string_mhod);
            write_string_mhod(&mut mhod, field, &text);
            record.push(mhod);
        }
        record.set_u32(M_MHODS, record.children_of(id::MHOD).count() as u32);

        if let Some(image_id) = self.artwork_for(path, self.next_dbid.saturating_sub(1))? {
            record.set_u32(M_IMAGE_ID, image_id);
            record.set_u8(M_HAS_ARTWORK, 1);
        }

        db::list_holder_mut(&mut self.root, db::kind::TRACKS)
            .ok_or("the track list vanished")?
            .push(record);
        self.next_id = track_id.saturating_add(1);
        self.next_dbid = self.next_dbid.saturating_add(1);
        self.link_into_library(track_id)?;
        Ok(track_id)
    }

    /// The artwork id this track should carry, adding the picture if this is the
    /// first track of its cover.
    ///
    /// The pixels go onto the device immediately and the *link* is written into
    /// the track record, which is not written until the end of the push. So a
    /// push that fails after this leaves a picture on the device that no track
    /// points at — which the firmware ignores, and the next push reuses the slot
    /// number rather than leaving a hole.
    /// How many `add_track` calls found the song already on the device, which is
    /// what `push_album` subtracts from the ids it was handed.
    pub fn already(&self) -> usize {
        self.already
    }

    /// Whether a track at `path` should be converted to AAC at `target_bps`.
    /// Whether a track at `path` should be converted to AAC at `target_bps`.
    ///
    /// - If the format is not playable on the device (FLAC, OGG, WAV), always convert.
    /// - Otherwise, convert only when the source's average bitrate exceeds the target.
    ///   Average bitrate = file_size * 8 / duration_ms, computed from fs::metadata and tracks::info.
    pub fn should_convert(path: &Path, target_bps: u32) -> bool {
        let Some(_kind) = kind_of(path) else {
            return true; // not playable, must convert
        };
        // Playable format (MP3/M4A): check if bitrate exceeds target.
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let file_size = meta.len();
        let Some(info) = tracks::info(path) else {
            return false;
        };
        let Some(duration) = info.duration else {
            return false;
        };
        let duration_ms = duration.as_millis() as u64;
        if duration_ms == 0 {
            return false;
        }
        let avg_bps = (file_size * 8) / duration_ms;
        avg_bps > target_bps as u64
    }

    /// What one song's identity is, and what the second push of it must not do.
    fn artwork_for(&mut self, path: &Path, dbid: u64) -> Result<Option<u32>, String> {
        let Some(cover) = tracks::cover(path) else {
            return Ok(None);
        };
        let key = cover_key(&cover);
        if let Some(existing) = self.covers.get(&key) {
            return Ok(Some(*existing));
        }
        let added = crate::ipod::artwork::add_image(self.device, &cover, dbid)?;
        self.covers.insert(key, added.image_id);
        Ok(Some(added.image_id))
    }

    /// Copy `path` onto the device. Returns the colon-separated location the
    /// database must carry and the size it must record.
    fn place(
        &mut self,
        track_id: u32,
        path: &Path,
        extension: &str,
        aac_bps: Option<u32>,
    ) -> Result<(String, u32, Option<u32>), String> {
        // iTunes distributes tracks across the fifty folders with its own hash,
        // which is not derivable from this database. It does not need to be: the
        // location string is what the device follows. Deriving the folder from
        // the track id spreads them the same way without inventing a rule.
        let folder = track_id % MUSIC_FOLDERS;
        let mut stem = stem_for(track_id);
        while self.taken.contains(&format!("{stem}.{extension}")) {
            // A clash means the id was reused, which only happens if this device
            // already holds a track the database did not list. Step to the next
            // name rather than overwrite somebody's audio.
            stem = stem_for(self.next_id.saturating_add(stem.as_bytes()[0] as u32));
        }
        let name = format!("{stem}.{extension}");
        self.taken.insert(name.clone());

        let dir = self.device.music_dir().join(format!("F{folder:02}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let destination = dir.join(&name);

        let (size, encoded_duration) = if let Some(bps) = aac_bps {
            if Self::should_convert(path, bps) {
                // Encode to AAC (.m4a)
                let aac_dest = destination.with_extension("m4a");
                let duration_ms = encode_file(path, bps, &aac_dest)
                    .map_err(|e| format!("AAC encode failed: {e}"))?;
                // Update extension to m4a for the location string
                let aac_name = format!("{stem}.m4a");
                return Ok((
                    format!(":iPod_Control:Music:F{folder:02}:{aac_name}"),
                    std::fs::metadata(&aac_dest)
                        .map(|m| m.len() as u32)
                        .unwrap_or(0),
                    Some(duration_ms),
                ));
            }
            // Copy as-is
            std::fs::copy(path, &destination)
                .map_err(|e| format!("{}: {e}", destination.display()))?;
            (
                std::fs::metadata(&destination)
                    .map(|m| m.len() as u32)
                    .unwrap_or(0),
                None,
            )
        } else {
            // No AAC conversion requested, copy as-is
            std::fs::copy(path, &destination)
                .map_err(|e| format!("{}: {e}", destination.display()))?;
            (
                std::fs::metadata(&destination)
                    .map(|m| m.len() as u32)
                    .unwrap_or(0),
                None,
            )
        };

        Ok((
            format!(":iPod_Control:Music:F{folder:02}:{name}"),
            size,
            encoded_duration,
        ))
    }

    /// The id of the album this track belongs to, adding an entry if the device
    /// has none by that name. `0` for an untagged file, which is what the device
    /// itself writes.
    fn album_id(&mut self, info: &TrackInfo) -> u32 {
        let name = info.album.trim();
        if name.is_empty() {
            return 0;
        }
        if let Some(albums) = db::list_holder(&self.root, db::kind::ALBUMS) {
            let found = albums.children_of(id::MHIA).find(|album| {
                album
                    .mhod(f::ALBUM_LIST)
                    .and_then(|m| m.mhod_text())
                    .is_some_and(|t| t.eq_ignore_ascii_case(name))
            });
            if let Some(id) = found.and_then(|album| album.u32(A_ID)) {
                return id;
            }
        }

        let id = self.next_album_id;
        self.next_album_id = self.next_album_id.saturating_add(1);
        let Some(albums) = db::list_holder_mut(&mut self.root, db::kind::ALBUMS) else {
            return 0;
        };
        // The three varying words that follow the id are iTunes' content hashes.
        // Zero, per the module comment: nothing outside iTunes can produce them
        // and the device cannot check them.
        let mut entry = Chunk::leaf(
            id::MHIA,
            header_or_zeros(albums.children_of(id::MHIA).next(), 76),
        );
        entry.set_u32(A_CHILDREN, 0);
        entry.set_u32(A_ID, id);
        entry.set_u32(A_HASH_A, 0);
        entry.set_u32(A_HASH_B, 0);
        entry.set_u64(A_HASH_C, 0);
        for (field, text) in [
            (f::ALBUM_LIST, name.to_string()),
            (f::ARTIST_LIST, info.artist.clone()),
            (f::ARTIST_SORT_LIST, info.artist.clone()),
        ] {
            let mut mhod = new_string_mhod();
            write_string_mhod(&mut mhod, field, &text);
            entry.push(mhod);
        }
        entry.set_u32(A_CHILDREN, 3);
        albums.push(entry);
        id
    }

    /// Put `track_id` into the Library playlist. The counts in a `mhyp` header
    /// are not derived by the tree the way lengths are, so they are recomputed
    /// by hand here.
    fn link_into_library(&mut self, track_id: u32) -> Result<(), String> {
        let playlists = db::list_holder_mut(&mut self.root, db::kind::PLAYLISTS)
            .ok_or("the playlist list vanished")?;
        let Some(master) = playlists
            .children_mut()
            .iter_mut()
            .find(|p| p.ident() == id::MHYP && p.u8(P_MASTER) == Some(1))
        else {
            return Err("the Library playlist vanished".into());
        };
        let template = master
            .children_of(id::MHIP)
            .next()
            .cloned()
            .ok_or("this iPod's Library playlist has no entries to copy the shape from")?;
        let position = master.children_of(id::MHIP).count() as u32;

        let mut item = template;
        item.set_u32(P_TRACK, track_id);
        item.set_u32(P_STAMP, now_mac());
        if let Some(order) = item.mhod_mut(f::POSITION_INDICATOR) {
            order.set_u32(ORDER_POSITION, position);
        }
        master.push(item);
        master.set_u16(H_MHIPS, master.children_of(id::MHIP).count() as u16);
        master.set_u32(H_MHODS, master.children_of(id::MHOD).count() as u32);
        Ok(())
    }

    /// Create a playlist called `name` holding `track_ids`.
    ///
    /// The tracks are already in the Library playlist from being added, so this
    /// adds a list that points at them and changes nothing else.
    pub fn add_playlist(&mut self, name: &str, track_ids: &[u32]) -> Result<(), String> {
        let playlists = db::list_holder_mut(&mut self.root, db::kind::PLAYLISTS)
            .ok_or("the playlist list vanished")?;
        let master = playlists
            .children_of(id::MHYP)
            .find(|p| p.u8(P_MASTER) == Some(1))
            .cloned()
            .ok_or("this iPod's database has no Library playlist")?;
        let item_template = master
            .children_of(id::MHIP)
            .next()
            .cloned()
            .ok_or("this iPod's Library playlist has no entries to copy the shape from")?;

        let mut list = Chunk::leaf(id::MHYP, master.header().to_vec());
        list.set_u32(H_MHODS, 1);
        list.set_u32(H_MHIPS, track_ids.len() as u32);
        list.set_u8(P_MASTER, 0);
        list.set_u32(P_STAMP, now_mac());
        list.set_u64(P_PERSISTENT, self.next_playlist_id);
        self.next_playlist_id = self.next_playlist_id.saturating_add(1);

        let mut title = new_string_mhod();
        write_string_mhod(&mut title, f::TITLE, name);
        list.push(title);

        for (position, track_id) in track_ids.iter().enumerate() {
            let mut item = item_template.clone();
            item.set_u32(P_TRACK, *track_id);
            item.set_u32(P_STAMP, now_mac());
            if let Some(order) = item.mhod_mut(f::POSITION_INDICATOR) {
                order.set_u32(ORDER_POSITION, position as u32);
            }
            list.push(item);
        }

        db::list_holder_mut(&mut self.root, db::kind::PLAYLISTS)
            .ok_or("the playlist list vanished")?
            .push(list);
        Ok(())
    }

    /// The finished database, signed.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        let mut bytes = self.root.to_bytes();
        hash58::sign(self.device.guid, &mut bytes)?;
        Ok(bytes)
    }
}

// ── Building the pieces ───────────────────────────────────────────────────────

/// One song's identity: title, artist, album and file type, lower-cased.
///
/// **Why the tags and not the bytes.** A byte hash would survive a tag edit, and
/// it costs reading every audio file on the device — gigabytes on a full
/// library. A byte *size* is free and is **not** an identity: two CBR MP3s of
/// the same duration are the same size to the byte, so a 14-track album would
/// come back as one track fourteen times over. The tags are what a person means
/// by "the same song", they cost nothing to read, and they have no false
/// positives.
///
/// **What that gives up, stated plainly:** editing a tag and pushing again adds a
/// second copy, because the edited file no longer matches anything on the
/// device. That is the deliberate direction to fail in — a duplicate is visible
/// and removable, and the alternative here is silently declining to put a song on
/// the device at all. Making a second push *update* the existing record is the
/// follow-up that needs a deliberate "replace" action; it is not something to
/// infer from a second "To iPod".
///
/// **One function, because the first version had two and they disagreed**: the
/// side read off the device skipped the normalisation, so every accented song
/// failed to match *itself*. The test that caught it uses a real Portuguese
/// title, because that is what this library actually contains.
fn identity(title: &str, artist: &str, album: &str, code: &str) -> String {
    [title, artist, album, code]
        .iter()
        .map(|s| s.trim().to_lowercase())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// A record the device already holds, read into the same shape as a file's.
fn identity_of(record: &Chunk) -> String {
    let tag = |field: u32| {
        record
            .mhod(field)
            .and_then(|m| m.mhod_text())
            .unwrap_or_default()
    };
    identity(
        &tag(f::TITLE),
        &tag(f::ARTIST),
        &tag(f::ALBUM),
        &filetype_key(record),
    )
}

/// The file type as the record stores it — four bytes, one of which may be a
/// space, which is why it is not read as a filename.
fn filetype_key(record: &Chunk) -> String {
    String::from_utf8_lossy(record.bytes_at(M_FILETYPE, 4).unwrap_or_default()).into_owned()
}
///
/// Derived from the track id rather than drawn at random, so the same id always
/// produces the same name. `26^4` is 456 976, far past any iPod's library, and
/// [`Push::place`] checks for a clash anyway rather than trusting that.
fn stem_for(id: u32) -> String {
    let mut rest = id;
    let mut out = [b'A'; 4];
    for slot in out.iter_mut().rev() {
        *slot = b'A' + (rest % 26) as u8;
        rest /= 26;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An existing track of this file type, to clone.
fn template_for(tracks: &Chunk, code: [u8; 4]) -> Option<&Chunk> {
    tracks
        .children_of(id::MHIT)
        .find(|t| t.bytes_at(M_FILETYPE, 4) == Some(&code))
}

fn strings_for(info: &TrackInfo, location: &str) -> Vec<(u32, String)> {
    vec![
        (f::TITLE, info.title.clone()),
        (f::LOCATION, location.to_string()),
        (f::ALBUM, info.album.clone()),
        (f::ARTIST, info.artist.clone()),
        (f::GENRE, String::new()),
        (f::FILETYPE, "MPEG audio file".to_string()),
        (f::ALBUM_ARTIST, info.artist.clone()),
        (f::ARTIST_SORT, info.artist.clone()),
        (f::ALBUM_ARTIST_SORT, info.artist.clone()),
    ]
}

/// An empty string `mhod`: a 24-byte header and nothing after it.
fn new_string_mhod() -> Chunk {
    let mut chunk = Chunk::leaf(id::MHOD, vec![0u8; 12]);
    chunk.set_trailer(vec![0u8; 16]);
    chunk
}

/// Point an `mhod` at `text`, rebuilding the parts of it that carry anything.
///
/// The position is `1` and never `0`. With a zero the track still lists in the
/// menus and the device refuses to play it, which is the failure this line
/// exists to prevent.
fn write_string_mhod(chunk: &mut Chunk, field: u32, text: &str) {
    let payload: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();

    // Bytes 12..24 are the field number and two unknowns the device leaves at
    // zero. Bytes 24..40 follow the header rather than being part of it, because
    // a string record has no children — which is exactly what a chunk's trailer
    // is for. The value at 32 is the one the published table was wrong about: it
    // reads as a string encoding flag, and iTunes writes `1` there on a record
    // whose payload is UTF-16.
    let mut header = vec![0u8; 12];
    header[0..4].copy_from_slice(&field.to_le_bytes());

    let mut trailer = Vec::with_capacity(16 + payload.len());
    trailer.extend_from_slice(&1u32.to_le_bytes());
    trailer.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    trailer.extend_from_slice(&1u32.to_le_bytes());
    trailer.extend_from_slice(&0u32.to_le_bytes());
    trailer.extend_from_slice(&payload);

    chunk.set_header(header);
    chunk.set_trailer(trailer);
}

/// Copy `template`'s header, or `len` zero bytes when there is no template.
fn header_or_zeros(template: Option<&Chunk>, len: usize) -> Vec<u8> {
    template.map_or_else(|| vec![0u8; len], |c| c.header().to_vec())
}

/// Copy the database, sign the new one, and leave the old one recoverable.
///
/// One write for the whole push, and a copy of what was there first — but the
/// copy goes to the app's cache directory, **not beside the database on the
/// device**. That is a deliberate reversal: see [`Device::backup_of`] for why.
/// The database is the only thing standing between a device's 1169 tracks and a
/// device that shows none of them, so the safety net stays — it just lives where
/// it will be looked for.
pub fn commit(device: &Device, bytes: &[u8]) -> Result<(), String> {
    let path = device.database();
    // Check the signature against the key before touching anything. A file the
    // device will reject is indistinguishable from a successful push until the
    // user looks at the iPod and finds nothing on it, and the old database is
    // still here — so this is the cheapest moment to refuse.
    if !hash58::verify(device.guid, bytes) {
        return Err("the finished database does not carry this iPod's signature".into());
    }
    let backup = device.backup_of(&path);
    if let Some(dir) = backup.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if let Err(e) = std::fs::copy(&path, &backup) {
        return Err(format!("{}: {e}", backup.display()));
    }
    config::atomic_write_bytes(&path, bytes);
    log::info!(
        "wrote the iPod database at {} ({} bytes); the previous one is {}",
        path.display(),
        bytes.len(),
        backup.display()
    );
    Ok(())
}

// ── The job ───────────────────────────────────────────────────────────────────

/// A copy in progress, and its outcome.
///
/// On a thread, because an album over USB takes minutes and the window is the
/// one thing that must not stop. The shape is `TagReader`'s — spawn, drain an
/// mpsc, the reply arrives on the frame the app is already running — rather than
/// a new mechanism for one job.
#[derive(Default)]
pub struct Job {
    running: Option<String>,
    finished: Option<Result<Outcome, String>>,
    rx: Option<std::sync::mpsc::Receiver<Result<Outcome, String>>>,
    /// Bumped whenever a job finishes, so a cache that depends on the outside
    /// world knows to look again.
    ///
    /// The Library sidebar reads `/proc/self/mounts`, `/dev/disk/by-label` and
    /// three `udisksctl` processes **once per session** — six-odd syscalls and
    /// three subprocesses a frame if drawn inline. That is the right trade for
    /// data that changes about never, and the wrong one after a mount or an
    /// eject, which is the one entry in that list that does change in a session.
    /// A counter rather than a flag because "did anything change" is a question
    /// with one honest answer.
    epoch: u64,
}

impl Job {
    /// Something is being copied.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// How many jobs have finished this session. See [`Job::epoch`].
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The last outcome, kept so the pane can show it after the job ends.
    pub fn result(&self) -> Option<&Result<Outcome, String>> {
        self.finished.as_ref()
    }

    /// Start copying `tracks`. `playlist` names a playlist to create from them;
    /// without one they are only added.
    ///
    /// One at a time, and the refusal is the honest kind: a second job while
    /// one runs would race it over the same database, and the file that landed
    /// last would carry a signature over two edits neither of them knows about.
    pub fn start(
        &mut self,
        what: String,
        tracks: Vec<PathBuf>,
        playlist: Option<String>,
        aac_bps: Option<u32>,
    ) {
        self.spawn(what, move |device| match playlist {
            Some(name) => push_playlist(device, &tracks, &name, aac_bps).map(Outcome::Added),
            None => push_album(device, &tracks, aac_bps).map(Outcome::Added),
        });
    }

    /// Take `paths` **off** the device: their files, their records, their
    /// playlist entries, and any album left empty.
    ///
    /// On a thread and through the same one-at-a-time gate as a push, because it
    /// rewrites the same 1.9 MB database the push rewrites — two of these at
    /// once would race over it, and the signature would cover one of them.
    pub fn start_remove(&mut self, what: String, paths: Vec<PathBuf>) {
        self.spawn(what, move |device| {
            crate::ipod::remove::remove_paths(device, &paths).map(Outcome::Removed)
        });
    }

    fn spawn(
        &mut self,
        what: String,
        work: impl FnOnce(&crate::ipod::device::Device) -> Result<Outcome, String> + Send + 'static,
    ) {
        let Some(device) = self.claim(what) else {
            return;
        };
        let Some(device) = device else {
            self.finished = Some(Err(
                "no iPod is mounted. Connect it and mount it first.".into()
            ));
            return;
        };
        self.send(move || work(&device));
    }

    /// Mount `device`, so the Library can browse it.
    ///
    /// **On a thread, like every other job here**, because `udisksctl` goes
    /// through polkit: a password or a confirmation belongs on a thread that can
    /// wait, not on the one drawing the window.
    pub fn start_mount(&mut self, device: crate::ipod::device::Plugged) {
        if self.claim("mounting the iPod".into()).is_none() {
            return;
        }
        self.send(move || {
            let at = crate::ipod::device::mount(&device)?;
            Ok(Outcome::Note(format!("iPod mounted at {}", at.display())))
        });
    }

    /// Unmount and power off, so the cable is safe to pull.
    pub fn start_eject(&mut self, device: crate::ipod::device::Plugged) {
        if self.claim("ejecting the iPod".into()).is_none() {
            return;
        }
        self.send(move || {
            crate::ipod::device::eject(&device)?;
            Ok(Outcome::Note("iPod ejected — safe to unplug".into()))
        });
    }

    /// The one-at-a-time gate, plus the device a push needs.
    ///
    /// Two levels on purpose: `None` is "refused, and the reason is recorded",
    /// and `Some(None)` is "accepted, but there is no mounted device". **Mount
    /// and eject need no `Device` at all** — one is what makes a device and the
    /// other is what unmakes it — so a single gate that insisted on one could not
    /// start either.
    fn claim(&mut self, what: String) -> Option<Option<crate::ipod::device::Device>> {
        if self.busy() {
            self.finished = Some(Err(format!(
                "already {} — wait for it to finish",
                self.running.as_deref().unwrap_or_default()
            )));
            return None;
        }
        self.running = Some(what);
        Some(Some(crate::ipod::device::find()?))
    }

    /// Spawn the work and hand back the channel it will answer on.
    fn send(&mut self, work: impl FnOnce() -> Result<Outcome, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
    }

    /// Take the finished outcome, if there is one. Returns whether anything
    /// changed, so the caller can ask for a repaint.
    pub fn drain(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(outcome) => {
                self.finished = Some(outcome);
                self.rx = None;
                self.running = None;
                // Both endings bump it, including the panic below: a cache that
                // re-reads on every finished job costs one rescan, and a cache
                // that re-reads only on success misses the case where the job
                // failed *after* half-changing the outside world.
                self.epoch = self.epoch.wrapping_add(1);
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            // The thread is gone without answering, which it only does if it
            // panicked. Either way the job is over and the pane must stop
            // saying it is copying.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.finished = Some(Err("the copy stopped before it finished".into()));
                self.rx = None;
                self.running = None;
                self.epoch = self.epoch.wrapping_add(1);
                true
            }
        }
    }
}

/// Copy `tracks` to the device and return what happened.
///
/// Files in a format the device cannot play are named in [`Summary::refused`]
/// and the rest still go.
pub fn push_album(
    device: &Device,
    paths: &[PathBuf],
    aac_bps: Option<u32>,
) -> Result<Summary, String> {
    let mut push = Push::open(device)?;
    let mut summary = Summary::default();
    let mut added = Vec::new();
    for path in paths {
        if kind_of(path).is_none() {
            summary.refused.push(
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
            );
            continue;
        }
        added.push(push.add_track(path, aac_bps)?);
    }
    summary.already = push.already();
    summary.added = added.len() - summary.already;
    commit(device, &push.finish()?)?;
    Ok(summary)
}

/// Copy `paths` to the device and add them as a playlist called `name`.
pub fn push_playlist(
    device: &Device,
    paths: &[PathBuf],
    name: &str,
    aac_bps: Option<u32>,
) -> Result<Summary, String> {
    let mut push = Push::open(device)?;
    let mut summary = Summary::default();
    let mut added = Vec::new();
    for path in paths {
        if kind_of(path).is_none() {
            summary.refused.push(
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
            );
            continue;
        }
        added.push(push.add_track(path, aac_bps)?);
    }
    // The playlist wants the ids of the songs it names, including the ones that
    // were already on the device — a second push of the same tracks as a
    // playlist is exactly when a user has the tracks and wants them listed.
    push.add_playlist(name, &added)?;
    summary.already = push.already();
    summary.added = added.len() - summary.already;
    commit(device, &push.finish()?)?;
    Ok(summary)
}
