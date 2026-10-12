---
name: ipod-database
description: |
  Comprehensive reference for Apple iPod database (iTunesDB/ArtworkDB) format:
  chunk tree grammar, every header field, version and device variations,
  Hash58/hashAB signing, artwork stores, smart playlist rules, and
  common parse/sign/modify operations. Maintained against the tplay fixture
  (iPod Classic 7G, v0x75, 1169 tracks) and cross-referenced with
  iOpenPod, podkit, and ipodsync reference implementations.
---

# iPod Database Reference

## Chunk Grammar (shared by iTunesDB and ArtworkDB)

Every chunk is three words plus a type-specific header:

| Offset | Size | Meaning |
|--------|------|---------|
| 0 | 4 | Chunk identifier (four lowercase letters: `mhbd`, `mhit`, `mhod`...) |
| 4 | 4 | **Header length** — where the type-specific header ends (relative to offset 0) |
| 8 | 4 | **Total length** — either the chunk's byte extent **or** the number of top-level children |
| 12 | — | Type-specific header data |

- **Count chunks** store a child count at offset 8 instead of a length: `mhlt` (track list), `mhla` (album list), `mhlp` (playlist list). Their extent is bounded by the parent.
- **Length chunks** store a byte extent; the parser uses it as the end offset.
- All integers are **little-endian**. Text strings are **UTF‑16LE**, length‑prefixed, not null‑terminated.

## Root: `mhbd` (iPod music DB), `mhfd` (artwork DB)

|iPodDB|Field|Offset|Size|Value / meaning|
|------|-----|------|----|---------------|
||Header ID|0|4|`mhbd` or `mhfd`|
||Header length|4|4|`0xF4` (244) for v0x75 fixture; `0x68` for ≤ 7.2, `0xBC` for ≥ 7.3|
||Total length|8|4|File size (1 915 544 in the fixture)|
||Unknown|12|4|Always `1`|
||Version|16|4|`0x75` = iTunes 7.5 / iPod Classic 7G|
||Children count|20|4|5 (five `mhsd` data sets)|
||Database ID|24|8|`0xCFCA8826B74BF09B`|
||Unknown|32|4|Always `0x2630001`|
||Unknown|38|8|Present from dbversion 0x11|
||Unknown|48|2|`0x01` for Nano 3G/Classic (dbversion 0x19+)|
||Unknown|50|20|dbversion 0x19+|
||Language ID|70|2|`0x65 0x6E` = "en"|
||Library persistent ID|72|8|Matches iTunes drag-object XML|
||Obscure hash|88|20|dbversion 0x19+ (iPod Nano 3G/Classic)|
||Hash scheme|0x30|2|`1` = **HASH58**; `4` = hashAB (white‑box AES, unreadable)|
||Signature|0x58|20|HMAC‑SHA1, see below|
||Compressed flag|0xA8|2|`1` = iTunesCDB zlib-framed|
||HashAB|0xAB|57|White‑box AES key+output for hashAB scheme|
||Audio language|0xA0|2|Secondary audio language code|
||Subtitle language|0xA2|2|Secondary subtitle language code|
||Unknown 0x50|0x50|4|Must be `1` on Classic|
||Unknown 0x54|0x54|4|Must be `15` on Classic|
||Hash72|0x72|46|Additional hash for some devices|
||Hash type indicator|0x70|2|Type of hash scheme in use|

**Data sets (`mhsd`)** are children of `mhbd`, distinguished by the `type` field at offset 12:

| Value | Meaning |
|-------|---------|
| 1 | Track list → `mhlt` |
| 2 | Playlist list → `mhlp` |
| 3 | Podcast list → `mhlp` (organized differently) |
| 4 | Album list → `mhla` (iTunes 7.1+) |
| 5 | New playlist list (smart playlists, iTunes 7.3+) |

## Hash58 Signature (iPod Classic / Nano 3G / 4G)

- HMAC‑SHA1 over the **entire file**.
- Regions zeroed before hashing:
  - `0x18‑0x1F` (db ID) → restored afterward
  - `0x32‑0x45` (20 unknown bytes) → restored
  - `0x58‑0x6C` (the signature) → written afterward
- Set scheme byte at `0x30` = `1` before hashing; restore everything except the signature.
- Key derivation from the 8‑byte FireWire GUID:
  - Pair each two consecutive GUID bytes through `lcm(a,b)`; hi‑byte and lo‑byte index the AES forward S‑box and inverse S‑box respectively (16 output bytes).
  - Prefix a fixed 18‑byte seed and SHA‑1 the result; first 20 bytes = key, remaining 44 = zero.
  - Compute HMAC‑SHA1 with this 64‑byte key (no padding needed).
- The GUID is read from `iPod_Control/Device/SysInfo` (`FirewireGuid: 0x…`) **or** from `udevadm` `ID_SERIAL_SHORT` (the USB serial IS the GUID; this fallback matters because many Classic devices have a zero‑byte `SysInfo`).
- **Always verify the resulting signature against the stored 20 bytes** (not just round‑trip). Any other scheme (`4` = hashAB) must be refused — a wrong signature means the device shows *no music* with no error.

## HashAB (Alternative Signature Scheme)

- Also HMAC‑SHA1 over the **entire file**, but uses a different key derivation from the FireWire GUID.
- The key derivation loads a 57‑byte blob from offset `0xAB` in the mhbd header (`hashab` field).
- Set scheme byte at `0x30` = `4` before hashing; restore everything except the signature.
- Any other scheme (`1` = hash58) must be refused — a wrong scheme means the device silently ignores the database.
- See `libgpod hashab.c` and ipodsync `_hashab_gen.py` / `_hashab_gen_rt.py` / `_hashab_tables.bin` for reference implementations.

## Extended Header Fields (dbversion ≥ 0x13)

| mhbd offset | Field | Meaning |
|-------------|-------|---------|
| 0xA0 | Audio language | Secondary audio language code |
| 0xA2 | Subtitle language | Secondary subtitle language code |
| 0xA4 | Unknown | Variable |
| 0xA6 | Unknown | Variable |
| 0xA8 | CDB flag | `1` = iTunesCDB zlib-framed format |
| 0xAB | hashAB | 57‑byte blob for hashAB scheme |
| 0x70 | Hash type indicator | Type selector for hash scheme |

## Track Item: `mhit` (header size varies by dbversion)

|iPodDB|Field|Offset|Size|Meaning|
|------|-----|------|----|-------|
|Header ID|`mhit`|0|4|type|
|Header length||4|`0x270` (624) for v0x75|
|Total length||8|header + children|
|String count||12|number of `mhod` children|
|Track ID|id|16|4|unique track id|
|Visible|visible|20|4|`1` = shown, `0` = hidden|
|File type|filetype|24|4|**reversed** 4‑char code: MP3=` 3PM`, AAC=` A4M`, M4A=` A4M`|
|Type 1|type1|28|1|CBR=`0x00`, VBR=`0x01`|
|Type 2|type2|29|1|codec flags|
|Compilation|compilation|30|1|`1` = compilation|
|Rating|rating|31|1|stars × 20|
|Last modified|last modified|32|4|Mac epoch (sec since 1/1/1904)|
|Size|size|36|4|file size in bytes|
|Duration|length|40|4|milliseconds|
|Track number|track number|44|4|e.g. `9` of `15`|
|Total tracks|total tracks|48|4|
|Year|year|52|4|
|Bitrate|bitrate|56|4|kbps|
|Sample rate|sample rate|60|4|`44100` × `0x10000`|
|Volume|volume|64|4|±255 adjustment|
|Start/Stop|start time|68|4|ms; stop time at 72|
|SoundCheck|soundcheck|76|4|`X = 1000·10^(−0.1·Y)`|
|Play count|play count|80|4|
|Play count 2|play count 2|84|4|
|Last played|last played|88|4|
|Disc / total|disc number|92|4|disc/total discs|
|User ID|userid|100|4|Apple Store/Audible (DRM) |
|Date added|date added|104|4|
|Bookmark|bookmark time|108|4|audiobook resume|
|DBID|dbid|112|8|unique 64‑bit id joining with `ArtworkDB`|
|Checked|checked|120|1|`0` = checked|
|App rating|application rating|121|1|previously‑synced rating|
|BPM|BPM|122|2|
|Artwork count|artwork count|124|2|must be ≥ 1|
|Unknown|unk9|126|2|`0xFFFF` MP3/AAC, `0x0` WAV|
|Artwork size|artwork size|128|4|
|Sample rate 2|sample rate 2|136|4|IEEE 32‑bit float|
|Date released|date released|140|4|
|Codec marker|unk14/1|144|2|MP3=`0x0C`, AAC=`0x33`, Audible=`0x29`, WAV=`0x00`|
|Played flag|unk14/2|146|2|
|DRM|unk15|148|4|Apple Store DRM (`0x01010100`)|
|Skip count|skip count|156|4|
|Last skipped|last skipped|160|4|
|Has artwork|has_artwork|164|1|`0x01` has art, `0x02` none|
|Skip when shuffling|skip_when_shuffling|165|1|
|Remember playback position|remember_playback_position|166|1|
|Podcast flag|flag4|167|1|
|DBID 2|dbid2|168|8|= dbid for dbversion < 0x12, then independent|
|Lyrics|lyrics flag|176|1|USLT present|
|Movie|movie file flag|177|1|
|Played mark|played_mark|178|1|`0x01` not‑played bullet|
|Pregap|pregap|184|4|silence samples before|
|Sample count|sample count|188|8|samples in song|
|Postgap|postgap|200|4|silence samples after|
|Media type|media type|208|4|`1` audio, `2` video, `0x08` audiobook|
|Season / episode|season number|212|4|
|Episode number|episode number|216|4|
|Gapless data|gaplessData|248|4|synch‑frame span (MP3 gapless)|
|Gapless flags|gaplessTrackFlag|256|2|`1` = has gapless data|
||gaplessAlbumFlag|258|2|`1` = no crossfade|
|Album ID|**AlbumID**|**288**|4|**at offset 288, NOT 314 as wiki claims**|
|mhii‑link|mhii‑link|352|4|offset of `ArtworkDB` `mhii` for slideshow|
|Strings|`mhod` children|356+|—|string records|

### Extended mhit fields (header ≥ 0xf4 ≈ 244 bytes, iTunes 7.2+)

| Offset | Size | Field |
|--------|------|-------|
| 156 | 4 | Skip count |
| 160 | 4 | Last skipped |
| 164 | 1 | Has artwork (`0x01` has art, `0x02` none) |
| 165 | 1 | Skip when shuffling |
| 166 | 1 | Remember playback position |
| 167 | 1 | Flag 4 (podcast) |
| 168 | 8 | DB ID 2 (independent from dbid for dbversion ≥ 0x12) |
| 176 | 1 | Lyrics flag |
| 177 | 1 | Movie file flag |
| 178 | 1 | Played mark |
| 179 | 1 | Always `0` (since dbversion 0x0c) |
| 180 | 4 | Unknown |
| 184 | 4 | Gapless pregap |
| 188 | 8 | Sample count |
| 192 | 4 | Unknown |
| 196 | 4 | Unknown |
| 200 | 4 | Postgap |
| 204 | 4 | Unknown |
| 208 | 4 | Media type (`1`=audio, `2`=video, `0x08` audiobook, `0x20` music video, `0x60` TV show) |
| 212 | 4 | Season number |
| 216 | 4 | Episode number |
| 220‑240 | 24 | 6 × uint32 unknown |
| 248 | 4 | Gapless track flag (`1` = has gapless data) |
| 252 | 4 | Unknown |
| 256 | 2 | Gapless album flag (`1` = no crossfade) |
| 258 | 20 | Unknown (hash) |
| 260 | — | (format‑specific fields) |

### Extended mhit fields (header ≥ 0x148 ≈ 328 bytes, iTunes 7.0+)

| Offset | Size | Field |
|--------|------|-------|
| 244 | 4 | Unknown |
| 248 | 4 | Gapless data |
| 252 | 4 | Unknown |
| 256 | 2 | Gapless track flag |
| 258 | 2 | Gapless album flag |

### Field numbers for `mhod` string records (offset 12 of mhod header)

| Type | Name |
|------|------|
| 1 | Title |
| 2 | Location — **must have a position field of `1`**; the track otherwise won't play |
| 3 | Album |
| 4 | Artist |
| 5 | Genre |
| 6 | File type |
| 8 | Comment |
| 12 | Composer |
| 14 | Description (podcast show notes) |
| 15 / 16 | Podcast enclosure / RSS URL (UTF‑8, no length field) |
| 17 | Chapter data |
| 19‑21 | TV show network / show / episode |
| 22 | Album artist |
| 23 | Artist sort ("Beatles, The") |
| 24 | Keywords |
| 27 / 28 / 29 | Title/Album/Artist sort variants |
| 50 | Smart playlist data |
| 51 | Smart playlist rules (see below) |
| 52 | Library playlist index |
| 53 | Letter jump table |
| 100 | Position indicator — the integer following every `mhip` |
| 200‑203 | Album‑list strings |

### String Record: `mhod` (any type)

|iPodDB|Field|Offset|Size|Meaning|
|------|-----|------|----|-------|
||Header ID|0|4|`mhod`|
||Header length|4|4|`0x18` for string records|
||Total length|8|4|header + string|
||Type|12|4|field number (see table above)|
||String length|28|4|bytes (UTF‑16 = 2 bytes/char), not null‑terminated|
||String|40|length|UTF‑16LE payload|

### Playlist: `mhlp` → `mhyp` → `mhip`

- `mhlp`: header length + **playlist count** at offset 8 (includes Library).
- `mhyp` (playlist): total length; data‑object count; **item count**; master flag (offset 20, `1` = Library); 3 flag bytes; timestamp; persistent playlist ID (8‑byte random); unknown; string count; podcast flag; list sort order.
- `mhyp` children: one title `mhod` (type 1) then `mhip` items.
- `mhip` (playlist item): header length; total length; data‑object count; **podcast grouping flag** (0 = normal, 0x100 = podcast group); 2 unknown bytes; group ID; **track ID at offset 24** (the id that must resolve to an existing `mhit`); timestamp; podcast group reference.
- Every `mhip` is followed by a **type‑100 `mhod`** containing a position integer (child of `mhip` from dbversion 0x0d).

### Smart Playlist Rules: `mhod` type 51

The payload after the header is little‑endian until the **"SLst"** marker, after which it switches to **big‑endian**:

- "SLst" (magic)
- unknown (4)
- rule count (4)
- rules operator: `0` = AND (match all), `1` = OR (match any)
- 120 bytes padding
- rules array

Each rule (fixed 56‑byte header, then value payload):

- field type (4)
- action (4) — bitwise; high byte = "string value" (bit 0) and NOT (bit 1); low bits: `0x01` IS, `0x02` CONTAINS, `0x04` STARTS WITH, `0x08` ENDS WITH, `0x10` GREATER THAN, `0x20` GREATER THAN OR EQUAL, `0x40` LESS THAN, `0x80` LESS THAN OR EQUAL, `0x100` IN THE RANGE, `0x200` IN THE LAST, `0x400` IS/IS NOT (video kind)
- 44 bytes padding
- length (4): `0x44` for numeric rules, real byte length for string rules
- value: string (UTF‑16) **or** non‑string (8‑byte from value, 8‑byte from date, 8‑byte from units, 8‑byte to value, 8‑byte to date, 8‑byte to units, 20 bytes unknown)

Special constant for relative rules: `0x2dae2dae2dae2dae` = "now". For "IN THE LAST", from = to = 0x2dae2dae2dae2dae, date = ‑N, units = seconds (e.g. 604800 = week).

## Artwork Database (`mhfd` root, `ArtworkDB`)

Same chunk grammar as iTunesDB. Records:

- `mhsd` type 1 → `mhli` (image list, **count** chunk) → `mhii` (image record).
- `mhii` header length `0x5C`; fields: image ID at offset 16, DB ID at offset 20, then children.
- Children: three `mhod` type 2 entries naming a store (offset 12 = type, offset 28/40 hold format code + size), plus one type‑6 `mhod` with a zero payload.

**Stores** (measured from a 7G; record N = slot N in every store):

| Code | Geometry | Slot size | File |
|------|----------|-----------|------|
| 1055 | 128 × 128, stride 128 | 32 768 | `F1055_1.ithmb` |
| 1060 | 320 × 320, stride 320 | 204 800 | `F1060_1.ithmb` |
| 1061 | 55 × 55, stride 56 | 6 160 | `F1061_1.ithmb` |

- Pixels are **RGB565 little‑endian** (measured by decoding, not assumed).
- The 55×55 store pads every row with one 2‑byte zero column (stride 56 ≠ width 55).
- Artwork DB declares scheme `1` at 0x30 but stores **20 zero bytes** at 0x58 — it is never signed.

### Artwork Track Link

- `mhit` offset 352 = `mhii` image ID in `ArtworkDB` (or the offset/pointer the device uses for the slideshow).
- `mhit` offset 164 has_artwork: `0x01` = has art, `0x02` = none. **Do not claim art (0x01) unless an image was actually written**, or the device shows an empty frame.

## Version / Device Variations

| dbversion | iTunes | mhbd header | mhit header | Notable changes |
|-----------|--------|-------------|-------------|-----------------|
| ≤ 0x0B | 4.7 | `0x68` | `0x9C` | filetype = integer (reversed); no artwork/count flags |
| 0x0C | 4.71/4.8 | `0x68` | `0xF4` | filetype = reversed ASCII string; skip count added |
| 0x0D | 4.9 | `0x68` | `0xF4` | podcast list; has_artwork, podcast flags |
| 0x11‑0x12 | 6.0.2‑6.0.5 | `0x68` | `0x148` | DBID 2 appears; play counts duplicated |
| 0x13 | 7.0 | `0xBC` | `0x184` | language ID, season/episode, media type, gapless data |
| 0x14 | 7.1 | `0xBC` | `0x184` | library persistent ID; album list (`mhla`) added |
| 0x15 | 7.2 | `0xBC` | `0x184` | smart playlist index type 52, letter jump table 53 |
| 0x17+ | 7.3+ | `0xBC` | `0x184` | new playlist list type 5 |
| 0x19 | 7.4 | `0xBC` | `0x184`+ | Nano 3G/Classic: extra fields at 48‑59, hash at 88 |
| 0x75 | 7.5 (7G fixture) | `0xF4` (244) | `0x270` (624) | **This is the layout the fixture uses; it supersedes the wiki's 0x184 mhit table** |

## Common Operation Patterns

1. **Parse** (`db::parse`): read 12‑byte header, decide length vs count from the identifier, recurse with the parent's boundary. Keep all bytes the parser does not understand (header + trailer) verbatim so round‑trip returns the input unchanged.
2. **Read a field**: 4‑byte LE read at offset relative to chunk start; for `mhod` strings decode the length field (offset 28) and UTF‑16LE at offset 40.
3. **Modify**: clone the whole record's header (`chunk.header()`), mutate only understood fields, then recompute sizes by walking children (never hand‑write parent lengths after a resize).
4. **Write**: serialize `chunk.to_bytes()`; all lengths are children‑derived.
5. **Sign/verify**: zero the three regions, set scheme `1` (or `4` for hashAB), HMAC‑SHA1, write the signature, restore the other regions; verify against the stored bytes.
6. **Add a track**: copy an existing `mhit` of the same file type (or retarget its four codec words if copying across types), set id, visible, size, duration, modified/added timestamps, dbid, album id, strings; append to the track list and add an `mhip` to the Library playlist; then write + sign once.
7. **Remove a track**: delete its `mhit`, remove all `mhip`s referencing it, delete empty albums, write once. Do **not** renumber artwork records (record N = slot N in every store).
8. **Detect an iPod**: the presence of `iPod_Control/iTunes/iTunesDB` (not the volume label); the GUID from SysInfo or udev.

## Known Disagreements with Published Documentation

- The wikiPodLinux `mhit` table describes iTunes 7.1 (header `0x184`); the v0x75 fixture has header `0x270` (624) and fields past ~0x100 have moved.
- **Album ID is at offset 288**, not 314 as the wiki states — verified against all 1169 tracks in the fixture.
- The position field of a type‑2 (`Location`) `mhod` must be `1`; a value of `0` makes the track list but not play.
- The string‑encoding flag at offset 32 of `mhod` reads as `1` on UTF‑16 records; it is not an encoding selector.
- `mhod` type‑100 data objects appear in two forms (playlist column definitions, and per‑item position indicators).

## Sources

- wikiPodLinux — ITunesDB file format / basic information / ArtworkDB.
- Linux Journal, "Learning the iTunesDB File Format" (Andrew flarey).
- `libgpod` (hash58 derivation, hashAB tables).
- tplay `tests/fixtures/ipod/iTunesDB` (iPod Classic 7G, 1 915 544 bytes, version `0x75`, 1169 tracks) — the only in‑tree oracle; every measured constant is pinned against it.
- iOpenPod `src/iPodDB/iTunesDB/shared/chunk_defs/` — definition-driven parser with `cf!` macros, CDB framing, `hash72`, and `hashAB` support.
- podkit `packages/ipod-db/src/itunesdb/reader.ts` — typed `MhitRecord`/`MhbdRecord` and `IpodReader` facade with artist/album/genre indexes.
- podkit `docs/formats/generations.md` — generation × support matrix (`access` / `verified`).
- podkit `docs/formats/itunessd-bdhs.md` — iTunesSD (`bdhs`) format for shuffle 3g/4g.
- podkit `device_time.py` — Mac epoch handling with DST fold resolution.
- ipodsync `_hashab_gen.py` / `_hashab_gen_rt.py` / `_hashab_tables.bin` — hashAB generation code.
- nims11/IPod‑Shuffle‑4g — iTunesSD variant notes (early files were big‑endian with 24‑bit ints; 3rd‑gen+ little‑endian 32‑bit).

## What Remains Undocumented

Unknown/undecoded bytes remain at: mhbd offsets 32‑37, 38‑59, 88; mhit offsets 132‑141, 144‑152, 156‑160, 220‑251, 260‑286, 300‑349; the 20‑byte unknown region at mhit 32; smart‑playlist type‑100 columns beyond their count; the exact meaning of `gaplessData` for AAC and the `unk40`/`unk41`/`unk43`/`unk44` cluster. Treat these as opaque when preserving or round‑tripping.

## Implementation Notes for tplay

The `tplay` project implements this format with these key behaviors:

- Round‑trip parsing → serialization preserves all bytes (asserted by `a_round_trip_returns_the_input_unchanged`).
- Hash58 signing/verification using derived GUID key; hashAB support with 57‑byte blob from header.
- Template cloning from existing records (not constructing from scratch) via `push::template_for`.
- Retargeting records across formats (MP3 ↔ M4A) via `push::retarget`.
- Artwork DB uses same chunk grammar under `mhfd` root.
- Database backup stored off‑device (app cache, not beside original).
- Per-track removal with batch capability (one database write for multiple tracks).
- Album ID stored at offset 288 (not 314).
- mhit header size of 624 bytes for v0x75 fixture (supersedes wiki's 0x184).
- mac epoch handling with DST fold resolution (mirrors `device_time.py` approach).
- Artwork stores with RGB565 little‑endian and stride padding for 55×55 format.
- mhod strings use UTF‑16LE with length prefix at offset 28.
- The hashing scheme byte at offset 0x30 (`1` = HASH58, `4` = hashAB).
- iTunesCDB zlib‑framed format flag at offset 0xA8.