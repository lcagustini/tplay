//! The **HASH58** signature on an iPod's music database.
//!
//! The iPod Classic (and the Nano 3G/4G) refuse to show a database whose
//! signature does not match. A wrong signature has one symptom: the device shows
//! no music at all, and the files are still on the disk, and nothing logs
//! anything. So this is the part of the format that has to be right, and it is
//! the part with a known-answer test (`tests/ipod_tests.rs` checks this against
//! a real device's own signature).
//!
//! # What it is
//!
//! An HMAC-SHA1 over the **whole file**, keyed from the device's 8-byte FireWire
//! GUID. The signature lives at `mhbd` offset `0x58`.
//!
//! The input is not the file as it sits on disk. Three regions are normalised
//! first, and the normalisation is what makes the signature reproducible:
//!
//! | offset | what | value while hashing |
//! |---|---|---|
//! | `0x18` | database id | zeroed, then restored |
//! | `0x32` | unknown | zeroed, then restored |
//! | `0x58` | the signature itself | zeroed |
//! | `0x30` | hashing scheme | set to `1` |
//!
//! `0x30` is not a constant of the algorithm, it is the switch the device reads
//! to decide *which* scheme signed this file. `1` means HASH58, which is what an
//! iPod Classic uses and what every file this module writes carries.
//!
//! # The key
//!
//! The GUID is not used directly. Each pair of its bytes goes through
//! `lcm(a, b)`, and the two halves of that value index the AES S-box and the AES
//! inverse S-box. Those sixteen bytes, prefixed by a fixed 18-byte seed, are
//! SHA-1'd; the 20-byte digest is the first 20 bytes of a 64-byte HMAC key, and
//! the remaining 44 are zero — which is why HMAC does not have to pad it, since
//! a 64-byte key is already the padded length.
//!
//! The two S-boxes are **computed**, not pasted. The alternative was 512 bytes
//! of table copied from another implementation, which a reader can only take on
//! faith; computing them from the AES definition is the same number of lines and
//! `sbox_agrees_with_the_published_table` pins the result against the values
//! everyone else copied.

use sha1::{Digest, Sha1};

/// The 8-byte FireWire GUID, as it comes out of `device::guid`.
pub type Guid = [u8; 8];

/// Offset of the signature inside the `mhbd` header.
const OFFSET_HASH58: usize = 0x58;
/// Offset of the switch the device reads to pick a scheme.
const OFFSET_SCHEME: usize = 0x30;
/// Offset of the database id, which is zeroed while hashing and restored after.
const OFFSET_DB_ID: usize = 0x18;
/// Offset of the 20 unknown bytes, zeroed while hashing and restored after.
const OFFSET_UNK32: usize = 0x32;

/// The scheme byte value that means HASH58.
const SCHEME_HASH58: u16 = 1;

/// The 18-byte seed the key derivation prefixes to the S-box output.
const SEED: [u8; 18] = [
    0x67, 0x23, 0xFE, 0x30, 0x45, 0x33, 0xF8, 0x90, 0x99, 0x21, 0x07, 0xC1, 0xD0, 0x12, 0xB2, 0xA1,
    0x07, 0x81,
];

/// Multiply in GF(2^8) modulo the AES polynomial.
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            product ^= a;
        }
        let carry = a & 0x80;
        a <<= 1;
        if carry != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    product
}

/// The multiplicative inverse in GF(2^8). `0` is its own inverse here because
/// the derivation divides by a byte that may be `0`, and a zero byte must not
/// make the whole key zero.
fn gf_inverse(a: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    // a^254 == a^-1, because a^255 == 1 for every nonzero element.
    let mut result = 1u8;
    let mut base = a;
    let mut exponent = 254u32;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = gf_mul(result, base);
        }
        base = gf_mul(base, base);
        exponent >>= 1;
    }
    result
}

/// One entry of the AES forward S-box: invert in GF(2^8), then the affine map.
fn sbox(value: u8) -> u8 {
    let b = gf_inverse(value);
    b ^ b.rotate_left(1) ^ b.rotate_left(2) ^ b.rotate_left(3) ^ b.rotate_left(4) ^ 0x63
}

/// The forward S-box and its inverse, derived once.
///
/// `LazyLock` for the same reason `audio/viz.rs` holds its window there: the
/// shape is fixed by two constants, so building it on every call would be
/// recomputing a value that cannot change.
static SBOX: std::sync::LazyLock<([u8; 256], [u8; 256])> = std::sync::LazyLock::new(|| {
    let mut forward = [0u8; 256];
    let mut inverse = [0u8; 256];
    for i in 0..=255u8 {
        let mapped = sbox(i);
        forward[i as usize] = mapped;
        inverse[mapped as usize] = i;
    }
    (forward, inverse)
});

/// The 64-byte HMAC key for `guid`.
///
/// Public only so a test can show the derivation depends on all eight bytes and
/// on nothing outside them.
pub fn derive_key(guid: Guid) -> [u8; 64] {
    let (forward, inverse) = &*SBOX;
    let mut mixed = [0u8; 16];
    for i in 0..4 {
        let a = u32::from(guid[i * 2]);
        let b = u32::from(guid[i * 2 + 1]);
        let lcm = if a == 0 || b == 0 {
            1
        } else {
            a * b / gcd(a, b)
        };
        let (hi, lo) = ((lcm >> 8) as u8, lcm as u8);
        mixed[i * 4] = forward[hi as usize];
        mixed[i * 4 + 1] = inverse[hi as usize];
        mixed[i * 4 + 2] = forward[lo as usize];
        mixed[i * 4 + 3] = inverse[lo as usize];
    }

    let mut hasher = Sha1::new();
    hasher.update(SEED);
    hasher.update(mixed);
    let mut key = [0u8; 64];
    key[..20].copy_from_slice(&hasher.finalize());
    key
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// HMAC-SHA1 with a key already at or over the 64-byte block length.
fn hmac_sha1(key: &[u8; 64], message: &[u8]) -> [u8; 20] {
    let mut inner_pad = [0u8; 64];
    let mut outer_pad = [0u8; 64];
    for (i, byte) in key.iter().enumerate() {
        inner_pad[i] = byte ^ 0x36;
        outer_pad[i] = byte ^ 0x5C;
    }
    let mut inner = Sha1::new();
    inner.update(inner_pad);
    inner.update(message);
    let mut outer = Sha1::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The signature `guid` demands over `database`.
///
/// `database` must already have the three covered regions zeroed and the scheme
/// set, i.e. it must be what [`sign`] hashes. Prefer [`sign`] or [`verify`].
pub fn calculate(guid: Guid, database: &[u8]) -> [u8; 20] {
    hmac_sha1(&derive_key(guid), database)
}

/// Write `guid`'s signature into `database`, in place.
///
/// The two regions that were zeroed for hashing go back to what they were, so
/// the only byte that changes on disk is the signature.
pub fn sign(guid: Guid, database: &mut [u8]) -> Result<(), String> {
    if !database.starts_with(b"mhbd") {
        return Err("not an iPod database: it does not start with mhbd".into());
    }
    if database.len() < OFFSET_HASH58 + 20 {
        return Err("iPod database is too short to hold a signature".into());
    }
    // Refuse rather than convert. A device that signed with another scheme will
    // reject a HASH58 file, and the refusal happens here instead of on the
    // device — where the symptom is "no music" and nothing says why.
    if let Some(found) = scheme(database) {
        if found != SCHEME_HASH58 {
            return Err(format!(
                "this iPod signs its database with scheme {found}, not HASH58 (1)"
            ));
        }
    }
    let db_id: [u8; 8] = database[OFFSET_DB_ID..OFFSET_DB_ID + 8]
        .try_into()
        .map_err(|_| "could not read the database id".to_string())?;
    let unk32: [u8; 20] = database[OFFSET_UNK32..OFFSET_UNK32 + 20]
        .try_into()
        .map_err(|_| "could not read the database's unknown region".to_string())?;

    database[OFFSET_DB_ID..OFFSET_DB_ID + 8].fill(0);
    database[OFFSET_UNK32..OFFSET_UNK32 + 20].fill(0);
    database[OFFSET_HASH58..OFFSET_HASH58 + 20].fill(0);
    database[OFFSET_SCHEME..OFFSET_SCHEME + 2].copy_from_slice(&SCHEME_HASH58.to_le_bytes());

    let signature = calculate(guid, database);

    database[OFFSET_HASH58..OFFSET_HASH58 + 20].copy_from_slice(&signature);
    database[OFFSET_DB_ID..OFFSET_DB_ID + 8].copy_from_slice(&db_id);
    database[OFFSET_UNK32..OFFSET_UNK32 + 20].copy_from_slice(&unk32);
    Ok(())
}

/// Whether `database` carries `guid`'s signature.
pub fn verify(guid: Guid, database: &[u8]) -> bool {
    let mut copy = database.to_vec();
    sign(guid, &mut copy).is_ok()
        && copy[OFFSET_HASH58..OFFSET_HASH58 + 20] == database[OFFSET_HASH58..OFFSET_HASH58 + 20]
}

/// Which scheme the device says signed this file.
///
/// Anything that is not HASH58 is a device this module must refuse to write
/// rather than guess at: `0x4` is hashAB, which is a white-box AES blob and not
/// something this module can reproduce at all.
pub fn scheme(database: &[u8]) -> Option<u16> {
    let bytes = database.get(OFFSET_SCHEME..OFFSET_SCHEME + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}
