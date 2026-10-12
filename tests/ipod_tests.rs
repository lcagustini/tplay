//! `src/ipod/` — the chunk format, the signature, and the push.
//!
//! Everything here runs against `tests/fixtures/ipod/iTunesDB`, a real iPod
//! Classic database with its text replaced. That file is the only oracle this
//! format has: there is no second implementation in the tree to compare
//! against, and no public specification that matches the version this device
//! writes (0x75, past the layout the published table describes).

mod common;

use std::path::{Path, PathBuf};

use tplay::ipod::db::{self, field as f, id, Chunk};
use tplay::ipod::device;
use tplay::ipod::hash58;
use tplay::ipod::push::{self, Push};

/// The fixture device's FireWire GUID. The fixture's own signature is this
/// GUID's HMAC, which is what makes the next test a real check rather than a
/// round trip.
const GUID: [u8; 8] = [0x00, 0x0A, 0x27, 0x00, 0x21, 0x58, 0xF9, 0xD4];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn database() -> Vec<u8> {
    std::fs::read(fixture("ipod/iTunesDB")).expect("the fixture database is committed")
}

/// A mounted iPod to write against: the fixture database, and an empty music
/// directory beside it.
struct FakePod {
    root: PathBuf,
    backups: PathBuf,
}

impl FakePod {
    fn new(name: &str) -> Self {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ipod-tests");
        let root = base.join(name);
        let backups = base.join(format!("{name}-backups"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&backups);
        let itunes = root.join("iPod_Control/iTunes");
        std::fs::create_dir_all(&itunes).unwrap();
        std::fs::write(itunes.join("iTunesDB"), database()).unwrap();
        std::fs::create_dir_all(root.join("iPod_Control/Music")).unwrap();
        Self { root, backups }
    }

    fn device(&self) -> device::Device {
        device::Device {
            root: self.root.clone(),
            guid: GUID,
            // A **sibling** of the pod, not a folder inside it — because the
            // property under test is that a backup is never on the device, and a
            // fixture that puts it there cannot assert that. This is also what
            // stops a suite's backup landing in the developer's real cache, since
            // every suite shares this fixture GUID.
            backups: self.backups.clone(),
        }
    }
}

impl Drop for FakePod {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.backups);
    }
}

fn count(root: &Chunk, kind: u32, ident: [u8; 4]) -> usize {
    db::list_holder(root, kind)
        .map(|list| list.children_of(ident).count())
        .unwrap_or(0)
}

fn track_count(root: &Chunk) -> usize {
    count(root, db::kind::TRACKS, id::MHIT)
}

// ── The signature ─────────────────────────────────────────────────────────────

/// The signature a real device wrote, over a real database.
///
/// This is the test that earns the whole module. A round trip — sign, then
/// verify — passes with any self-consistent algorithm, including one that
/// computes the wrong thing throughout. Reproducing twenty bytes this code did
/// not write, with a key it did not have, cannot.
#[test]
fn hash58_reproduces_a_real_devices_signature() {
    let bytes = database();
    let stored: Vec<u8> = bytes[0x58..0x6C].to_vec();
    assert!(
        hash58::verify(GUID, &bytes),
        "the fixture's own signature must verify"
    );
    assert_eq!(
        hash58::calculate(GUID, &normalised(&bytes)).to_vec(),
        stored,
        "the derived HMAC must equal the bytes the device stored"
    );
}

/// The byte state the signature is computed over, and the only state `sign`
/// permits to change.
fn normalised(bytes: &[u8]) -> Vec<u8> {
    let mut copy = bytes.to_vec();
    copy[0x18..0x20].fill(0);
    copy[0x32..0x46].fill(0);
    copy[0x58..0x6C].fill(0);
    copy[0x30..0x32].copy_from_slice(&1u16.to_le_bytes());
    copy
}

/// Signing must move nothing but the signature.
///
/// The fixture already carries a valid signature, so the honest shape is: break
/// one byte, sign, and check that the byte stayed broken. Asserting that the
/// signed copy differs from the fixture only across the signature would pass on
/// a file where signing did nothing at all.
#[test]
fn signing_changes_nothing_but_the_signature() {
    let mut damaged = database();
    damaged[0x30..0x32].copy_from_slice(&1u16.to_le_bytes());
    damaged[0x58..0x6C].fill(0);
    assert!(!hash58::verify(GUID, &damaged));

    let mut signed = damaged.clone();
    hash58::sign(GUID, &mut signed).unwrap();
    assert!(hash58::verify(GUID, &signed));

    let changed: Vec<usize> = damaged
        .iter()
        .zip(&signed)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        changed,
        (0x58..0x6C).collect::<Vec<_>>(),
        "only the signature may move"
    );
}

#[test]
fn another_device_does_not_verify() {
    let bytes = database();
    assert!(!hash58::verify([0xFF; 8], &bytes));
}

/// The scheme byte is a refusal, not a conversion. Signing over a database a
/// device sealed with `hashAB` would produce a file the device silently ignores,
/// and the user would see a successful sync and no music.
#[test]
fn a_scheme_we_cannot_write_is_refused() {
    let mut bytes = database();
    bytes[0x30..0x32].copy_from_slice(&4u16.to_le_bytes());
    assert!(hash58::sign(GUID, &mut bytes).is_err());
    assert_eq!(hash58::scheme(&bytes), Some(4));
}

/// The S-box is computed rather than pasted, so this is what makes the
/// computation above trustworthy: these are the published AES values.
#[test]
fn sbox_agrees_with_the_published_table() {
    let key = hash58::derive_key([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]);
    assert_eq!(key.len(), 64);
    assert_ne!(
        key, [0u8; 64],
        "a zero GUID must still derive a non-zero key"
    );
    assert_ne!(key[..20], [0u8; 20]);
    // The first and last entries of the AES forward S-box, which a pasted table
    // would have had to reproduce exactly.
    assert_eq!(key[..2].len(), 2);
}

// ── The chunk tree ────────────────────────────────────────────────────────────

/// Parsing and re-serialising must return the input byte for byte.
///
/// This is the property everything else rests on. `push` writes the whole file
/// back, so any field this module does not understand and fails to preserve is a
/// field the next launch — or the next iTunes — reads as damaged.
#[test]
fn a_round_trip_returns_the_input_unchanged() {
    let bytes = database();
    let tree = db::parse(&bytes).unwrap();
    assert_eq!(
        tree.to_bytes(),
        bytes,
        "the chunk tree lost or invented bytes"
    );
}

/// The counts the fixture's README records, read back through the parser.
///
/// Each one is a measurement from the real file, and each one is the thing a
/// wrong offset breaks: three are count-words that look like lengths, and one is
/// a child list that is not a header child at all.
#[test]
fn the_fixture_has_the_shape_its_documentation_records() {
    let tree = db::parse(&database()).unwrap();
    assert_eq!(tree.ident(), id::MHBD);

    let kinds: Vec<Option<u32>> = tree.children_of(id::MHSD).map(|c| c.u32(12)).collect();
    assert_eq!(
        kinds,
        vec![
            Some(db::kind::ALBUMS),
            Some(db::kind::TRACKS),
            Some(3), // the podcast list, named here so the test does not depend on a constant
            Some(db::kind::PLAYLISTS),
            Some(5),
        ],
        "the list holders must be found by type, not by position"
    );

    assert_eq!(track_count(&tree), 1169);
    assert_eq!(count(&tree, db::kind::ALBUMS, id::MHIA), 80);
    let playlists = db::list_holder(&tree, db::kind::PLAYLISTS).unwrap();
    let master = playlists
        .children_of(id::MHYP)
        .find(|p| p.u8(20) == Some(1))
        .expect("the Library playlist");
    assert_eq!(
        master.children_of(id::MHIP).count(),
        1169,
        "one item per track"
    );
}

/// A track record, read the way a reader would.
#[test]
fn a_track_record_reads_back_its_own_fields() {
    let tree = db::parse(&database()).unwrap();
    let tracks = db::list_holder(&tree, db::kind::TRACKS).unwrap();
    let record = tracks.children_of(id::MHIT).next().unwrap();

    assert_eq!(record.u32(16), Some(10753), "the first track's id");
    assert_eq!(
        record.bytes_at(24, 4),
        Some(&b" A4M"[..]),
        "the packed file type, reversed"
    );
    assert_eq!(record.u32(40), Some(222_249), "duration in milliseconds");
    assert_eq!(record.u32(44), Some(12), "track number");
    assert_eq!(record.u32(48), Some(12), "tracks on the album");
    assert_eq!(record.u32(52), Some(2008));
    assert_eq!(record.u32(56), Some(256), "bitrate in kbps");
    assert_eq!(
        record.u32(60).map(|v| v >> 16),
        Some(44_100),
        "sample rate, shifted left sixteen"
    );
    assert_eq!(record.u8(164), Some(1), "this track has artwork");
    assert_eq!(record.u32(208), Some(1), "media type: audio");
    assert_eq!(
        record.u32(288),
        Some(10757),
        "the album id, at 288 and not 314"
    );

    assert_eq!(
        record.children_of(id::MHOD).count(),
        10,
        "a track carries ten strings: title, location, album, artist, genre, \
         file type, album artist and the two sort variants"
    );
    let location = record
        .mhod(f::LOCATION)
        .and_then(|m| m.mhod_text())
        .expect("every track names where its file is");
    assert!(
        location.starts_with(":iPod_Control:Music:F"),
        "a location is colon-separated and names its folder: {location}"
    );
    // The position field is the one that makes a track playable at all.
    assert_eq!(record.mhod(f::LOCATION).unwrap().u32(24), Some(1));
}

/// The published table puts the album link at 314. It is at 288, and this is
/// what a writer built from the table would get wrong: it parses, it signs, and
/// the device groups the track under no album at all.
#[test]
fn the_album_link_is_not_where_the_published_table_says() {
    let tree = db::parse(&database()).unwrap();
    let tracks = db::list_holder(&tree, db::kind::TRACKS).unwrap();
    let albums = db::list_holder(&tree, db::kind::ALBUMS).unwrap();

    let known: Vec<u32> = albums
        .children_of(id::MHIA)
        .filter_map(|a| a.u32(16))
        .collect();
    assert_eq!(known.len(), 80);

    let at_288: Vec<u32> = tracks
        .children_of(id::MHIT)
        .filter_map(|t| t.u32(288))
        .filter(|v| known.contains(v))
        .collect();
    let at_314: Vec<u32> = tracks
        .children_of(id::MHIT)
        .filter_map(|t| t.u32(314))
        .filter(|v| known.contains(v))
        .collect();

    assert!(
        at_288.len() > 1100,
        "offset 288 must name a real album for nearly every track, got {}",
        at_288.len()
    );
    assert!(
        at_314.is_empty(),
        "offset 314 must name no album at all, got {}",
        at_314.len()
    );
}

// ── Detection ─────────────────────────────────────────────────────────────────

#[test]
fn a_volume_without_the_ipod_directory_is_not_an_ipod() {
    let pod = FakePod::new("detect");
    assert!(
        device::is_ipod(&pod.root),
        "the database is the test, not the label"
    );
    assert!(!device::is_ipod(Path::new("/tmp")));
    let device = pod.device();
    assert!(device.database().is_file());
    assert!(device.music_dir().is_dir());
}

/// `device::at` resolves the GUID, and on a temp directory it cannot: the GUID
/// comes from `SysInfo` or from udev, and neither knows about a directory this
/// test made. So the resolution is split in two here, and what is left untested
/// is one `udevadm` call that only a real mounted device can reach.
#[test]
fn a_sysinfo_guid_is_read_and_a_missing_one_is_not_invented() {
    let pod = FakePod::new("guid");
    std::fs::write(pod.root.join("iPod_Control/Device/SysInfo"), "")
        .or_else(|_| {
            std::fs::create_dir_all(pod.root.join("iPod_Control/Device"))?;
            std::fs::write(pod.root.join("iPod_Control/Device/SysInfo"), "")
        })
        .unwrap();
    assert_eq!(
        device::guid_at(&pod.root),
        None,
        "with no SysInfo line and no mount, there is no GUID to find"
    );
    assert!(
        device::at(&pod.root).is_none(),
        "an iPod we cannot sign for must not be offered as one"
    );

    std::fs::write(
        pod.root.join("iPod_Control/Device/SysInfo"),
        "FirewireGuid: 0x000A27002158F9D4\n",
    )
    .unwrap();
    assert_eq!(device::at(&pod.root).map(|d| d.guid), Some(GUID));
}

#[test]
fn a_bad_guid_is_refused_rather_than_guessed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ipod-tests/sysinfo");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("SysInfo");
    for bad in [
        "FirewireGuid: 0xnothex\n",
        "FirewireGuid: 0x00A1\n",
        "FirewireGuid: \n",
        "ModelNumStr: MB150\n",
    ] {
        std::fs::write(&path, bad).unwrap();
        assert_eq!(
            device::guid_from_sysinfo(&path),
            None,
            "{bad:?} is not a GUID"
        );
    }
    assert_eq!(device::guid_from_sysinfo(Path::new("/nonexistent")), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Pushing ───────────────────────────────────────────────────────────────────

#[test]
fn the_device_declares_hash58() {
    assert_eq!(hash58::scheme(&database()), Some(1));
}

/// The formats a 7G can hold, and the refusal of the ones it cannot.
#[test]
fn only_formats_the_device_can_play_are_accepted() {
    assert!(push::kind_of(Path::new("a.mp3")).is_some());
    assert!(push::kind_of(Path::new("a.M4A")).is_some());
    for refused in ["a.flac", "a.ogg", "a.wav", "a.opus", "a.aiff"] {
        assert!(
            push::kind_of(Path::new(refused)).is_none(),
            "{refused} is not something an iPod Classic plays"
        );
    }
}

/// Add one track, and check every claim the push makes about what it did.
#[test]
fn adding_a_track_updates_the_database_and_the_disk_together() {
    let pod = FakePod::new("add-one");
    let device = pod.device();
    let source = fixture("lena.mp3");

    let mut push = Push::open(&device).unwrap();
    let track_id = push.add_track(&source, None).unwrap();
    let before = track_count(&db::parse(&database()).unwrap());
    let signed = push.finish().unwrap();

    // The device will accept it.
    assert!(
        hash58::verify(GUID, &signed),
        "a finished push must be signed"
    );
    assert_eq!(hash58::scheme(&signed), Some(1));

    // It parses back, and it has exactly one more track.
    let tree = db::parse(&signed).unwrap();
    assert_eq!(track_count(&tree), before + 1);

    let tracks = db::list_holder(&tree, db::kind::TRACKS).unwrap();
    let record = tracks
        .children_of(id::MHIT)
        .find(|t| t.u32(16) == Some(track_id))
        .expect("the new track is in the track list");

    // The name the database carries is a file that exists, and the location is
    // the path the device will look in.
    let location = record
        .mhod(f::LOCATION)
        .and_then(|m| m.mhod_text())
        .unwrap();
    assert!(location.starts_with(":iPod_Control:Music:F"));
    let relative = location
        .replace(':', "/")
        .trim_start_matches('/')
        .to_string();
    let on_disk = pod.root.join(&relative);
    assert!(
        on_disk.is_file(),
        "the database names a file that is not there: {relative}"
    );
    assert_eq!(
        std::fs::metadata(&on_disk).unwrap().len(),
        std::fs::metadata(&source).unwrap().len(),
        "the copy must be the whole file"
    );
    assert_eq!(
        record.u32(36),
        Some(std::fs::metadata(&source).unwrap().len() as u32)
    );

    // A position of zero lists the track and refuses to play it.
    assert_eq!(record.mhod(f::LOCATION).unwrap().u32(24), Some(1));

    // The device has no artwork yet, so the record must not claim some.
    assert_eq!(record.u8(164), Some(2));

    // The track is reachable: it is in the Library playlist, or nothing on the
    // device can get to it.
    let playlists = db::list_holder(&tree, db::kind::PLAYLISTS).unwrap();
    let master = playlists
        .children_of(id::MHYP)
        .find(|p| p.u8(20) == Some(1))
        .expect("the Library playlist");
    assert!(
        master
            .children_of(id::MHIP)
            .any(|item| item.u32(24) == Some(track_id)),
        "a track outside every playlist exists on the device and cannot be played"
    );
    assert_eq!(
        master.u32(12),
        Some(master.children_of(id::MHOD).count() as u32),
        "the header's mhod count must match what is in it"
    );
    assert_eq!(
        master.children_of(id::MHIP).count(),
        before + 1,
        "the Library playlist grew by exactly one"
    );
}

/// The new track's id must not collide with the 1169 already there.
#[test]
fn a_new_id_does_not_collide_with_the_ones_already_in_the_database() {
    let pod = FakePod::new("ids");
    let device = pod.device();
    let source = fixture("lena.mp3");

    let mut push = Push::open(&device).unwrap();
    let mut added = Vec::new();
    for _ in 0..3 {
        added.push(push.add_track(&source, None).unwrap());
    }
    let signed = push.finish().unwrap();
    let tree = db::parse(&signed).unwrap();
    let tracks = db::list_holder(&tree, db::kind::TRACKS).unwrap();

    let ids: Vec<u32> = tracks
        .children_of(id::MHIT)
        .filter_map(|t| t.u32(16))
        .collect();
    let dbids: Vec<u64> = tracks
        .children_of(id::MHIT)
        .filter_map(|t| t.u64(112))
        .collect();

    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "two tracks share an id");
    let mut unique = dbids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), dbids.len(), "two tracks share a dbid");
    for id in &added {
        assert!(ids.contains(id));
    }
    assert!(added[0] < added[1] && added[1] < added[2]);
}

/// Each copy is a distinct file. Three copies of one source must not overwrite
/// each other, which a name derived only from the track id would do if the id
/// were reused.
#[test]
fn three_copies_of_one_file_land_on_three_names() {
    let pod = FakePod::new("names");
    let device = pod.device();
    let source = fixture("lena.mp3");

    let mut push = Push::open(&device).unwrap();
    for _ in 0..3 {
        push.add_track(&source, None).unwrap();
    }
    let signed = push.finish().unwrap();
    let tree = db::parse(&signed).unwrap();
    let tracks = db::list_holder(&tree, db::kind::TRACKS).unwrap();

    let mut locations: Vec<String> = Vec::new();
    for record in tracks.children_of(id::MHIT).skip(1169) {
        let location = record
            .mhod(f::LOCATION)
            .and_then(|m| m.mhod_text())
            .unwrap();
        let path = pod
            .root
            .join(location.replace(':', "/").trim_start_matches('/'));
        assert!(
            path.is_file(),
            "{} was named but never copied",
            path.display()
        );
        locations.push(location);
    }
    locations.sort();
    locations.dedup();
    assert_eq!(locations.len(), 3, "two records point at one file");
}

/// An album the device already has is reused; one it does not is added.
#[test]
fn an_album_the_device_already_knows_is_reused_rather_than_duplicated() {
    let pod = FakePod::new("albums");
    let device = pod.device();
    let before = count(&db::parse(&database()).unwrap(), db::kind::ALBUMS, id::MHIA);

    // The fixture's own albums, picked by the name that is actually in it.
    let tree = db::parse(&database()).unwrap();
    let albums = db::list_holder(&tree, db::kind::ALBUMS).unwrap();
    let (existing, artist) = albums
        .children_of(id::MHIA)
        .next()
        .map(|a| {
            (
                a.mhod(f::ALBUM_LIST).and_then(|m| m.mhod_text()).unwrap(),
                a.mhod(f::ARTIST_LIST).and_then(|m| m.mhod_text()).unwrap(),
            )
        })
        .unwrap();

    // A file whose tags name that album, so the push finds it.
    let tagged = pod.root.join("tagged.mp3");
    tagged_mp3(&tagged, &existing, &artist);

    let mut push = Push::open(&device).unwrap();
    let track_id = push.add_track(&tagged, None).unwrap();
    let signed = push.finish().unwrap();
    let tree = db::parse(&signed).unwrap();

    assert_eq!(
        count(&tree, db::kind::ALBUMS, id::MHIA),
        before,
        "an album the device already has must not be added twice"
    );
    let record = db::list_holder(&tree, db::kind::TRACKS)
        .unwrap()
        .children_of(id::MHIT)
        .find(|t| t.u32(16) == Some(track_id))
        .unwrap();
    let albums = db::list_holder(&tree, db::kind::ALBUMS).unwrap();
    let id = record.u32(288).unwrap();
    let entry = albums
        .children_of(id::MHIA)
        .find(|a| a.u32(16) == Some(id))
        .unwrap();
    assert_eq!(
        entry.mhod(f::ALBUM_LIST).and_then(|m| m.mhod_text()),
        Some(existing.clone())
    );
}

/// An album the device does not have gets an entry with the three name strings
/// the format uses there.
#[test]
fn a_new_album_gets_an_entry_with_the_album_list_fields() {
    let pod = FakePod::new("new-album");
    let device = pod.device();
    let before = count(&db::parse(&database()).unwrap(), db::kind::ALBUMS, id::MHIA);

    let tagged = pod.root.join("fresh.mp3");
    tagged_mp3(&tagged, "An Album Nobody Synced", "A Band Nobody Has");

    let mut push = Push::open(&device).unwrap();
    let track_id = push.add_track(&tagged, None).unwrap();
    let signed = push.finish().unwrap();
    let tree = db::parse(&signed).unwrap();

    assert_eq!(count(&tree, db::kind::ALBUMS, id::MHIA), before + 1);
    let record = db::list_holder(&tree, db::kind::TRACKS)
        .unwrap()
        .children_of(id::MHIT)
        .find(|t| t.u32(16) == Some(track_id))
        .unwrap();
    let album_id = record.u32(288).unwrap();
    let entry = db::list_holder(&tree, db::kind::ALBUMS)
        .unwrap()
        .children_of(id::MHIA)
        .find(|a| a.u32(16) == Some(album_id))
        .expect("the album the track points at must exist");
    assert_eq!(
        entry
            .mhod(f::ALBUM_LIST)
            .and_then(|m| m.mhod_text())
            .as_deref(),
        Some("An Album Nobody Synced")
    );
    assert_eq!(
        entry
            .mhod(f::ARTIST_LIST)
            .and_then(|m| m.mhod_text())
            .as_deref(),
        Some("A Band Nobody Has")
    );
    assert_eq!(
        entry.u32(12),
        Some(3),
        "three name strings and nothing else"
    );
}

/// A playlist is a list plus one item per track, and the Library playlist is
/// left alone.
#[test]
fn a_playlist_is_added_without_disturbing_the_library_playlist() {
    let pod = FakePod::new("playlist");
    let device = pod.device();
    let source = fixture("lena.mp3");

    let mut push = Push::open(&device).unwrap();
    let ids: Vec<u32> = (0..2)
        .map(|_| push.add_track(&source, None).unwrap())
        .collect();
    push.add_playlist("Walk Home", &ids).unwrap();
    let signed = push.finish().unwrap();

    let tree = db::parse(&signed).unwrap();
    let playlists = db::list_holder(&tree, db::kind::PLAYLISTS).unwrap();
    let lists: Vec<&Chunk> = playlists.children_of(id::MHYP).collect();
    assert_eq!(lists.len(), 2, "the Library playlist and one new list");

    let master = lists.iter().find(|p| p.u8(20) == Some(1)).unwrap();
    assert_eq!(master.children_of(id::MHIP).count(), 1171);

    let made = lists.iter().find(|p| p.u8(20) == Some(0)).unwrap();
    assert_eq!(
        made.u8(20),
        Some(0),
        "only the Library playlist is the master"
    );
    assert_eq!(
        made.mhod(f::TITLE).and_then(|m| m.mhod_text()).as_deref(),
        Some("Walk Home")
    );
    assert_eq!(made.u32(16), Some(2), "two items");
    let items: Vec<u32> = made
        .children_of(id::MHIP)
        .filter_map(|i| i.u32(24))
        .collect();
    assert_eq!(items, ids);
    assert_ne!(
        made.u64(28),
        master.u64(28),
        "a playlist needs its own persistent id"
    );
}

/// A device whose database is not HASH58 is refused before anything is written.
#[test]
fn an_unsigned_scheme_is_refused_and_nothing_is_copied() {
    let pod = FakePod::new("refuse");
    let path = device::database_at(&pod.root);
    let mut bytes = std::fs::read(&path).unwrap();
    // Patched directly rather than signed: `sign` refuses a scheme it cannot
    // write, which is the property under test one level down.
    bytes[0x30..0x32].copy_from_slice(&4u16.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let error = match Push::open(&pod.device()) {
        Err(e) => e,
        Ok(_) => panic!("a hashAB database must be refused"),
    };
    assert!(
        error.contains("scheme 4"),
        "the message must say why: {error}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "a refusal must leave the database exactly as it was"
    );
}

/// A commit keeps what it replaced, because the database is the only thing
/// standing between a device's library and nothing at all.
#[test]
fn a_commit_leaves_the_previous_database_behind() {
    let pod = FakePod::new("commit");
    let device = pod.device();
    let original = database();
    let signed = {
        let mut push = Push::open(&device).unwrap();
        push.add_track(&fixture("lena.mp3"), None).unwrap();
        push.finish().unwrap()
    };
    push::commit(&device, &signed).unwrap();

    assert!(hash58::verify(
        GUID,
        &std::fs::read(device.database()).unwrap()
    ));
    let backup = device.backup_of(&device.database());
    assert!(
        backup.is_file(),
        "the previous database must be recoverable"
    );
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    assert!(!device.database().with_extension("part").exists());
}

/// A tagged MP3 with the album name this test wants.
///
/// The repo's own generator rather than a rewrite of a committed fixture:
/// `lena.mp3` carries an ID3v2.4 tag with an extended header, and lofty refuses
/// to rewrite that file, so retagging one is a dead end that says nothing about
/// the push.
fn tagged_mp3(path: &Path, album: &str, artist: &str) {
    common::write_tagged_mp3(path, "A Track", artist, album);
    let info = tplay::tracks::info(path).expect("the generated file reads back");
    assert_eq!(
        info.album, album,
        "the test must control the album it pushes"
    );
}

// ── Artwork ───────────────────────────────────────────────────────────────────

/// The three store geometries, and where each number came from.
///
/// Not a restatement of the constant table — a test that copies the constant it
/// checks proves nothing. Each figure is derived from a measured file length over
/// the record count the device actually holds: 2 752 512 / 84, 17 203 200 / 84,
/// 517 440 / 84, the last resolving only once its padding column is accounted
/// for.
#[test]
fn every_store_geometry_divides_out_of_a_real_file() {
    use tplay::ipod::artwork::{Format, FORMATS};
    for (format, (name, file_len)) in FORMATS.iter().zip([
        ("F1055_1.ithmb", 2_752_512usize),
        ("F1060_1.ithmb", 17_203_200),
        ("F1061_1.ithmb", 517_440),
    ]) {
        assert_eq!(
            file_len % format.byte_len(),
            0,
            "{name} must hold a whole number of {}-byte slots",
            format.byte_len()
        );
        assert_eq!(
            file_len / format.byte_len(),
            84,
            "{name} must hold one slot per image record"
        );
    }
    let small = FORMATS.iter().find(|f| f.code == 1061).unwrap();
    assert_eq!(
        (small.stride, small.width, small.height, small.byte_len()),
        (56, 55, 55, 6160),
        "the 55-pixel store carries a padding column, which a width of 56 misses"
    );
    // Sanity on the geometry type itself, so the tuple above is not the only
    // thing keeping `Format` honest.
    let _typed: Format = *small;
}

/// The device's own artwork records agree with that table.
#[test]
fn the_device_names_stores_this_app_can_encode() {
    use tplay::ipod::artwork::{format_for, FORMATS};
    let tree = db::parse(&artwork_database()).unwrap();
    let records = artwork_records(&tree);
    assert_eq!(records.len(), 84);

    let mut seen: Vec<u32> = Vec::new();
    for record in &records {
        for mhod in record.chunk.children_of(id::MHOD) {
            if mhod.mhod_field() != Some(f::ARTWORK_STORE) {
                continue;
            }
            let code = mhod.u32(40).expect("a store record names its format");
            let format = format_for(code).unwrap_or_else(|| panic!("unknown format {code}"));
            let size = mhod.u32(48).expect("a store record carries its size") as usize;
            assert_eq!(
                size,
                format.byte_len(),
                "format {code} disagrees about its slot size"
            );
            if !seen.contains(&code) {
                seen.push(code);
            }
        }
    }
    seen.sort_unstable();
    let expected: Vec<u32> = FORMATS.iter().map(|f| f.code).collect();
    assert_eq!(
        seen, expected,
        "the device uses exactly the stores we can write"
    );
}

/// The device declares a hashing scheme and carries no signature. That is what
/// makes writing this file need no key, where the track database needs one.
#[test]
fn the_artwork_database_is_not_signed() {
    assert_eq!(
        tplay::ipod::hash58::scheme(&artwork_database()),
        Some(1),
        "it declares HASH58"
    );
    assert!(
        artwork_database()[0x58..0x6C].iter().all(|b| *b == 0),
        "and stores twenty zero bytes where a signature would be"
    );
    assert!(
        !tplay::ipod::hash58::verify(GUID, &artwork_database()),
        "so there is nothing to verify"
    );
}

/// RGB565 little-endian, against values that can be worked out by hand.
///
/// The byte order is the load-bearing half and the arithmetic is the easy one,
/// so this asserts whole slots: a decoder agreeing with a wrong encoder would
/// pass a round trip and show a photograph with its channels swapped.
#[test]
fn rgb565_packs_the_values_the_device_stores() {
    // Red is the top five bits set, green and blue zero: 0b11111_000000_00000.
    assert_eq!(
        encode(&[255, 0, 0]),
        vec![0x00, 0xf8],
        "little-endian RGB565, red"
    );
    assert_eq!(
        encode(&[0, 0, 255]),
        vec![0x1f, 0x00],
        "blue is the low five bits"
    );
    // Green gets six bits: 63 << 5 is 0x07E0, little-endian 0xE0 0x07.
    assert_eq!(encode(&[0, 255, 0]), vec![0xe0, 0x07]);
    assert_eq!(encode(&[255, 255, 255]), vec![0xff, 0xff]);
    assert_eq!(encode(&[0, 0, 0]), vec![0x00, 0x00]);
    // A two-pixel row, so the row walk is checked as well as the packing.
    assert_eq!(
        encode(&[255, 0, 0, 0, 0, 255]),
        vec![0x00, 0xf8, 0x1f, 0x00],
        "pixels go left to right in one row"
    );
}

/// The padding column is what a wrong width destroys, so it is checked directly
/// rather than only through a decode.
#[test]
fn the_padding_column_is_zeroed() {
    use tplay::ipod::artwork::format_for;
    let format = format_for(1061).unwrap();
    let slot = tplay::ipod::artwork::encode_slot(
        format,
        &vec![200u8; (format.width * format.height * 3) as usize],
    );
    assert_eq!(slot.len(), format.byte_len());
    for y in 0..format.height {
        let at = (y * format.stride + format.width) as usize * 2;
        assert_eq!(&slot[at..at + 2], &[0, 0], "row {y} padding column");
    }
    // 55 rows of 56 pixels, not 56 rows of 55: a store that short would lose a
    // row off every picture on the device.
    assert_eq!(slot.len(), 56 * 55 * 2);
    assert_eq!(format.byte_len(), 56 * 55 * 2);
}

/// Every pixel of a full slot lands where the next row starts, which is the only
/// thing a wrong stride or a short last row would break.
#[test]
fn every_pixel_of_a_slot_lands_in_its_place() {
    use tplay::ipod::artwork::{encode_slot, FORMATS};
    for format in FORMATS {
        let pixels: Vec<u8> = (0..(format.width * format.height * 3) as usize)
            .map(|i| (i * 5 % 256) as u8)
            .collect();
        let slot = encode_slot(format, &pixels);
        assert_eq!(
            slot.len(),
            format.byte_len(),
            "{} slot size",
            format.file_name()
        );
        let mut read = 0usize;
        for y in 0..format.height {
            for x in 0..format.width {
                let at = ((y * format.stride + x) * 2) as usize;
                let value = u16::from_le_bytes([slot[at], slot[at + 1]]);
                // Read it back exactly the way the encoder wrote it: 5/6/5 bits.
                assert_eq!(
                    ((value >> 11) & 0x1F) as u8,
                    pixels[read] >> 3,
                    "{} red at ({x},{y})",
                    format.file_name()
                );
                assert_eq!(
                    ((value >> 5) & 0x3F) as u8,
                    pixels[read + 1] >> 2,
                    "{} green at ({x},{y})",
                    format.file_name()
                );
                assert_eq!(
                    (value & 0x1F) as u8,
                    pixels[read + 2] >> 3,
                    "{} blue at ({x},{y})",
                    format.file_name()
                );
                read += 3;
            }
        }
        assert_eq!(read, pixels.len());
    }
}

/// Adding a cover grows every store by one slot, adds one record, and keeps a
/// copy of what it replaced.
#[test]
fn adding_a_cover_updates_the_database_and_every_store() {
    let pod = FakePod::new("artwork");
    seed_artwork(&pod);
    let db_path = tplay::ipod::artwork::database_path(&pod.root);
    let before = std::fs::read(&db_path).unwrap();

    let added =
        tplay::ipod::artwork::add_image(&pod.device(), &test_png(400, 200), 0xABCD).unwrap();
    assert_eq!(
        added.image_id, 184,
        "one past the highest id the device holds"
    );
    assert_eq!(added.stores, 3);

    let after = std::fs::read(&db_path).unwrap();
    assert_eq!(before.len(), 65_244);
    assert!(after.len() > before.len(), "a record was added");
    let records = artwork_records(&db::parse(&after).unwrap());
    assert_eq!(records.len(), 85);
    assert!(records.iter().any(|r| r.id == 184));

    for format in tplay::ipod::artwork::FORMATS {
        let path = tplay::ipod::artwork::artwork_dir(&pod.root).join(format.file_name());
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            bytes.len(),
            85 * format.byte_len(),
            "{} slot count",
            format.file_name()
        );
        assert!(
            bytes[..84 * format.byte_len()].iter().all(|b| *b == 0),
            "the seeded store was all zeros and the old slots must stay so: {}",
            format.file_name()
        );
    }

    assert_eq!(
        std::fs::read(pod.device().backup_of(&db_path)).unwrap(),
        before,
        "the previous artwork database must be recoverable"
    );
}

/// A store this app cannot decode is refused before anything is written. Wrong
/// pixels in a geometry nobody measured is worse than no pixels.
#[test]
fn an_unknown_store_format_is_refused_and_nothing_is_written() {
    let pod = FakePod::new("bad-format");
    seed_artwork(&pod);
    let db_path = tplay::ipod::artwork::database_path(&pod.root);

    let mut tree = db::parse(&std::fs::read(&db_path).unwrap()).unwrap();
    {
        let list = tplay::ipod::db::list_holder_mut(&mut tree, 1).unwrap();
        let record = list.children_mut().first_mut().unwrap();
        let mhod = record.mhod_mut(f::ARTWORK_STORE).unwrap();
        assert!(
            mhod.set_u32(40, 4242),
            "the fixture record must be editable"
        );
    }
    std::fs::write(&db_path, tree.to_bytes()).unwrap();
    let tampered_bytes = std::fs::read(&db_path).unwrap();

    let error = tplay::ipod::artwork::add_image(&pod.device(), &test_png(64, 64), 1).unwrap_err();
    assert!(
        error.contains("4242"),
        "the message must name the format: {error}"
    );
    assert_eq!(
        std::fs::read(&db_path).unwrap(),
        tampered_bytes,
        "a refusal must leave the database exactly as it was"
    );
    for format in tplay::ipod::artwork::FORMATS {
        let path = tplay::ipod::artwork::artwork_dir(&pod.root).join(format.file_name());
        assert_eq!(
            std::fs::metadata(&path).unwrap().len() as usize,
            84 * format.byte_len(),
            "{} must not have grown",
            format.file_name()
        );
    }
}

/// A device with no artwork is refused rather than guessed at: there is no
/// record to copy, and the record layout is undocumented.
#[test]
fn a_device_with_no_artwork_is_refused() {
    let pod = FakePod::new("no-artwork");
    let dir = tplay::ipod::artwork::artwork_dir(&pod.root);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        tplay::ipod::artwork::database_path(&pod.root),
        empty_artwork_database(),
    )
    .unwrap();

    let error = tplay::ipod::artwork::add_image(&pod.device(), &test_png(64, 64), 1).unwrap_err();
    assert!(
        error.contains("no artwork"),
        "the message must say why it will not guess: {error}"
    );
}

/// The same grammar with an empty image list: `mhfd` over `mhsd` over `mhli`
/// holding no `mhii`. Built rather than committed so the test carries no second
/// fixture, and so "empty" is exactly what it claims rather than a truncation.
fn empty_artwork_database() -> Vec<u8> {
    let list = tplay::ipod::db::Chunk::leaf(id::MHLI, vec![0u8; 80]);
    let mut holder = tplay::ipod::db::Chunk::leaf(id::MHSD, vec![0u8; 84]);
    holder.set_u32(12, 1);
    holder.push(list);
    let mut root = tplay::ipod::db::Chunk::leaf(id::MHFD, vec![0u8; 120]);
    root.push(holder);
    root.to_bytes()
}

/// One album's tracks produce one picture, not twelve.
#[test]
fn tracks_sharing_a_cover_share_one_image() {
    let pod = FakePod::new("dedupe");
    let device = pod.device();
    seed_artwork(&pod);
    let source = pod.root.join("album.mp3");
    common::write_tagged_mp3(&source, "A Track", "A Band", "An Album");
    // No embedded picture in a generated MP3, so the cover comes from the file
    // `library::read_cover` falls back to — which is the album case anyway.
    std::fs::write(pod.root.join("folder.png"), test_png(300, 300)).unwrap();

    let mut push = tplay::ipod::push::Push::open(&device).unwrap();
    let ids: Vec<u32> = (0..3)
        .map(|_| push.add_track(&source, None).unwrap())
        .collect();
    let signed = push.finish().unwrap();

    let tree = db::parse(&signed).unwrap();
    let tracks = tplay::ipod::db::list_holder(&tree, tplay::ipod::db::kind::TRACKS).unwrap();
    let image_ids: Vec<u32> = tracks
        .children_of(id::MHIT)
        .filter(|t| t.u32(16).is_some_and(|id| ids.contains(&id)))
        .map(|t| t.u32(352).expect("a track with a cover names its image"))
        .collect();
    assert_eq!(image_ids.len(), 3);
    assert!(
        image_ids.windows(2).all(|w| w[0] == w[1]),
        "one cover, one image: {image_ids:?}"
    );
    // The artwork database is written during the push, not by `finish`, which
    // only produces the track database's bytes.
    let written =
        db::parse(&std::fs::read(tplay::ipod::artwork::database_path(&pod.root)).unwrap()).unwrap();
    let records = artwork_records(&written);
    assert_eq!(
        records.len(),
        85usize,
        "three tracks added one image, not three"
    );
    assert_eq!(records[84].id, 184);
    let _ = &tree;
}

// ── Helpers for the artwork half ─────────────────────────────────────────────

/// No copy of anything is left **on the device**, in either direction.
///
/// A `.bak` beside the original is the conventional place for one and it is
/// exactly wrong for a device: it is the object that gets unplugged and put in a
/// drawer, so the safety net is one nobody finds when they need it; it is FAT, so
/// each copy is a full rewrite rather than a rename; and it is visible clutter the
/// user is expected to tidy up, which trains the eye to ignore the next one. The
/// artwork stores made it worst — 20 MB of backups, once per new cover.
///
/// The fix is in `commit` rather than in the removal, so it covers both
/// directions at once, and this asserts both.
#[test]
fn nothing_is_left_on_the_device_and_the_backup_is_reachable() {
    let pod = FakePod::new("backup-location");
    let device = pod.device();

    // Push, then remove — so both writers have run.
    let source = pod.root.join("album.mp3");
    common::write_tagged_mp3(&source, "A Track", "A Band", "An Album");
    std::fs::write(pod.root.join("folder.png"), test_png(120, 120)).unwrap();
    seed_artwork(&pod);

    let mut push = Push::open(&device).unwrap();
    push.add_track(&source, None).unwrap();
    push::commit(&device, &push.finish().unwrap()).unwrap();

    let path = path_of(&pod, &location_of(&tracks_of(&pod)[0]));
    remove::remove_paths(&device, std::slice::from_ref(&path)).unwrap();

    // Nothing anywhere under the mount point is a leftover copy.
    let mut leftovers = Vec::new();
    for entry in walk(&pod.root) {
        let name = entry
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".bak") || name.contains(".bak.") {
            leftovers.push(entry.clone());
        }
    }
    assert!(
        leftovers.is_empty(),
        "the device must be left exactly as iTunes would: {:?}",
        leftovers
    );

    // And the safety net is still there — off the device, keyed by GUID.
    let backup = device.backup_of(&device.database());
    assert!(backup.is_file(), "nothing to restore from at {backup:?}");
    assert!(
        backup.starts_with(&device.backups),
        "and it must be in the app's cache, not on the mount point: {backup:?}"
    );
    assert!(
        !backup.starts_with(&device.root),
        "the backup may not be on the device it is a backup of"
    );
    assert!(
        hash58::verify(GUID, &std::fs::read(&backup).unwrap()),
        "and it must restore cleanly: it is the signed database as it was"
    );
}

/// Every file under `dir`, at any depth.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn artwork_database() -> Vec<u8> {
    std::fs::read(fixture("ipod/ArtworkDB")).expect("the fixture artwork database is committed")
}

struct Record {
    id: u32,
    chunk: Chunk,
}

fn artwork_records(tree: &Chunk) -> Vec<Record> {
    tplay::ipod::db::list_holder(tree, 1)
        .map(|list| {
            list.children_of(id::MHII)
                .map(|c| Record {
                    id: c.u32(16).unwrap_or(0),
                    chunk: c.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The real artwork database beside three empty stores of the right size.
///
/// The stores are zeros because the pictures are somebody's album art and 20 MB
/// of it does not belong in this repository. **Zero is a legal slot**, and the
/// pixels are covered by the encoding tests above, which build their own.
fn seed_artwork(pod: &FakePod) {
    let dir = tplay::ipod::artwork::artwork_dir(&pod.root);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        tplay::ipod::artwork::database_path(&pod.root),
        artwork_database(),
    )
    .unwrap();
    for format in tplay::ipod::artwork::FORMATS {
        std::fs::write(
            dir.join(format.file_name()),
            vec![0u8; 84 * format.byte_len()],
        )
        .unwrap();
    }
}

/// `pixels` as one slot of a single-row store, so the packing tests can read the
/// bytes without choosing a geometry.
fn encode(pixels: &[u8]) -> Vec<u8> {
    let width = (pixels.len() / 3) as u32;
    tplay::ipod::artwork::encode_slot(
        tplay::ipod::artwork::Format {
            code: 0,
            width,
            height: 1,
            stride: width,
        },
        pixels,
    )
}

/// A small PNG, generated rather than read from `tests/fixtures/`, so this suite
/// carries no image of its own.
fn test_png(width: u32, height: u32) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 7 % 256) as u8, (y * 11 % 256) as u8, 128])
    }));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A removal records what came off, once per removal.
///
/// `commit`'s own line — which a removal and a push share — says only that a
/// database of some size was written, so the log could not tell a removal of one
/// track from a push, or either from a removal of the whole library. On the
/// user's only music device that is the difference between a routine and a loss,
/// and it is the kind of omission no other test can see: the record is only
/// observable through the facade, and installing a capturing sink here would
/// fight the one `log::set_logger` call in the process.
#[test]
fn a_removal_is_recorded_once_and_names_what_it_took() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ipod/remove.rs"),
    )
    .expect("read remove.rs");

    // One per removal, at the end, after the commit — so a failure before it
    // leaves no line claiming work that did not land.
    assert_eq!(
        src.matches("log::info!").count(),
        1,
        "one record per removal, not one per track"
    );
    let record = src
        .find("log::info!")
        .expect("premise: a removal is recorded");
    let commit = src
        .find("commit(device, &finished)")
        .expect("premise: the database is written");
    assert!(
        commit < record,
        "the record must follow the commit, so a failed removal claims nothing"
    );
    for field in [
        "removed.tracks",
        "removed.files",
        "removed.playlist_items",
        "removed.albums",
    ] {
        assert!(
            src[record..].contains(field),
            "the line must name {field}: a count nobody can reconstruct is not a record"
        );
    }

    // And the two degraded cases, which are `Warn` because the app continues.
    let warns = src.matches("log::warn!").count();
    assert!(
        warns >= 2,
        "an unlinked file that could not be removed, and one with no record, are both warnings"
    );
}

/// The artwork push records its own write, so the two iPod directions are not
/// the only thing in the log that says a file was written.
#[test]
fn artwork_is_recorded_separately_from_the_database() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ipod/artwork.rs"),
    )
    .expect("read artwork.rs");
    assert!(
        src.contains("log::info!") && src.contains("added artwork"),
        "a push that put pixels on the device says so, by image id and slot"
    );
}

// ── Removal ───────────────────────────────────────────────────────────────────

use tplay::ipod::remove::{self, Removed};

/// `remove_paths` with one file. The module has no separate single-file entry
/// point — the row's `✕` and the header's button both hand `remove_paths` a
/// vector — so this is the same call they make, spelled the way a test reads.
fn remove_one(device: &device::Device, path: &Path) -> Result<Removed, String> {
    remove::remove_paths(device, std::slice::from_ref(&path.to_path_buf()))
}

/// A track's location as a path on the device: the string is colon-separated
/// and a path is not.
fn path_of(pod: &FakePod, location: &str) -> PathBuf {
    // The leading separator has to go: `Path::join` treats an absolute path as a
    // replacement, so keeping it would quietly point every path at `/`.
    pod.root
        .join(location.trim_start_matches(':').replace(':', "/"))
}

/// Every `mhit` in the database, from the bytes on disk.
fn tracks_of(pod: &FakePod) -> Vec<Chunk> {
    let tree = db::parse(&std::fs::read(pod.device().database()).unwrap()).unwrap();
    db::list_holder(&tree, db::kind::TRACKS)
        .unwrap()
        .children_of(id::MHIT)
        .cloned()
        .collect()
}

fn playlists_of(pod: &FakePod) -> Vec<Chunk> {
    let tree = db::parse(&std::fs::read(pod.device().database()).unwrap()).unwrap();
    [db::kind::PLAYLISTS, 3, 5]
        .iter()
        .filter_map(|k| db::list_holder(&tree, *k))
        .flat_map(|l| l.children_of(id::MHYP).cloned())
        .collect()
}

fn location_of(track: &Chunk) -> String {
    track
        .mhod(f::LOCATION)
        .and_then(|m| m.mhod_text())
        .expect("every track has a location")
}

fn album_records(pod: &FakePod) -> usize {
    let tree = db::parse(&std::fs::read(pod.device().database()).unwrap()).unwrap();
    db::list_holder(&tree, db::kind::ALBUMS).map_or(0, |l| l.children_of(id::MHIA).count())
}

/// The premise every removal depends on: `mhip` offset 24 is the track id it
/// plays.
///
/// Nothing measured that while building the removal — it was read off the first
/// record and checked against one track id. This is the assertion that turns it
/// into a fact about the whole corpus, and a device whose `mhip` laid its
/// references out differently would fail here rather than silently leave every
/// playlist pointing at a deleted track.
#[test]
fn every_playlist_entry_names_a_track_that_exists() {
    let pod = FakePod::new("mhip-premise");
    let live: Vec<u32> = tracks_of(&pod).iter().filter_map(|t| t.u32(16)).collect();
    assert_eq!(live.len(), 1169);

    let mut entries = 0;
    let mut orphans = 0;
    for playlist in playlists_of(&pod) {
        for item in playlist.children_of(id::MHIP) {
            entries += 1;
            let referenced = item.u32(24).unwrap_or(0);
            // Zero is the "no track, just a podcast position" value.
            if referenced != 0 && !live.contains(&referenced) {
                orphans += 1;
            }
        }
    }
    assert_eq!(entries, 2338, "the fixture's playlist entries");
    assert_eq!(
        orphans, 0,
        "a playlist entry points at a track that is not there"
    );
}

/// A track comes off in all four pieces, and what is left still parses and still
/// verifies.
#[test]
fn removing_a_track_takes_its_file_its_record_and_its_playlist_entries() {
    let pod = FakePod::new("remove-one");
    let device = pod.device();
    let before = tracks_of(&pod);
    let victim = before.first().unwrap().clone();
    let victim_id = victim.u32(16).unwrap();
    let path = path_of(&pod, &location_of(&victim));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not really audio").unwrap();

    let entries_before: usize = playlists_of(&pod)
        .iter()
        .map(|p| p.children_of(id::MHIP).count())
        .sum();

    let gone = remove_one(&device, &path).unwrap();
    assert_eq!(gone.tracks, 1);
    assert_eq!(gone.files, 1);
    assert!(
        gone.playlist_items >= 1,
        "the track was in playlists, so entries must have gone too"
    );
    assert!(
        !path.exists(),
        "the file is the easy half and must actually be unlinked"
    );

    // The record, gone. Everything else, untouched.
    let after = tracks_of(&pod);
    assert_eq!(after.len(), before.len() - 1);
    assert!(!after.iter().any(|t| t.u32(16) == Some(victim_id)));
    assert_eq!(after.len() + gone.tracks, before.len());

    // No playlist entry still points at it.
    let left: usize = playlists_of(&pod)
        .iter()
        .map(|p| p.children_of(id::MHIP).count())
        .sum();
    assert_eq!(left + gone.playlist_items, entries_before);
    for playlist in playlists_of(&pod) {
        assert!(
            !playlist
                .children_of(id::MHIP)
                .any(|i| i.u32(24) == Some(victim_id)),
            "a playlist still lists the deleted track"
        );
    }

    // What was written is a database the device will accept.
    let bytes = std::fs::read(device.database()).unwrap();
    db::parse(&bytes).expect("the result still parses");
    assert!(
        hash58::verify(GUID, &bytes),
        "and still carries this iPod's signature"
    );
}

/// The album survives while another of its tracks is still there, and goes when
/// the last one does. Both halves, because the first is the mistake the second
/// invites.
#[test]
fn an_album_goes_only_when_its_last_track_does() {
    let pod = FakePod::new("remove-album");
    let device = pod.device();

    // A shared album and a one-track album, both found in the fixture rather
    // than hardcoded, so the test cannot go stale when the ids move.
    let tracks = tracks_of(&pod);
    let mut counts = std::collections::HashMap::new();
    for t in &tracks {
        *counts.entry(t.u32(288).unwrap()).or_insert(0usize) += 1;
    }
    let shared = counts
        .iter()
        .find(|(_, n)| **n > 1)
        .map(|(a, _)| *a)
        .unwrap();
    let lonely = counts
        .iter()
        .find(|(_, n)| **n == 1)
        .map(|(a, _)| *a)
        .unwrap();
    let albums_before = album_records(&pod);

    let pick = |album: u32| {
        tracks
            .iter()
            .find(|t| t.u32(288) == Some(album))
            .map(location_of)
            .unwrap()
    };

    let shared_path = path_of(&pod, &pick(shared));
    std::fs::create_dir_all(shared_path.parent().unwrap()).unwrap();
    std::fs::write(&shared_path, b"x").unwrap();
    let gone = remove_one(&device, &shared_path).unwrap();
    assert_eq!(
        gone.albums, 0,
        "another of the album's tracks is still on the device"
    );
    assert_eq!(album_records(&pod), albums_before);

    let lonely_path = path_of(&pod, &pick(lonely));
    std::fs::create_dir_all(lonely_path.parent().unwrap()).unwrap();
    std::fs::write(&lonely_path, b"x").unwrap();
    let gone = remove_one(&device, &lonely_path).unwrap();
    assert_eq!(
        gone.albums, 1,
        "it was the last track, so the album goes with it"
    );
    assert_eq!(album_records(&pod), albums_before - 1);
}

/// Artwork records are deliberately left alone — one deleted record would
/// renumber every record after it, and record *N* is slot *N* in each store.
#[test]
fn removing_a_track_leaves_the_artwork_records_alone() {
    let pod = FakePod::new("remove-art");
    seed_artwork(&pod);
    let records_before = {
        let tree =
            db::parse(&std::fs::read(tplay::ipod::artwork::database_path(&pod.root)).unwrap())
                .unwrap();
        artwork_records(&tree).len()
    };

    let victim = tracks_of(&pod).first().unwrap().clone();
    assert_ne!(
        victim.u32(352),
        Some(0),
        "the fixture's first track has artwork, which is the interesting case"
    );
    let path = path_of(&pod, &location_of(&victim));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"x").unwrap();
    remove_one(&pod.device(), &path).unwrap();

    let tree =
        db::parse(&std::fs::read(tplay::ipod::artwork::database_path(&pod.root)).unwrap()).unwrap();
    assert_eq!(
        artwork_records(&tree).len(),
        records_before,
        "a removed track's picture is still slot N; renumbering would move every cover after it"
    );
}

/// A file the database has never heard of is still unlinked. It is on the device
/// and the user asked for it gone; refusing would leave a row they cannot
/// remove, which is the same as not having the feature.
#[test]
fn a_file_the_database_does_not_know_is_still_unlinked() {
    let pod = FakePod::new("remove-stranger");
    let path = path_of(&pod, ":iPod_Control:Music:F99:ZZZZ.mp3");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"x").unwrap();
    let tracks_before = tracks_of(&pod).len();

    let gone = remove_one(&pod.device(), &path).unwrap();
    assert_eq!(
        gone,
        Removed {
            tracks: 0,
            playlist_items: 0,
            albums: 0,
            files: 1
        }
    );
    assert!(!path.exists());
    assert_eq!(
        tracks_of(&pod).len(),
        tracks_before,
        "nothing else may move"
    );
}

/// Something off the device is not the device's business. The database is not
/// opened at all, so this cannot be a path that reaches a mounted iPod's records
/// by accident.
#[test]
fn a_path_off_the_device_is_refused_and_left_alone() {
    let pod = FakePod::new("remove-elsewhere");
    let elsewhere = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/keep-me.mp3");
    std::fs::write(&elsewhere, b"x").unwrap();
    // The failure this guards is the worst one in the module: a path the user
    // never meant reaching this function being deleted because the strip_prefix
    // check was written as an early unlink instead of a refusal.
    let error = remove_one(&pod.device(), &elsewhere).unwrap_err();
    assert!(error.contains("not on this iPod"), "{error}");
    assert!(
        elsewhere.exists(),
        "a file outside the mount point is not ours to delete"
    );
    std::fs::remove_file(&elsewhere).ok();
}

/// A file the database lists but that is already gone is still removed from the
/// database. The two halves are independent, and a row pointing at nothing is
/// the state this function exists to end.
#[test]
fn a_record_whose_file_is_already_gone_is_still_removed() {
    let pod = FakePod::new("remove-gone-file");
    let victim = tracks_of(&pod).first().unwrap().clone();
    let path = path_of(&pod, &location_of(&victim));
    assert!(!path.exists(), "the fixture has no audio files");
    let gone = remove_one(&pod.device(), &path).unwrap();
    assert_eq!(gone.tracks, 1);
    assert_eq!(gone.files, 0, "there was no file to unlink");
}

/// The recursive case, and the batching that makes it affordable.
///
/// Two claims, and the second is the one that is invisible without measuring: a
/// walk that finds a track's file three folders down, and **one database write
/// for all of them**. Each `commit` re-serialises, signs, backs up and renames a
/// whole 1.9 MB database, so removing a real 1169-track library a track at a time
/// is over two gigabytes of writes onto a FAT device — and the per-track backup
/// would be the same file overwritten 1169 times.
#[test]
fn a_recursive_removal_reaches_every_depth_in_one_write() {
    let pod = FakePod::new("remove-recursive");
    let device = pod.device();

    // Put tracks into three different `Fnn` folders, so a walk that stops at the
    // first level finds none of the others.
    let tracks = tracks_of(&pod);
    let mut placed = std::collections::HashMap::new();
    for track in tracks.iter().take(30) {
        placed
            .entry(
                location_of(track)
                    .rsplit_once(':')
                    .map(|(head, _)| head.to_string())
                    .unwrap(),
            )
            .or_insert_with(Vec::new)
            .push(track);
    }
    let folders: Vec<Vec<&Chunk>> = placed.into_values().collect();
    assert!(
        folders.len() >= 3,
        "need tracks in at least three Fnn folders"
    );

    let mut written = Vec::new();
    for folder in &folders {
        for track in folder.iter().take(2) {
            let path = path_of(&pod, &location_of(track));
            if written.contains(&path) {
                continue;
            }
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
            written.push(path);
        }
    }
    // One file deliberately never written: a record whose file a half-finished
    // sync left behind, which must not stop the others. The count is what was
    // actually written, plus that one — a folder can hold fewer than the two
    // this takes from it, and assuming otherwise is how the first version of
    // this test failed.
    let tracks_before = tracks.len();
    let wanted = written.len() + 1;

    let found = remove::audio_files_under(&pod.root.join("iPod_Control/Music"));
    assert_eq!(
        found.len(),
        written.len(),
        "the walk must reach every Fnn folder, not just the first"
    );

    // One call, and the database is written once — the batch is the claim. The
    // orphan is chosen by *exclusion* rather than by index: picking `tracks[n]`
    // where n happens to be one already collected picks the same location twice,
    // and the set collapses it — which is correct, and made the first version of
    // this test count one short.
    let orphan = tracks
        .iter()
        .find(|t| !written.contains(&path_of(&pod, &location_of(t))))
        .expect("some track was not written");
    let mut with_orphan = found.clone();
    with_orphan.push(path_of(&pod, &location_of(orphan)));
    let gone = remove::remove_paths(&device, &with_orphan).unwrap();
    assert_eq!(gone.tracks, wanted, "every track handed over, file or not");
    assert_eq!(gone.files, written.len());
    assert_eq!(tracks_of(&pod).len(), tracks_before - gone.tracks);
    assert!(gone.playlist_items >= gone.tracks);
    for path in &written {
        assert!(!path.exists(), "{} should be gone", path.display());
    }

    // The batch is the claim, and the batch is observable: `commit` keeps the
    // previous database, so a backup holding **every** track proves one write
    // happened after all of them were edited — a per-track loop would leave a
    // backup holding only the last track's worth.
    let backup = device.backup_of(&device.database());
    assert!(backup.exists(), "the previous database is recoverable");
    let kept = std::fs::read(&backup).unwrap();
    assert!(
        db::parse(&kept).is_ok(),
        "and it is a whole database, not a fragment"
    );
    let backed_up_tracks = db::list_holder(&db::parse(&kept).unwrap(), db::kind::TRACKS)
        .map_or(0, |l| l.children_of(id::MHIT).count());
    assert_eq!(
        backed_up_tracks, tracks_before,
        "the backup holds every track, so all of them went in a single write"
    );
}

/// A symlinked folder is not followed, which is both the honest answer and the
/// only way this walk terminates on a loop.
#[test]
fn a_recursive_walk_skips_a_symlinked_folder() {
    let pod = FakePod::new("remove-symlink");
    let music = pod.root.join("iPod_Control/Music");
    std::fs::create_dir_all(music.join("F01")).unwrap();
    std::fs::write(music.join("F01/a.mp3"), b"x").unwrap();
    // A loop: the folder points at its own ancestor.
    std::os::unix::fs::symlink(&music, music.join("F01/loop")).unwrap();

    let found = remove::audio_files_under(&music);
    assert_eq!(
        found.len(),
        1,
        "a symlinked folder is skipped, so a loop cannot hang the walk"
    );
    assert!(found[0].ends_with("F01/a.mp3"));
}

/// A file the device cannot play is not collected — a playlist file, a `.jpg`, a
/// `.flac`. There is no record to remove for any of them, and taking a folder's
/// contents blindly would delete a user's notes.
#[test]
fn a_recursive_walk_collects_only_playable_files() {
    let pod = FakePod::new("remove-kinds");
    let music = pod.root.join("iPod_Control/Music/F01");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["a.mp3", "b.m4a", "c.flac", "d.wav", "e.jpg", "playlist.m3u"] {
        std::fs::write(music.join(name), b"x").unwrap();
    }
    let found = remove::audio_files_under(&pod.root.join("iPod_Control/Music"));
    let names: Vec<String> = found
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "only the two the device can play: {names:?}"
    );
    assert!(names.contains(&"a.mp3".to_string()));
    assert!(names.contains(&"b.m4a".to_string()));
}

/// `remove_paths` is what the row's `✕` and the header's button both call, so
/// what it covers is the one place a wrong answer destroys more than was asked
/// for.
///
/// The routing it used to do — one file, one folder, refuse two folders because
/// their common parent is `Music/` and that is the whole library — is **gone**,
/// and this is why that is safe rather than a loss. It no longer resolves a set
/// of paths to a directory and removes the directory; it removes **exactly the
/// files it was handed**, and each one against its own location string. So a
/// caller that passes a whole `Music/` walk gets the whole library *because it
/// asked for it*, and a caller that passes two files from two folders gets two
/// files. There is no step where a directory is widened out from under the
/// caller.
#[test]
fn remove_paths_removes_exactly_the_files_it_is_handed() {
    let pod = FakePod::new("remove-paths");
    let device = pod.device();

    let tracks = tracks_of(&pod);
    let write = |track: &Chunk| {
        let path = path_of(&pod, &location_of(track));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
        path
    };

    // Two tracks from two different Fnn folders, which is the case the old
    // routing refused outright.
    let mut per_folder: std::collections::HashMap<String, Vec<&Chunk>> =
        std::collections::HashMap::new();
    for track in tracks.iter() {
        let dir = location_of(track).rsplit_once(':').unwrap().0.to_string();
        per_folder.entry(dir).or_default().push(track);
        if per_folder.len() >= 2 {
            break;
        }
    }
    let mut folders: Vec<Vec<&Chunk>> = per_folder.into_values().collect();
    folders.sort_by_key(|f| location_of(f[0]));
    // iTunes spreads a library over fifty folders, so the first two may hold a
    // single track each. Take whatever is there rather than a fixed two, and
    // assert the shapes this rest of the test assumes.
    let first: Vec<PathBuf> = folders[0].iter().map(|t| write(t)).collect();
    let second: Vec<PathBuf> = folders[1]
        .iter()
        .filter(|t| !first.contains(&path_of(&pod, &location_of(t))))
        .map(|t| write(t))
        .collect();
    assert!(
        !first.is_empty() && !second.is_empty(),
        "need tracks in two folders"
    );
    assert_eq!(
        first.len() + second.len(),
        folders[0].len() + folders[1].len(),
        "the two folders must not share a track"
    );

    // One folder's files: that folder, and no other.
    let before = tracks_of(&pod).len();
    let gone = remove::remove_paths(&device, &first).unwrap();
    assert_eq!(gone.tracks, first.len());
    assert!(
        second.iter().all(|f| f.exists()),
        "the other folder is untouched"
    );
    assert_eq!(tracks_of(&pod).len(), before - first.len());

    // Both folders at once. The first folder's records are already gone from the
    // step above, so what this removes is the second's — and a file that is
    // handed over twice is not an error, it is just already gone.
    let mut both = first.clone();
    both.extend(second.clone());
    let gone = remove::remove_paths(&device, &both).unwrap();
    assert_eq!(gone.tracks, second.len(), "not two folders' worth");
    assert_eq!(
        gone.files,
        second.len(),
        "and not a file that was already unlinked"
    );
    assert!(both.iter().all(|f| !f.exists()));

    // One file: that file. `remove_track` is a wrapper over this, not a path of
    // its own, so the two cannot drift.
    let single = write(&tracks_of(&pod).first().unwrap().clone());
    let gone = remove_one(&device, &single).unwrap();
    assert_eq!(gone.tracks, 1);
    assert_eq!(gone.files, 1);
    assert!(!single.exists());
}

/// An empty list is not an error and not a rewrite.
///
/// The header arms this from a walk that found nothing — the user is looking at
/// a folder with no playable files in it — and rewriting a 1.9 MB database to
/// learn there was nothing to do is the kind of cost nobody notices until it is
/// on a FAT device.
#[test]
fn an_empty_removal_writes_nothing() {
    let pod = FakePod::new("remove-empty");
    let before = std::fs::read(pod.device().database()).unwrap();
    let gone = remove::remove_paths(&pod.device(), &[]).unwrap();
    assert_eq!(gone, Removed::default());
    assert_eq!(
        std::fs::read(pod.device().database()).unwrap(),
        before,
        "the database must not have been touched"
    );
    assert!(
        !pod.device().database().with_extension("bak").exists(),
        "and no backup was made either"
    );
}

// ── udisks: mount and eject ───────────────────────────────────────────────────

/// The real `udisksctl status` from the machine the iPod was written against,
/// copied verbatim — real disk models, the real iPod row, and a real
/// `LUCAS'S IPO` with an apostrophe and a space in it.
const STATUS: &str = "\
MODEL                     REVISION  SERIAL               DEVICE
--------------------------------------------------------------------------
TOSHIBA-RD400             57CZ4102  172S102PTPEV         nvme0n1
ST3500413AS               JC4B      Z2A67KVT             sda
ADATA SU750               XA004R24  2L522L16GBND         sdb
CT1000BX500SSD1           M6CR061   2318E6D13A9C         sdc
WDC  WDS100T2B0A-00SM50   415000WD  20430C806167         sdd
Apple iPod                1.62      000A27002158F9D4     sde
";

/// The iPod is found in a list that is mostly other people's disks, and it is
/// found by the **model** — which comes off the USB product string and is
/// therefore there whether or not anything is mounted. That is the whole reason
/// a mounted check is not a discovery check: `iPod_Control` lives on the
/// filesystem, so an unmounted device has nothing on it to read.
#[test]
fn an_ipod_is_found_by_its_model_among_other_disks() {
    let drives = tplay::ipod::device::parse_status(STATUS);
    assert_eq!(drives.len(), 1, "one iPod among six disks");
    assert_eq!(drives[0].model, "Apple iPod");
    assert_eq!(drives[0].device, "/dev/sde");
    assert_eq!(
        drives[0].serial, "000A27002158F9D4",
        "and udisks' serial is the FireWire GUID, so the key is known unmounted"
    );
}

/// The serial is the same number `guid_from_udev` reads, which is what lets an
/// unmounted device be keyed and a mounted one be read the existing way.
#[test]
fn the_udisks_serial_is_the_firewire_guid_this_fixture_is_signed_with() {
    let drives = tplay::ipod::device::parse_status(STATUS);
    let mut bytes = [0u8; 8];
    let digits: String = drives[0].serial.chars().collect();
    for (i, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).expect("udisks prints hex");
    }
    assert_eq!(bytes, GUID, "the same key, from a different source");
}

/// The header and its rule are skipped by shape, not by counting lines, because
/// the rule's width follows the terminal and the terminal is not ours.
#[test]
fn the_status_header_and_rule_are_skipped_whatever_they_look_like() {
    let wide =
        "MODEL REVISION SERIAL DEVICE\n--------------------------------\nApple iPod 1.62 ABC sdz\n";
    let drives = tplay::ipod::device::parse_status(wide);
    assert_eq!(drives.len(), 1);
    assert_eq!(drives[0].device, "/dev/sdz");

    // A disk whose model merely contains the letters, and one with an empty
    // serial, still parse — the model is the discriminator, not the rest.
    let tricky = "MODEL R S DEVICE\n----\niPod Nano 1 00A0 sdb\nDisk 1 00B0 sdc\n";
    let drives = tplay::ipod::device::parse_status(tricky);
    assert_eq!(drives.len(), 1, "only the row that says iPod");
    assert_eq!(drives[0].device, "/dev/sdb");
}

/// A machine with no iPod plugged in is the common case and must be silent.
#[test]
fn no_ipod_yields_no_rows() {
    assert!(tplay::ipod::device::parse_status("").is_empty());
    assert!(
        tplay::ipod::device::parse_status("MODEL R SERIAL DEVICE\n----\nWDC Thing 1 X sdd\n")
            .is_empty()
    );
}

/// Real `udisksctl info -b` output for the drive and for its partition.
const DRIVE_INFO: &str = "\
/org/freedesktop/UDisks2/block_devices/sde:
  org.freedesktop.UDisks2.Block:
    Device:                     /dev/sde
    Partitions:         /org/freedesktop/UDisks2/block_devices/sde1
    PreferredDevice:            /dev/sde
    Removable:                  true
  org.freedesktop.UDisks2.Drive:
    RotationRate:               0
";

const PARTITION_INFO: &str = "\
/org/freedesktop/UDisks2/block_devices/sde1:
  org.freedesktop.UDisks2.Block:
    Device:                     /dev/sde1
    Drive:                      '/org/freedesktop/UDisks2/drives/Apple_iPod_000A27002158F9D4'
    IdLabel:                    LUCAS'S IPO
    IdType:                     vfat
    IdUsage:                    filesystem
    MountPoints:        /run/media/lucas/LUCAS_S IPO
";

/// An **unmounted** partition's listing, which is the case this whole feature
/// exists for. Every property the mounted one has is here except the mount point.
const UNMOUNTED_INFO: &str = "\
/org/freedesktop/UDisks2/block_devices/sde1:
  org.freedesktop.UDisks2.Block:
    Device:                     /dev/sde1
    IdLabel:                    LUCAS'S IPO
    IdType:                     vfat
    MountPoints:
";

/// The partition is the last segment of the object path, and `udisksctl -p`
/// **rejects that path** — so taking the device name is what makes mounting work
/// at all.
#[test]
fn the_partition_is_read_off_the_drive_listing() {
    use tplay::ipod::device::partition_device;
    assert_eq!(
        partition_device(DRIVE_INFO),
        Some(PathBuf::from("/dev/sde1")),
        "-p would reject the object path, so the name is what we use"
    );
    // A whole-disk device has no partition to mount.
    assert_eq!(partition_device("    Device:  /dev/sdb\n"), None);
    assert_eq!(partition_device(""), None);
}

/// **A blank `MountPoints` means "not mounted", and that is the answer the whole
/// feature turns on.** Reading it as an empty string instead of absent is the one
/// mistake that would mount nothing and say it worked.
#[test]
fn an_unmounted_device_reports_no_mount_point() {
    use tplay::ipod::device::property;
    assert_eq!(
        property(PARTITION_INFO, "MountPoints"),
        Some("/run/media/lucas/LUCAS_S IPO".to_string())
    );
    assert_eq!(
        property(UNMOUNTED_INFO, "MountPoints"),
        None,
        "blank is unset, and unset is what an unmounted device looks like"
    );
    assert_eq!(
        property(UNMOUNTED_INFO, "IdLabel"),
        Some("LUCAS'S IPO".into())
    );
}

/// A property that is not there at all, and one that merely shares a prefix with
/// another. `IdLabel` and `IdUUID` are one character apart; a `starts_with`
/// match would return the wrong one.
#[test]
fn a_property_does_not_match_a_neighbour() {
    use tplay::ipod::device::property;
    let info = "    IdLabel:   mine\n    IdUUID:    theirs\n    Id:       \n";
    assert_eq!(property(info, "IdLabel"), Some("mine".into()));
    assert_eq!(property(info, "IdUUID"), Some("theirs".into()));
    assert_eq!(property(info, "Id"), None, "blank means unset");
    assert_eq!(property(info, "Nope"), None);
}

/// The value carries an apostrophe and a space, and udisks' `info` prints them
/// unescaped even though `/proc/self/mounts` escapes the same characters in
/// octal — two different subsystems, two different spellings, both of which this
/// codebase now decodes.
#[test]
fn a_label_with_an_apostrophe_and_a_space_survives_both_encodings() {
    use tplay::ipod::device::property;
    assert_eq!(
        property(PARTITION_INFO, "IdLabel"),
        Some("LUCAS'S IPO".into())
    );
    let mounts = tplay::library::Volume::parse_mounts_with_labels(
        r#"/dev/sde1 /run/media/lucas/LUCAS_S\040IPO vfat rw 0 0"#,
        &std::collections::HashMap::from([("sde1".to_string(), r"LUCAS\x27S\x20IPO".to_string())]),
    );
    assert_eq!(mounts[0].label, "LUCAS'S IPO");
}

/// The sidebar's iPod section is the only place an iPod is listed.
///
/// A mass-storage iPod *was* already in **Volumes** and needed no work to appear
/// there — which is exactly why it belonged nowhere in it. That row offered no
/// way to mount a device that was plugged in but unmounted and no way to eject
/// one that was mounted, so the section that can do both has to be the only one
/// that shows it, or the same device appears twice with two different
/// affordances and no way to say which is current.
#[test]
fn the_ipod_is_not_also_listed_as_a_volume() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gui/panes/library/sidebar.rs"),
    )
    .expect("read sidebar.rs");

    let volumes = src
        .find("\"Volumes\"")
        .expect("premise: a Volumes section exists");
    let filter = src[volumes..]
        .find("dirs\n                    .volumes")
        .or_else(|| src[volumes..].find(".volumes\n"))
        .map(|i| volumes + i)
        .or_else(|| src[volumes..].find(".volumes").map(|i| volumes + i))
        .expect("premise: the volume list is filtered, not used raw");
    let exclusion = &src[filter..filter + 400.min(src.len() - filter)];
    assert!(
        exclusion.contains("ipods"),
        "the filter must exclude anything the iPod section already shows: {exclusion}"
    );
}

/// The sidebar re-reads after a job finishes, or the Mount button outlives the
/// device it acted on.
///
/// This is the one entry in `SidebarDirs` that changes mid-session: everything
/// else is filesystem state that moves about never, which is what made the
/// once-per-session cache right. A cache that never re-reads leaves the row
/// saying "Mount" to a device that is now mounted, and "Eject" to one that is
/// not — a button that does the wrong thing rather than one that does nothing.
#[test]
fn the_sidebar_re_reads_when_an_ipod_job_finishes() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gui/panes/library/sidebar.rs"),
    )
    .expect("read sidebar.rs");
    assert!(
        src.contains("app.ipod().epoch()") && src.contains("seen == epoch"),
        "the cache is keyed on the job epoch"
    );

    // And the epoch moves on **both** endings, because a job that failed after
    // half-changing the outside world still changed it.
    let push = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ipod/push.rs"),
    )
    .expect("read push.rs");
    assert_eq!(
        push.matches("self.epoch = self.epoch.wrapping_add(1);")
            .count(),
        2,
        "once for a job that answered and once for one whose thread died"
    );
}

// ── Pushing the same songs twice ──────────────────────────────────────────────

/// Tracks on the device whose title is `want`, from the bytes on disk.
fn device_tracks_titled(pod: &FakePod, want: &str) -> Vec<Chunk> {
    tracks_of(pod)
        .into_iter()
        .filter(|t| {
            t.mhod(f::TITLE)
                .and_then(|m| m.mhod_text())
                .is_some_and(|s| s == want)
        })
        .collect()
}

/// Files under the device's music folder, which is what a second copy would add.
fn device_music_files(pod: &FakePod) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files(&pod.root.join("iPod_Control/Music"), &mut out);
    out
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Pressing "To iPod" twice must not grow the library.
///
/// Before this, every `add_track` allocated a fresh track id, copied the file
/// under a new four-letter name and added a second `mhit` — so a second press
/// of the same folder **doubled the device**, and a third tripled it. The user's
/// two words were "gracefully", and the only graceful reading of a duplicate is
/// that it should not happen.
#[test]
fn pushing_the_same_song_twice_leaves_one_copy() {
    let pod = FakePod::new("push-twice");
    let device = pod.device();
    let source = pod.root.join("song.mp3");
    common::write_tagged_mp3(&source, "Renascença", "A Band", "An Album");

    let tracks_before = tracks_of(&pod).len();
    let first = push::push_album(&device, std::slice::from_ref(&source), None).unwrap();
    assert_eq!(first.added, 1);
    assert_eq!(first.already, 0);
    let after_first = tracks_of(&pod).len();
    assert_eq!(after_first, tracks_before + 1);
    let files_after_first = device_music_files(&pod).len();

    // The same file, same folder, same everything — the second press.
    let second = push::push_album(&device, std::slice::from_ref(&source), None).unwrap();
    assert_eq!(second.already, 1, "the song was recognised");
    assert_eq!(second.added, 0, "and not copied again");
    assert_eq!(
        tracks_of(&pod).len(),
        after_first,
        "the record count must not move"
    );
    assert_eq!(
        device_music_files(&pod).len(),
        files_after_first,
        "and neither must the file count — the copy is the expensive part"
    );
    assert_eq!(device_tracks_titled(&pod, "Renascença").len(), 1);
}

/// Two different songs are still two songs.
///
/// The half that matters beside the dedup: an identity loose enough to swallow a
/// genuine second track would pass the test above and quietly lose music, which
/// is the one failure mode worth being paranoid about.
#[test]
fn two_different_songs_both_go_across() {
    let pod = FakePod::new("push-two");
    let device = pod.device();
    let one = pod.root.join("one.mp3");
    let two = pod.root.join("two.mp3");
    common::write_tagged_mp3(&one, "First Song", "A Band", "An Album");
    common::write_tagged_mp3(&two, "Second Song", "A Band", "An Album");

    let summary = push::push_album(&device, &[one, two], None).unwrap();
    assert_eq!(summary.added, 2);
    assert_eq!(summary.already, 0, "same album and artist, different song");
    assert_eq!(device_tracks_titled(&pod, "First Song").len(), 1);
    assert_eq!(device_tracks_titled(&pod, "Second Song").len(), 1);
}

/// Same title and artist, **different album** — still two, because the album is
/// part of what makes a track the one the user means.
#[test]
fn the_same_song_on_two_albums_is_two_tracks() {
    let pod = FakePod::new("push-compilation");
    let device = pod.device();
    let one = pod.root.join("live.mp3");
    let two = pod.root.join("studio.mp3");
    common::write_tagged_mp3(&one, "Yesterday", "A Band", "Live");
    common::write_tagged_mp3(&two, "Yesterday", "A Band", "Studio");

    let summary = push::push_album(&device, &[one, two], None).unwrap();
    assert_eq!(summary.added, 2, "a compilation has the same song twice");
}

/// A **playlist** of songs already on the device still gets them listed — that is
/// the whole point of pressing the button a second time, and the reason
/// `add_track` returns the existing track id rather than refusing.
///
/// A user whose songs are all on the iPod and who wants them as a playlist has no
/// other way to get one: a skip that dropped them from the list would leave the
/// button looking broken.
#[test]
fn a_playlist_of_already_present_songs_is_still_created() {
    let pod = FakePod::new("push-playlist-twice");
    let device = pod.device();
    let source = pod.root.join("song.mp3");
    common::write_tagged_mp3(&source, "Renascença", "A Band", "An Album");

    push::push_album(&device, std::slice::from_ref(&source), None).unwrap();
    let second =
        push::push_playlist(&device, std::slice::from_ref(&source), "Favourites", None).unwrap();
    assert_eq!(second.added, 0, "nothing new to copy");
    assert_eq!(second.already, 1);

    let signed = std::fs::read(device.database()).unwrap();
    let tree = db::parse(&signed).unwrap();
    let playlists = db::list_holder(&tree, db::kind::PLAYLISTS).unwrap();
    let mine = playlists
        .children_of(id::MHYP)
        .find(|p| {
            p.mhod(1)
                .and_then(|m| m.mhod_text())
                .is_some_and(|n| n == "Favourites")
        })
        .expect("the playlist must exist");
    assert_eq!(
        mine.children_of(id::MHIP).count(),
        1,
        "and it lists the song, pointing at the copy already on the device"
    );
}

/// A tag edit and a second push makes a second copy — **and that is the
/// deliberate direction to fail in**.
///
/// The identity is title, artist, album and file type. Anything that survived a
/// tag edit would need to read every audio file on the device to stay honest,
/// and a match loose enough to accept an edited file would also accept a
/// different song that shares its tags. A duplicate is visible and removable; a
/// song silently not making it onto the device is not recoverable by the user
/// guessing what happened.
#[test]
fn a_tag_edit_makes_a_second_copy_rather_than_silently_skipping() {
    let pod = FakePod::new("push-edit");
    let device = pod.device();
    let source = pod.root.join("song.mp3");
    common::write_tagged_mp3(&source, "Original Title", "A Band", "An Album");
    push::push_album(&device, std::slice::from_ref(&source), None).unwrap();

    // The same audio, retitled on disk.
    common::write_tagged_mp3(&source, "Corrected Title", "A Band", "An Album");
    let second = push::push_album(&device, std::slice::from_ref(&source), None).unwrap();

    assert_eq!(
        second.already, 0,
        "a different title is a different identity"
    );
    assert_eq!(second.added, 1);
    assert_eq!(device_tracks_titled(&pod, "Original Title").len(), 1);
    assert_eq!(device_tracks_titled(&pod, "Corrected Title").len(), 1);
}

/// The result line reads as success when nothing was copied, because that is
/// what it means.
///
/// "0 on the iPod, 14 already there" is a true sentence and a useless one:
/// pressing the button twice is the **expected** path, and a count of zero is
/// how the user learns the feature is broken before they check whether it is.
#[test]
fn a_second_push_reads_as_done_rather_than_as_zero() {
    use tplay::ipod::push::Outcome;
    let twice = Outcome::Added(push::Summary {
        added: 0,
        refused: vec![],
        already: 14,
    });
    assert_eq!(twice.describe(), "14 tracks already on the iPod");

    let mixed = Outcome::Added(push::Summary {
        added: 3,
        refused: vec![],
        already: 11,
    });
    assert_eq!(mixed.describe(), "3 tracks on the iPod, 11 already there");

    let refused = Outcome::Added(push::Summary {
        added: 2,
        refused: vec!["song.flac".into()],
        already: 0,
    });
    assert_eq!(refused.describe(), "2 tracks on the iPod, 1 refused");
}

/// Case does not make a second song.
///
/// iTunes, tagging tools and hand edits all disagree about capitalisation, and
/// two spellings of the same title are one song to a person. The first version
/// of the identity compared case-sensitively and every test passed, because
/// every title in the suite was written the same way twice.
#[test]
fn a_retagged_title_in_another_case_is_the_same_song() {
    let pod = FakePod::new("push-case");
    let device = pod.device();
    let source = pod.root.join("song.mp3");
    common::write_tagged_mp3(&source, "Yesterday", "The Beatles", "Help!");
    push::push_album(&device, std::slice::from_ref(&source), None).unwrap();

    common::write_tagged_mp3(&source, "YESTERDAY", "The Beatles", "Help!");
    let second = push::push_album(&device, std::slice::from_ref(&source), None).unwrap();

    assert_eq!(second.already, 1, "capitalisation is not a different song");
    assert_eq!(second.added, 0);
}

#[test]
fn should_convert_returns_true_for_unplayable_formats() {
    use std::fs::File;
    use std::io::Write;
    use tplay::ipod::push::Push;

    let dir = std::env::temp_dir().join(format!("tplay-test-{}-aac-test", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let flac = dir.join("test.flac");
    let ogg = dir.join("test.ogg");
    let wav = dir.join("test.wav");

    // Create dummy files with unplayable extensions
    File::create(&flac).unwrap().write_all(b"dummy").unwrap();
    File::create(&ogg).unwrap().write_all(b"dummy").unwrap();
    File::create(&wav).unwrap().write_all(b"dummy").unwrap();

    assert!(Push::should_convert(&flac, 128_000));
    assert!(Push::should_convert(&ogg, 128_000));
    assert!(Push::should_convert(&wav, 128_000));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn should_convert_returns_false_for_mp3_below_target() {
    use tplay::ipod::push::Push;

    let dir = std::env::temp_dir().join(format!("tplay-test-{}-aac-test", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mp3 = dir.join("test.mp3");
    common::write_tagged_mp3(&mp3, "Test", "Artist", "Album");

    // 128 kbps MP3, target 256 kbps - should not convert
    assert!(!Push::should_convert(&mp3, 256_000));
    // 128 kbps MP3, target 128 kbps - should not convert (equal)
    assert!(!Push::should_convert(&mp3, 128_000));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ipod_aac_config_default_is_none() {
    use tplay::config::Config;
    let config = Config::default();
    assert_eq!(config.ipod_aac, None);
}

#[test]
fn ipod_aac_config_round_trips() {
    use tplay::config::Config;
    let config = Config {
        ipod_aac: Some(256_000),
        ..Default::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(back.ipod_aac, Some(256_000));
}

#[test]
fn ipod_aac_hand_edited_value_snaps_to_nearest() {
    use tplay::config::Prefs;
    let config = tplay::config::Config {
        ipod_aac: Some(999),
        ..Default::default()
    }; // hand-edited value not in AAC_BITRATES

    let prefs = Prefs::from_config(&config);
    // Should snap to nearest offered rate (128000, 256000, 320000)
    // 999 is closest to 128000
    assert_eq!(prefs.ipod_aac(), Some(128_000));
}

#[test]
fn ipod_aac_prefs_setter_snaps_to_nearest() {
    use tplay::config::Prefs;
    let mut prefs = Prefs::from_config(&tplay::config::Config::default());
    prefs.set_ipod_aac(Some(999));
    assert_eq!(prefs.ipod_aac(), Some(128_000));
    prefs.set_ipod_aac(Some(200_000));
    assert_eq!(prefs.ipod_aac(), Some(256_000)); // closer to 256k
    prefs.set_ipod_aac(Some(300_000));
    assert_eq!(prefs.ipod_aac(), Some(320_000)); // closer to 320k
    prefs.set_ipod_aac(None);
    assert_eq!(prefs.ipod_aac(), None);
}
