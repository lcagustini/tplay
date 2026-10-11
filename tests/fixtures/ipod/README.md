# iPod fixtures

Three files from one **iPod Classic 7G** (119 GB, FAT32), used by
`tests/ipod_tests.rs`. Read them with `tplay::ipod`.

| file | bytes | what it is |
|---|---|---|
| `iTunesDB` | 1 915 544 | the music database, verbatim except for the text described below |
| `ArtworkDB` | 65 244 | the artwork database, **verbatim** |
| `SysInfo` | 0 | **empty on purpose** — see "Why SysInfo is empty" |

The `.ithmb` stores are **not** here. They are 20 MB of somebody's album art,
and no test needs a real photograph: what matters is the geometry, which
`tests/ipod_tests.rs` derives from the file lengths recorded below, and the
packing, which its own generated pixels cover. A store of zeros is a legal slot.

## Why these are files and not generated

Same reason as the eight audio files one directory up: nothing in the dependency
tree can produce this format. The format is not documented by the vendor, its
schema changed without notice between releases, and every constant in it was
measured from a real device rather than read anywhere. A hand-built file would be
a guess at the format wearing the costume of a test.

This one is worth more than the audio fixtures, because there is no second
source to check it against. `tests/audio_fixtures.rs` can assert that a decoder
and a tag reader agree on a duration, because two independent readers exist.
Here the only other implementation in the tree is the one under test.

## What is in it

Measured, not assumed:

| | |
|---|---|
| `mhbd` header length | `0xF4` (244) |
| database version | `0x75` (117) |
| hashing scheme at `0x30` | `1` — **hash58** |
| signature at `0x58` | 20 bytes, and it verifies (below) |
| tracks | 1169 |
| albums | 80 |
| playlist list holders | 3 (`mhsd` types 2, 3 and 5) |
| playlists | 8 `mhyp` across them |
| playlist items | 2338 `mhip` |
| `mhod` records | 13324 |
| artwork records | 84 `mhii` |
| artwork stores | 3, **none of them signed** |

Three of those numbers contradict the only public description of the format
(wikiPodLinux's `ITunesDB` page), and the contradictions are the reason this file
is here:

- **`iTunesDB` is not SQLite.** It begins `mhbd`. The iOS-family devices use a
  SQLite library in `iTunes Library.itlp/`, and that is the layout most tooling
  writes about. A `rusqlite`-based reader opens this file as an ordinary SQLite
  database, fails, and produces an error that says nothing about the real cause.
- **The `mhit` header is `0x270` (624) bytes**, not the `0x184` the wiki gives
  for iTunes 7.1. Fields past about `0x100` have moved.
- **The album link is at `mhit` offset `288`,** not `314` where the wiki puts
  `AlbumID`. Measured against all 1169 tracks: offset 288 carries an album id
  that resolves to the right `mhia` for 1165 of them. Offset 314 resolves for
  none.

A writer built from the wiki alone would produce a file that parses and a device
that shows nothing.

## The artwork half, and why `ArtworkDB` is committed verbatim

`ArtworkDB` is the same chunk tree under an `mhfd` root and it is **not
sanitised**, because it holds no prose: an image record is an id, a 64-bit
`dbid`, and three store descriptors that are geometry. There is no title, no
filename and no picture in it. The dbids are opaque random values and they are
already in the `iTunesDB` fixture, so changing them would only make the two files
disagree with each other.

What it does establish, and what the `.ithmb` files it points at are for:

| store | geometry | bytes per slot | file |
|---|---|---|---|
| 1055 | 128×128 | 32 768 | `F1055_1.ithmb` — 2 752 512 |
| 1060 | 320×320 | 204 800 | `F1060_1.ithmb` — 17 203 200 |
| 1061 | **55×55, stride 56** | 6 160 | `F1061_1.ithmb` — 517 440 |

Each file length divides by its slot size to give 84 — one slot per record — and
the slots hold the same images in the same order, so record *N* is slot *N*. The
pixels are RGB565 little-endian, which was established by decoding slot 0 under
each candidate layout and rendering it: one of them was a photograph.

**The pictures themselves are not committed.** They are 20 MB of somebody's album
art, and no test needs them — the geometry is pinned by the file lengths above and
the packing by generated pixels. A store of zeros is a legal slot, which is what
`tests/ipod_tests.rs` seeds.

## hash58 verifies against this file

The signature at `0x58` is an HMAC-SHA1 over the whole file, keyed from the
device's 8-byte FireWire GUID, with the `db_id` at `0x18`, the 20 bytes at `0x32`
and the signature itself zeroed, and the scheme field at `0x30` set to `1`.

Computing that over this file with GUID `000A27002158F9D4` reproduces the stored
20 bytes exactly. So `ipod::hash58` is pinned against a real device's own output
rather than against itself, which is the difference between a round trip and a
check.

## Why SysInfo is empty

It is 0 bytes on the device it came from, and that is **kept** rather than fixed.

`SysInfo` is where the FireWire GUID normally lives, and it is where a tool
looks for it first. This device has none, so a reader that only reads `SysInfo`
has no way to sign anything, and every write fails with a checksum error whose
message points at the wrong thing.

The GUID is the **USB serial number**, which the kernel already exposes through
`/dev/disk/by-label` and `udevadm`. `ipod::device` reads `SysInfo` first and
falls back to `udevadm`; keeping the empty file is what keeps that fallback
tested on a machine whose real device *does* lack the GUID.

## What was changed in it, and what was not

The owner's library is not in this repository. Every `mhod` whose payload
decodes as UTF-16 text was overwritten with cycling uppercase letters, of exactly
the same byte length, so every offset and every chunk size in the file is
unchanged. The file was then re-signed with the known GUID, which is why it still
verifies.

The one exception is `mhod` type 2, the location string
`:iPod_Control:Music:Fxx:NAME.ext`. It is kept, because it is structure rather
than prose: the `Fxx` and the four random letters are chosen by iTunes, not by
the owner, and keeping them preserves the real distribution across the fifty
folders.

One thing is **not** removed: the FireWire GUID `000A27002158F9D4` is this
device's serial number. It has to stay, because a signature is meaningless
without the key that produced it. Treat this fixture, and this repository, as
carrying that one identifier.

### A whitelist leaked, which is why the README says how this was done

The first attempt rewrote a fixed list of `mhod` types. It leaked: type `0x27`
(39) holds the U+2117 SOUND RECORDING COPYRIGHT line and is not in the wiki's
type list at all, so `℗ 2021 Universal Music Ltda` survived into the file. The
rewrite therefore decides by **decoding**, not by type — anything that decodes
as UTF-16 prose is replaced — so an unlisted type cannot hide a title.

Twelve `mhod` records keep non-letter bytes after the rewrite: types 50 and 53
are binary smart-playlist and index blobs, and a few are tab padding. None
carries text.

## Do not regenerate this

Re-deriving it needs a real device and the sanitiser, and the sanitiser is a
one-shot: the shape of the rewrite is recorded above, not scripted, because a
committed script would be a second thing to keep true about a fixture nobody
should be regenerating.

If a replacement is ever needed, take it from a device of your own, run the same
rewrite over every `mhod` that decodes as text, re-sign it with `ipod::hash58`,
and update the counts in this file. Do not commit a database that still holds
somebody's music.

## Do not commit this device's music again

The whole point of `iTunes_Control/iTunes/iTunesDB` is that it names every track
the owner has. A fixture dump is a library export, and the accident is one
`cp` away. Sanitise before the file reaches `git add`, not after.
