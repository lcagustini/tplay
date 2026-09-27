# Audio fixtures

Eight real encoded files, one per format, used by `tests/audio_fixtures.rs`.

## Why these are files and not generated

`tests/common.rs` hand-builds every other fixture — `write_wav`, `write_aiff`,
`write_tagged_mp3`, `write_minimal_flac` — because nothing in the dependency tree
can *encode* audio. That is still true of the crates, but it is no longer a
reason to ship a synthetic file when a real one is available: a 1 s tone tells you
a decoder opened the file, and nothing about whether the tag block is where that
format actually puts it, whether a container's length field survives a round
trip, or what a 12 s track does to the crossfade window.

So these are one real recording, encoded into eight containers by the upstream
project, and committed. They are what closes the three gaps the hand-built set
left open: **FLAC had no audio at all** (`write_minimal_flac` is headers only),
and **OGG and M4A had no fixture whatsoever**.

## Provenance

| | |
|---|---|
| Source | [audiojs/audio-lena](https://github.com/audiojs/audio-lena) — "the audio equivalent of the Lena test image" |
| Original | [freesound #246148](https://www.freesound.org/people/heshamwhite/sounds/246148/), *heshamwhite* |
| Licence | **CC0 1.0** — public domain dedication, no attribution required, no restrictions |
| Retrieved | 2026-09-27 from `raw.githubusercontent.com/audiojs/audio-lena/master/` |

CC0 was the deciding criterion: it can be committed to this repository without a
licence file, without a NOTICE, and without the fixtures becoming the most
restrictive thing in the tree.

## The set

All eight are the same 12.27 s / 44.1 kHz recording, so a duration assertion means
the same thing in each container. `lena.opus` is the exception at **48 kHz** —
any cross-format position or seek assertion must not assume 44.1.

| file | codec | ch | rate | tags | why it is here |
|---|---|---|---|---|---|
| `lena.flac` | FLAC | 1 | 44.1k | ✅ | `AUDIO_EXTENSIONS`; the only real FLAC audio in the repo |
| `lena.ogg` | Vorbis | 1 | 44.1k | ✅ | `AUDIO_EXTENSIONS`; no OGG fixture existed |
| `lena.m4a` | AAC | 2 | 44.1k | ❌ | `AUDIO_EXTENSIONS`; no M4A fixture existed, and the only **stereo** file |
| `lena.mp3` | MP3 | 1 | 44.1k | ✅ | real audio where `write_tagged_mp3` has silent frames |
| `lena.wav` | PCM s16le | 1 | 44.1k | ✅ | 44.1 kHz baseline; `write_wav` is 8 kHz mono |
| `lena.aiff` | PCM s16be | 1 | 44.1k | ❌ | pins that the AIFF rate bug is **not** container-wide |
| `lena.aac` | AAC ADTS | 1 | 44.1k | ❌ | decodes, but lofty's duration is a 0.5 s estimate |
| `lena.opus` | Opus | 1 | **48k** | ❌ | the one file symphonia cannot open at all |

## The tags are in four different places, which is the point

`lena.flac` keeps them in a native `VORBIS_COMMENT` metadata block, `lena.ogg` in
a Vorbis comment packet, `lena.mp3` in ID3v2, and `lena.wav` in a `LIST`/`INFO`
chunk. One reader call covers all four container-specific locations — which is
exactly the coverage a hand-built fixture cannot offer.

`lena.aiff` is the instructive contrast: the same extension family as the WAV
but **no text chunks at all**, so lofty reads a blank title from it. A WAV and an
AIFF that are both "uncompressed PCM with no tags" still differ.

## The one untagged *supported* file

`lena.m4a` has no `ilst` atom — only a legacy `udta/name` title, which lofty does
not read. That is an ordinary real-world shape, not a broken file, and the suite
asserts the consequence: a row with no readable title shows its **filename stem**,
which is what the Library pane does for any untagged file.

## Two claims that measurement contradicted

Both were believed when these files were chosen, and both are now pinned by the
opposite test:

- **Raw `.aac` decodes.** The belief was that symphonia 0.5.5 has no ADTS
  demuxer. It does — `lena.aac` opens and plays. The real disqualifier is
  different: ADTS has no length field, so lofty estimates the duration from the
  bitrate and reports 12.808 s where the decoder counts 12.330 s, and the
  Duration column reads lofty.
- **The AIFF rate bug is not container-wide.** It was believed to turn 44100 Hz
  into 11332 as well as 8000 Hz into 3904. It does not: `lena.aiff` decodes at a
  correct 44100 Hz with a correct duration. The bug is a property of the 8 kHz
  value, so re-enabling AIFF is a per-rate check.

## Why the upstream set is mostly absent

The upstream repository ships 21 files. The other 13 are not here because
symphonia 0.5.5 has no demuxer or codec for them — `caf` ×3, `raw`, `webm` ×2,
`wma`, `mp2`, `m4r`, and the 24-/32-bit, A-law and µ-law AIFF variants — and
committing 10 MB that this app provably cannot open would buy bytes, not
coverage. Add one only alongside a test that needs it.

## Do not regenerate these

`ffmpeg` is not a build dependency and the suite must stay hermetic. If a fixture
needs replacing, take a replacement from a CC0 source, record it here, and pin
its checksum rather than encoding it from a tone.
