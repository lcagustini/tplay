//! The `iTunesDB` chunk tree — a reader and writer over the raw bytes.
//!
//! # This is not SQLite
//!
//! The obvious design for this format is a `rusqlite` reader, because the iPod
//! *family* has a SQLite library and most tooling written about it targets a
//! device from that family. An iPod Classic's `iTunesDB` is not that. It begins
//! `mhbd` and is a tree of length-prefixed chunks, and SQLite opens it without
//! complaint and then fails on the first query — with an error that names
//! SQLite and says nothing about the real cause. The fixture in
//! `tests/fixtures/ipod/` is the one this was learned from.
//!
//! # The shape
//!
//! Every chunk is the same four words:
//!
//! | offset | field |
//! |---|---|
//! | 0 | four-byte identifier |
//! | 4 | header length — where the type-specific header ends |
//! | 8 | total length, **or a child count** |
//! | 12 | the header |
//!
//! The third word is the only irregularity. `mhlt`, `mhla` and `mhlp` put a
//! child *count* there, so their extent is their parent's, and every other chunk
//! puts a *length*, so their extent is their own. Parse a count chunk by its
//! total and you read off a plausible number of songs that has nothing to do
//! with the number of songs.
//!
//! # Why the tree is owned rather than edited in place
//!
//! Writing means recomputing a length at every level: a new `mhit` grows its
//! `mhlt`, which grows its `mhsd`, which grows `mhbd`. Doing that against the
//! byte vector means a resize invalidates an offset held three frames deep.
//! Owning the tree makes every length a function of its children, so the write
//! path is a serialise and nothing else can forget to fix a parent.
//!
//! What is *not* lost is the bytes this does not understand. A chunk's header
//! and its trailing bytes are kept verbatim, so a round trip through this module
//! returns the input unchanged — which is the property
//! `a_round_trip_returns_the_input_unchanged` asserts over the real fixture, and
//! it is the only thing standing between this module and a silent rewrite of a
//! schema from a future iTunes that moved a field we never looked at.

use std::fmt;

/// Chunk identifiers, as four-byte arrays.
///
/// Spelled `b"mhbd"` rather than a string because the file holds bytes and
/// comparing a `str` against them would go through UTF-8 validation on every
/// child of every chunk.
pub mod id {
    pub const MHBD: [u8; 4] = *b"mhbd";
    /// The artwork database's root. Same grammar, different first word, so the
    /// parser takes both rather than growing a second reader.
    pub const MHFD: [u8; 4] = *b"mhfd";
    pub const MHSD: [u8; 4] = *b"mhsd";
    pub const MHLT: [u8; 4] = *b"mhlt";
    pub const MHIT: [u8; 4] = *b"mhit";
    pub const MHOD: [u8; 4] = *b"mhod";
    pub const MHLA: [u8; 4] = *b"mhla";
    pub const MHIA: [u8; 4] = *b"mhia";
    pub const MHLP: [u8; 4] = *b"mhlp";
    /// The artwork database's image list. A **count** chunk like the three
    /// above: its offset 8 reads 84 where a length would have to exceed its own
    /// 92-byte header, which is impossible.
    pub const MHLI: [u8; 4] = *b"mhli";
    /// One artwork image record.
    pub const MHII: [u8; 4] = *b"mhii";
    pub const MHYP: [u8; 4] = *b"mhyp";
    pub const MHIP: [u8; 4] = *b"mhip";
    /// The artwork database's file list, in its third data set.
    pub const MHLF: [u8; 4] = *b"mhlf";
}

/// Which chunks may be a given chunk's children.
///
/// **This table is why the artwork database parses at all.** A leaf chunk's
/// bytes after its header are arbitrary — an `mhod`'s payload in the artwork
/// database begins with the letters `mhaf` — so "does this look like a chunk"
/// is not a question with a safe answer. Only the format knows, so the format
/// says. Walking greedily on `is_ascii_lowercase` alone made the parser march
/// into a payload and then refuse the whole file, which is the right failure once
/// something has gone wrong and the wrong one to depend on.
fn children_of(parent: [u8; 4]) -> &'static [[u8; 4]] {
    match parent {
        id::MHBD | id::MHFD => &[id::MHSD],
        id::MHSD => &[id::MHLP, id::MHLT, id::MHLA, id::MHLI, id::MHLF],
        id::MHLT => &[id::MHIT],
        id::MHIT => &[id::MHOD],
        id::MHLA => &[id::MHIA],
        id::MHIA => &[id::MHOD],
        id::MHLP => &[id::MHYP],
        id::MHII => &[id::MHOD],
        id::MHLI => &[id::MHII],
        id::MHLF => &[id::MHOD],
        id::MHYP => &[id::MHOD, id::MHIP],
        id::MHIP => &[id::MHOD],
        _ => &[],
    }
}

/// The chunks that store a child count where the rest store a length.
///
/// The tell is arithmetic rather than a name: **a chunk's length is at least its
/// own header**, so a smaller number at offset 8 can only be a count. That is
/// how the last two were found — `mhli` claims 84 where a length would have had
/// to exceed its 92-byte header, and `mhlf` claims 3 against the same 92.
const COUNTS_CHILDREN: [[u8; 4]; 5] = [id::MHLT, id::MHLA, id::MHLP, id::MHLI, id::MHLF];

/// `mhod` field numbers. Only the ones this app writes or reads.
pub mod field {
    pub const TITLE: u32 = 1;
    pub const LOCATION: u32 = 2;
    pub const ALBUM: u32 = 3;
    pub const ARTIST: u32 = 4;
    pub const GENRE: u32 = 5;
    pub const FILETYPE: u32 = 6;
    pub const ALBUM_ARTIST: u32 = 22;
    pub const ARTIST_SORT: u32 = 23;
    pub const ALBUM_ARTIST_SORT: u32 = 29;
    /// The album-list variants of the three names above.
    pub const ALBUM_LIST: u32 = 200;
    pub const ARTIST_LIST: u32 = 201;
    pub const ARTIST_SORT_LIST: u32 = 202;
    /// The manual sort position that follows every playlist item. Not text: it is
    /// a small integer in the same header a string would use.
    pub const POSITION_INDICATOR: u32 = 100;
    /// In `ArtworkDB`, names one artwork store: its payload is binary, not
    /// UTF-16, and the first word is the format code.
    pub const ARTWORK_STORE: u32 = 2;
}

/// A chunk: its identifier, its header verbatim, its children, and any trailing
/// bytes that are neither header nor child.
///
/// `Clone` is the point of the whole design. Every record this app writes is a
/// copy of a record the device already holds — see `push::template_for` — so
/// "take a working `mhit` and change the fields we understand" is one line here
/// and one `set_u32` per field, instead of three hundred lines guessing at the
/// values of fields no public document describes.
#[derive(Clone)]
pub struct Chunk {
    ident: [u8; 4],
    header: Vec<u8>,
    children: Vec<Chunk>,
    trailer: Vec<u8>,
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk")
            .field("ident", &String::from_utf8_lossy(&self.ident))
            .field("header", &self.header.len())
            .field("children", &self.children.len())
            .field("trailer", &self.trailer.len())
            .finish()
    }
}

impl Chunk {
    /// A chunk with no children and no trailing bytes.
    pub fn leaf(ident: [u8; 4], header: Vec<u8>) -> Self {
        Self {
            ident,
            header,
            children: Vec::new(),
            trailer: Vec::new(),
        }
    }

    pub fn ident(&self) -> [u8; 4] {
        self.ident
    }

    /// The type-specific header, verbatim: the bytes from offset 12 to the
    /// header length. Cloning one of these is how a record is cloned.
    pub fn header(&self) -> &[u8] {
        &self.header
    }

    pub fn set_trailer(&mut self, bytes: Vec<u8>) {
        self.trailer = bytes;
    }

    pub fn set_header(&mut self, bytes: Vec<u8>) {
        self.header = bytes;
    }

    pub fn children_mut(&mut self) -> &mut Vec<Chunk> {
        &mut self.children
    }

    pub fn push(&mut self, chunk: Chunk) {
        self.children.push(chunk);
    }

    /// Every child with `ident`.
    pub fn children_of(&self, ident: [u8; 4]) -> impl Iterator<Item = &Chunk> {
        self.children.iter().filter(move |c| c.ident == ident)
    }

    /// `len` bytes at absolute offset `off` within this chunk, across the header
    /// and the trailer. `None` when the range is not inside the chunk.
    pub fn bytes_at(&self, off: usize, len: usize) -> Option<&[u8]> {
        let start = off.checked_sub(12)?;
        let header_len = self.header.len();
        if start + len <= header_len {
            return Some(&self.header[start..start + len]);
        }
        let into_trailer = start.checked_sub(header_len)?;
        self.trailer.get(into_trailer..into_trailer + len)
    }

    fn bytes_at_mut(&mut self, off: usize, len: usize) -> Option<&mut [u8]> {
        let start = off.checked_sub(12)?;
        let header_len = self.header.len();
        if start + len <= header_len {
            return Some(&mut self.header[start..start + len]);
        }
        let into_trailer = start.checked_sub(header_len)?;
        let end = into_trailer.checked_add(len)?;
        self.trailer.get_mut(into_trailer..end)
    }

    pub fn u32(&self, off: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes_at(off, 4)?.try_into().ok()?))
    }

    pub fn u8(&self, off: usize) -> Option<u8> {
        Some(*self.bytes_at(off, 1)?.first()?)
    }

    pub fn set_u32(&mut self, off: usize, value: u32) -> bool {
        match self.bytes_at_mut(off, 4) {
            Some(b) => {
                b.copy_from_slice(&value.to_le_bytes());
                true
            }
            None => false,
        }
    }

    pub fn set_u16(&mut self, off: usize, value: u16) -> bool {
        match self.bytes_at_mut(off, 2) {
            Some(b) => {
                b.copy_from_slice(&value.to_le_bytes());
                true
            }
            None => false,
        }
    }

    pub fn set_u8(&mut self, off: usize, value: u8) -> bool {
        match self.bytes_at_mut(off, 1) {
            Some(b) => {
                b[0] = value;
                true
            }
            None => false,
        }
    }

    pub fn set_u64(&mut self, off: usize, value: u64) -> bool {
        match self.bytes_at_mut(off, 8) {
            Some(b) => {
                b.copy_from_slice(&value.to_le_bytes());
                true
            }
            None => false,
        }
    }

    pub fn u64(&self, off: usize) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes_at(off, 8)?.try_into().ok()?))
    }

    /// An `mhod` child's payload, decoded.
    pub fn mhod_text(&self) -> Option<String> {
        if self.ident != id::MHOD {
            return None;
        }
        let len = self.u32(28)? as usize;
        let bytes = self.bytes_at(40, len)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16(&units).ok()
    }

    /// This `mhod`'s field number.
    pub fn mhod_field(&self) -> Option<u32> {
        self.u32(12)
    }

    /// Find the first `mhod` child carrying `field`.
    pub fn mhod(&self, field: u32) -> Option<&Chunk> {
        self.children_of(id::MHOD)
            .find(|c| c.mhod_field() == Some(field))
    }

    /// [`Chunk::mhod`], mutably.
    pub fn mhod_mut(&mut self, field: u32) -> Option<&mut Chunk> {
        self.children
            .iter_mut()
            .find(|c| c.ident == id::MHOD && c.mhod_field() == Some(field))
    }

    /// Serialise the tree.
    pub fn to_bytes(&self) -> Vec<u8> {
        let header_len = 12 + self.header.len();
        let body: usize =
            self.children.iter().map(Chunk::byte_len).sum::<usize>() + self.trailer.len();
        let total = header_len + body;
        let third = if COUNTS_CHILDREN.contains(&self.ident) {
            self.children.len()
        } else {
            total
        };
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&self.ident);
        out.extend_from_slice(&(header_len as u32).to_le_bytes());
        out.extend_from_slice(&(third as u32).to_le_bytes());
        out.extend_from_slice(&self.header);
        for child in &self.children {
            out.extend_from_slice(&child.to_bytes());
        }
        out.extend_from_slice(&self.trailer);
        out
    }

    /// How many bytes this chunk serialises to. Not `len` on purpose: a name
    /// that reads as a container length invites `is_empty` beside it, and the
    /// question does not exist here — the smallest chunk in the format is a
    /// 24-byte `mhod` header, so one is never empty.
    pub fn byte_len(&self) -> usize {
        12 + self.header.len()
            + self.children.iter().map(Chunk::byte_len).sum::<usize>()
            + self.trailer.len()
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

/// Parse a whole `iTunesDB` or `ArtworkDB`.
///
/// Both are the same chunk tree under different first words, so one reader takes
/// both; a second reader would be a second thing to keep right about a format
/// that is the same format.
pub fn parse(bytes: &[u8]) -> Result<Chunk, String> {
    if bytes.len() < 12 {
        return Err("file is too short to be an iPod database".into());
    }
    let magic: [u8; 4] = bytes[..4].try_into().unwrap_or_default();
    if magic != id::MHBD && magic != id::MHFD {
        return Err("this is not an iPod database: it starts with neither mhbd nor mhfd".into());
    }
    let total = read_u32(bytes, 8)? as usize;
    if total != bytes.len() {
        return Err(format!(
            "iPod database says it is {total} bytes but the file is {}",
            bytes.len()
        ));
    }
    parse_chunk(bytes, 0, bytes.len())
}

/// Read the child `count`-chunked list holder of a given type inside `root`.
///
/// The albums, tracks, podcasts and the two playlist lists are siblings under
/// one `mhbd`, told apart by a type number in the `mhsd` header at offset 12.
/// Walking them by index would mean knowing the order iTunes happened to write,
/// which is not a thing the format promises.
pub fn list_holder(root: &Chunk, kind: u32) -> Option<&Chunk> {
    root.children_of(id::MHSD)
        .find(|c| c.u32(12) == Some(kind))
        .and_then(|c| c.children.first())
}

/// As [`list_holder`], mutably.
pub fn list_holder_mut(root: &mut Chunk, kind: u32) -> Option<&mut Chunk> {
    root.children
        .iter_mut()
        .find(|c| c.ident == id::MHSD && c.u32(12) == Some(kind))
        .and_then(|c| c.children.first_mut())
}

/// The `mhsd` kinds this app names.
pub mod kind {
    pub const TRACKS: u32 = 1;
    pub const PLAYLISTS: u32 = 2;
    pub const ALBUMS: u32 = 4;
}

fn read_u32(bytes: &[u8], off: usize) -> Result<u32, String> {
    bytes
        .get(off..off + 4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| format!("iPod database ended inside the field at {off}"))
}

/// Parse one chunk starting at `start`, whose parent's children end at `limit`.
///
/// `limit`, not a length from the file, is what bounds a count-chunk's children:
/// it has none of its own.
fn parse_chunk(bytes: &[u8], start: usize, limit: usize) -> Result<Chunk, String> {
    let ident: [u8; 4] = bytes
        .get(start..start + 4)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("iPod database ended inside a chunk id at {start}"))?;
    let header_len = read_u32(bytes, start + 4)? as usize;
    let third = read_u32(bytes, start + 8)? as usize;
    let counts = COUNTS_CHILDREN.contains(&ident);

    let end = if counts { limit } else { start + third };
    if header_len < 12 || start + header_len > end || end > bytes.len() {
        return Err(format!(
            "chunk {} at {start} claims a {header_len}-byte header inside {end} bytes",
            String::from_utf8_lossy(&ident)
        ));
    }
    let header = bytes[start + 12..start + header_len].to_vec();

    let allowed = children_of(ident);
    let mut children = Vec::new();
    let mut pos = start + header_len;
    while pos + 12 <= end {
        let child_ident: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap_or_default();
        if !allowed.contains(&child_ident) {
            break;
        }
        let child = parse_chunk(bytes, pos, end)?;
        pos += child.byte_len();
        children.push(child);
    }

    // Whatever is left after the last child is kept rather than dropped: an
    // `mhod`'s string payload lives there, and a future iTunes may put
    // something else there that this module has no business discarding.
    let trailer = if children.is_empty() {
        bytes[start + header_len..end].to_vec()
    } else {
        bytes[pos..end].to_vec()
    };

    Ok(Chunk {
        ident,
        header,
        children,
        trailer,
    })
}
