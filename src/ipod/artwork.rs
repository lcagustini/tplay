//! Album art: the pixels, and the database record that points at them.
//!
//! # Where the picture actually is
//!
//! Not in a file. An iPod keeps its artwork in three `.ithmb` stores under
//! `iPod_Control/Artwork/`, one per screen size, plus a `ArtworkDatabase` whose
//! records say which store holds which image and at what geometry. **All three
//! stores hold the same images in the same order**, so record *N* is slot *N* in
//! every file and a new image is an append.
//!
//! # The pixel format, established by decoding and looking
//!
//! **RGB565, little-endian.** Not assumed from a table: slot 0 of `F1055_1.ithmb`
//! was decoded under each candidate layout and rendered as a PNG, and only one
//! was a photograph. Every value in [`FORMATS`] below was measured the same way —
//! the sizes divide out of the file lengths exactly (2 752 512 / 84 = 32 768,
//! 17 203 200 / 84 = 204 800, 517 440 / 84 = 6 160), and the 55×55 one is stored
//! with a **stride of 56**, a zero padding column per row, which a wrong width
//! turns into horizontal streaks.
//!
//! # The record, and why one is cloned rather than built
//!
//! `ArtworkDB` is the same chunk grammar as `iTunesDB` under an `mhfd` root, and
//! an `mhii` holds three `mhod`s naming the three stores plus one type-6 `mhod`
//! whose payload is zero in all 84 records of a real device. Two fields in the
//! type-2 payload are **not understood**: one is the image size (verified against
//! every file length), and the other is constant per store for two of the three
//! and varies slightly for the third. So [`add_image`] copies an existing record
//! and changes only the identity — the same move [`crate::ipod::push`] makes for
//! a track, and for the same reason.
//!
//! **A device with no artwork gets a refusal rather than a guess.** There is no
//! record to copy, the format is undocumented, and a wrong record can leave the
//! device showing no art for the whole library.
//!
//! # Not signed
//!
//! `ArtworkDB` declares hashing scheme 1 at offset `0x30` and carries **twenty
//! zero bytes** where `iTunesDB` carries its signature. So no key is needed here,
//! and [`crate::ipod::hash58::sign`] is only ever applied to the track database.

use crate::ipod::db::id;
use crate::ipod::db::{self, Chunk};
use crate::ipod::device::Device;
use std::path::{Path, PathBuf};

/// One artwork size: a store's format code, and the geometry its slots use.
///
/// `stride` is separate from `width` because the smallest store pads every row.
/// That padding is the difference between a thumbnail and a row of streaks, so
/// it is a field rather than something the encoder works out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub code: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

impl Format {
    /// Bytes one slot occupies. `stride * height * 2`, which is what divides the
    /// file length exactly on a real device.
    pub const fn byte_len(self) -> usize {
        (self.stride * self.height * 2) as usize
    }

    /// The store file name this format's slots live in.
    pub fn file_name(self) -> String {
        format!("F{}_{1}.ithmb", self.code, STORE_INDEX)
    }
}

/// The single `ithmb` a device carries per format. Every one measured has the
/// `_1` suffix, and a second index would be a different store the device would
/// have to have told us about.
const STORE_INDEX: u32 = 1;

/// The three sizes an iPod Classic stores artwork at.
///
/// **A code that is not here is refused**, rather than encoded from a guess: the
/// geometry of a store cannot be derived from its file, only read out of a
/// record the device wrote.
pub const FORMATS: [Format; 3] = [
    Format {
        code: 1055,
        width: 128,
        height: 128,
        stride: 128,
    },
    Format {
        code: 1060,
        width: 320,
        height: 320,
        stride: 320,
    },
    Format {
        code: 1061,
        width: 55,
        height: 55,
        stride: 56,
    },
];

/// The format a record's `mhod` names, or `None` for a store this app cannot
/// write.
pub fn format_for(code: u32) -> Option<Format> {
    FORMATS.iter().copied().find(|f| f.code == code)
}

/// `image` fields, in one place so the encoder and decoder cannot disagree.
mod rgb565 {
    /// Pack one pixel. The device's own low byte ordering, confirmed by decoding.
    pub fn pack(r: u8, g: u8, b: u8) -> [u8; 2] {
        let value = (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3);
        value.to_le_bytes()
    }
}

/// Encode `pixels` — row-major RGB8, `format.width` wide — into one slot.
///
/// Public because it is the honest shape of the operation (one size in, one slot
/// out) rather than a test hook, and because the geometry it walks is the
/// geometry the device walks.
pub fn encode_slot(format: Format, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(format.byte_len());
    for y in 0..format.height {
        let start = (y * format.width * 3) as usize;
        let width = (format.width * 3) as usize;
        let row = &pixels[start..start + width];
        for chunk in row.as_chunks::<3>().0 {
            out.extend_from_slice(&rgb565::pack(chunk[0], chunk[1], chunk[2]));
        }
        // The padding column, zeroed. Every row of the 55×55 store has one.
        out.resize(out.len() + ((format.stride - format.width) * 2) as usize, 0);
    }
    out
}

/// Scale `source` to `format`'s size, filling rather than fitting.
///
/// An album cover is square and so is every store, and a cover pushed into a
/// list row is better cropped than letterboxed: this scales the shorter side up
/// and takes the middle off the longer one. The alternative stretches, and a
/// stretched square cover looks wrong at 55 pixels in a way a crop does not.
pub fn fit_cover(source: &image::DynamicImage, format: Format) -> Vec<u8> {
    let (sw, sh) = (source.width() as f32, source.height() as f32);
    if sw <= 0.0 || sh <= 0.0 {
        return vec![0; (format.width * format.height * 3) as usize];
    }
    let scale = (format.width as f32 / sw).max(format.height as f32 / sh);
    let scaled = image::imageops::resize(
        source,
        (sw * scale).round().max(1.0) as u32,
        (sh * scale).round().max(1.0) as u32,
        image::imageops::FilterType::Triangle,
    );
    let rgb = image::DynamicImage::ImageRgba8(scaled).into_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let raw = rgb.as_raw();
    let (mut x0, mut y0) = (0usize, 0usize);
    if w > format.width {
        x0 = (w - format.width) as usize / 2;
    }
    if h > format.height {
        y0 = (h - format.height) as usize / 2;
    }
    let width = format.width as usize * 3;
    let mut out = Vec::with_capacity(width * format.height as usize);
    for y in 0..format.height as usize {
        let start = ((y0 + y) * w as usize + x0) * 3;
        out.extend_from_slice(&raw[start..start + width]);
    }
    out
}

/// The `iPod_Control/Artwork` directory.
pub fn artwork_dir(root: &Path) -> PathBuf {
    root.join("iPod_Control").join("Artwork")
}

/// The `ArtworkDB` path.
pub fn database_path(root: &Path) -> PathBuf {
    artwork_dir(root).join("ArtworkDB")
}

/// What one image became.
#[derive(Debug, PartialEq, Eq)]
pub struct Added {
    /// The id the new `mhii` carries, which is what `mhit` offset 352 wants.
    pub image_id: u32,
    /// How many stores the pixels went into.
    pub stores: usize,
}

/// Add `cover` to the device as one new image, and return the id to point a
/// track at.
///
/// Every store is written, because nothing measurable says the device falls back
/// to the size it has when another is missing, and a missing size means no
/// artwork on whichever screen wanted it.
///
/// Takes the `Device` rather than a bare root because the backups are keyed on
/// its GUID and go to the cache directory like the track database's — the stores
/// are 20 MB between them, and a `.bak` beside each is 20 MB of clutter on the
/// one device the user is least likely to go looking through.
pub fn add_image(device: &Device, cover: &[u8], dbid: u64) -> Result<Added, String> {
    let root = &device.root;
    let image = image::load_from_memory(cover)
        .map_err(|e| format!("the cover image could not be decoded: {e}"))?;

    let db_path = database_path(root);
    let db_bytes = std::fs::read(&db_path).map_err(|e| format!("{}: {e}", db_path.display()))?;
    let mut tree = db::parse(&db_bytes)?;

    let records = image_records(&tree);
    if records.is_empty() {
        return Err(
            "this iPod holds no artwork, so there is no record to copy. Adding artwork to an \
             empty library needs the undocumented record layout."
                .into(),
        );
    }
    let new_id = records.iter().map(|r| r.id).max().unwrap_or(99) + 1;
    let slot = records.len();

    // The stores this device actually uses, taken from its own records. A store
    // it does not carry is not written, and a code not in FORMATS is refused.
    let mut stores: Vec<Format> = Vec::new();
    for record in &records {
        for mhod in record.chunk.children_of(id::MHOD) {
            if mhod.mhod_field() != Some(db::field::ARTWORK_STORE) {
                continue;
            }
            let code = mhod.u32(40).unwrap_or(0);
            let Some(format) = format_for(code) else {
                return Err(format!(
                    "this iPod stores artwork in format {code}, whose layout is not known. \
                     Refusing rather than writing pixels in a geometry that would be wrong."
                ));
            };
            if !stores.contains(&format) {
                stores.push(format);
            }
        }
    }
    if stores.is_empty() {
        return Err("this iPod's artwork database names no image store".into());
    }

    // Append the pixels first, in memory, so a failure touches nothing.
    let mut pixels = Vec::new();
    for format in &stores {
        let path = artwork_dir(root).join(format.file_name());
        let existing = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if existing.len() != slot * format.byte_len() {
            return Err(format!(
                "{} is {} bytes, which is not {slot} slots of {} — refusing to append to a store \
                 this app has not read end to end",
                path.display(),
                existing.len(),
                format.byte_len()
            ));
        }
        pixels.push((path, encode_slot(*format, &fit_cover(&image, *format))));
    }

    // Then the record: a copy of an existing one with a new id. The whole record
    // comes over — header, both store `mhod`s and the type-6 one — so the two
    // payload fields this app does not understand keep values the device itself
    // wrote for a real image.
    let template = records[0].chunk.clone();
    let mut entry = template;
    entry.set_u32(A_IMAGE_ID, new_id);
    entry.set_u64(A_DBID, dbid);
    image_list_mut(&mut tree)
        .ok_or("the artwork database lost its image list")?
        .push(entry);

    let finished = tree.to_bytes();
    db::parse(&finished)?;

    // Only now, with every byte computed and every file still untouched.
    backup(device, &db_path)?;
    config_write(&db_path, &finished);
    for (path, slot_bytes) in pixels {
        backup(device, &path)?;
        let mut whole = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        whole.extend_from_slice(&slot_bytes);
        config_write(&path, &whole);
    }
    log::info!(
        "added artwork {new_id} to the iPod: {} stores, slot {slot}",
        stores.len()
    );
    Ok(Added {
        image_id: new_id,
        stores: stores.len(),
    })
}

/// `mhii` header: where the image id sits. The child count at offset 12 is left
/// alone, because the record is cloned whole and the clone has the same children.
const A_IMAGE_ID: usize = 16;
const A_DBID: usize = 20;

/// One image record, with its id read out.
struct Record {
    id: u32,
    chunk: Chunk,
}

fn image_records(tree: &Chunk) -> Vec<Record> {
    db::list_holder(tree, 1)
        .map(|list| {
            list.children_of(id::MHII)
                .map(|c| Record {
                    id: c.u32(A_IMAGE_ID).unwrap_or(0),
                    chunk: c.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn image_list_mut(tree: &mut Chunk) -> Option<&mut Chunk> {
    db::list_holder_mut(tree, 1)
}

/// Keep the previous copy, in the cache directory like the track database's.
///
/// These are the files where a `.bak` beside the original would have been worst:
/// three stores of 2.7, 17 and 0.5 MB, so a push with new artwork put **20 MB of
/// backups on the device**, and put them there once per new cover.
fn backup(device: &Device, path: &Path) -> Result<(), String> {
    let copy = device.backup_of(path);
    if let Some(dir) = copy.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::copy(path, &copy)
        .map(|_| ())
        .map_err(|e| format!("could not keep a copy of {}: {e}", path.display()))
}

fn config_write(path: &Path, bytes: &[u8]) {
    crate::config::atomic_write_bytes(path, bytes);
}
