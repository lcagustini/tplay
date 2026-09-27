//! Real encoded audio, one file per format, from `tests/fixtures/`.
//!
//! Every other fixture in this suite is hand-built by `common.rs`, because
//! nothing in the dependency tree can *encode* audio. These eight are committed
//! real files instead, and they cover the three formats the hand-built set never
//! reached at all: **FLAC had no audio** (`write_minimal_flac` is headers only)
//! and **OGG and M4A had no fixture whatsoever**.
//!
//! What a synthetic tone cannot tell you, and these can: whether a decoder
//! accepts what a real encoder wrote, where each container actually keeps its
//! tag block, and whether a format's declared duration survives a round trip.
//!
//! The files are one 12.27 s / 44.1 kHz recording in eight containers, so a
//! duration assertion means the same thing in each. Provenance and licence:
//! `tests/fixtures/README.md`.
//!
//! **Three facts here were measured, not assumed, and two of them contradict
//! what this file's first draft and `AGENTS.md` claimed.** They are written up
//! where they are asserted, and the comments say which way to fix what.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tplay::library::{is_audio, read_info, title_or_stem};
use tplay::rodio::{Decoder, Source};

/// The five formats in `library::AUDIO_EXTENSIONS`, which is the list the Library
/// pane filters on. `aac`, `opus` and `aiff` are deliberately absent — and the
/// last two absences are each justified by a measurement below, not by taste.
const SUPPORTED: [&str; 5] = ["lena.flac", "lena.ogg", "lena.m4a", "lena.mp3", "lena.wav"];

/// The files that are not listed, with the reason each one is missing. A
/// negative list is worth as much as a positive one: it is what stops the next
/// person adding a format that looks decodable and is not.
const UNLISTED: [&str; 3] = ["lena.aac", "lena.aiff", "lena.opus"];

/// Roughly the length of every fixture, and a window loose enough that a lossy
/// codec's padding is not a failure. MP3 adds a decoder delay and AAC a priming
/// pad, so "the same recording" is a band, never an equality.
const EXPECTED: Duration = Duration::from_millis(12_270);

/// The committed fixtures. `CARGO_MANIFEST_DIR` rather than a relative path, so a
/// test's working directory is not load-bearing — cargo does not promise one.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Every fixture is a readable file of real size. A `None` from any reader below
/// would otherwise be ambiguous between "the fixture is missing" and "the format
/// is broken", so the premise is stated once, here.
#[test]
fn the_fixtures_are_all_present() {
    for name in SUPPORTED.iter().copied().chain(UNLISTED) {
        let p = fixture(name);
        assert!(
            p.is_file(),
            "missing fixture {} — see tests/fixtures/README.md",
            p.display()
        );
        assert!(
            p.metadata().unwrap().len() > 50_000,
            "premise: {name} is a real recording, not a stub"
        );
    }
}

/// The claim half the suite rests on: the Library lists every supported fixture
/// and none of the three unlisted ones.
#[test]
fn only_the_supported_fixtures_are_listed_as_audio() {
    for name in SUPPORTED {
        assert!(
            is_audio(&fixture(name)),
            "premise: {name} is in AUDIO_EXTENSIONS"
        );
    }
    for name in UNLISTED {
        assert!(
            !is_audio(&fixture(name)),
            "premise: {name} is deliberately not in AUDIO_EXTENSIONS"
        );
    }
}

/// Every supported format opens and reports the recording's real length.
#[test]
fn every_supported_format_decodes_and_reports_its_own_duration() {
    for name in SUPPORTED {
        let secs = tplay::tracks::probe(&fixture(name))
            .unwrap_or_else(|| panic!("{name}: probe found no duration"))
            .as_secs_f64();
        assert!(
            (secs - EXPECTED.as_secs_f64()).abs() < 0.5,
            "{name}: probed {secs:.2}s, expected the fixture's ~{:.2}s",
            EXPECTED.as_secs_f64()
        );
    }
}

/// One number, two independent readers — the decoder's frame count and the tag
/// reader's. A mismatch means one of them is estimating rather than reading.
///
/// This is the assertion that earns the fixtures their keep: `lena.aac` passes
/// every decode check above and still fails here, which is the whole reason it is
/// not in `AUDIO_EXTENSIONS` (see the tripwire).
#[test]
fn the_decoder_and_the_tag_reader_agree_on_every_duration() {
    for name in SUPPORTED {
        let path = fixture(name);
        let probed = tplay::tracks::probe(&path).expect("probe");
        let tagged = read_info(&path)
            .and_then(|i| i.duration)
            .unwrap_or_else(|| panic!("{name}: lofty reported no duration"));
        assert!(
            tagged.abs_diff(probed) < Duration::from_millis(250),
            "{name}: lofty says {tagged:?}, the decoder says {probed:?}"
        );
    }
}

/// The four that carry a real tag block, read through the app's own reader.
///
/// This is the part a hand-built fixture cannot reach, because the four put their
/// tags in **four different places**: FLAC in a native `VORBIS_COMMENT` metadata
/// block, OGG in a Vorbis comment packet, MP3 in ID3v2, and WAV in a `LIST`/`INFO`
/// chunk. One call, four container-specific locations — and the WAV case is the
/// one that is easy to assume is untagged and is not.
#[test]
fn the_four_tagged_formats_return_their_real_tags() {
    for name in ["lena.flac", "lena.ogg", "lena.mp3", "lena.wav"] {
        let info =
            read_info(&fixture(name)).unwrap_or_else(|| panic!("{name}: lofty could not read it"));
        assert_eq!(info.artist, "Lena Stolze", "{name}: artist");
        assert_eq!(info.album, "Das schreckliche Mädchen", "{name}: album");
        assert_eq!(info.title.trim(), "Oh lad le", "{name}: title");
    }
}

/// The one supported format that is **untagged**, and the fallback that covers
/// it.
///
/// `lena.m4a` has no `ilst` atom at all — only a legacy `udta/name` title that
/// lofty does not read. That is not a broken file; it is an ordinary one, and a
/// library is full of them. The contract is that a row with no readable title
/// shows its **filename stem** rather than nothing, which is `title_or_stem`'s
/// job, so that is what is asserted.
#[test]
fn the_untagged_m4a_falls_back_to_the_filename_stem() {
    let path = fixture("lena.m4a");
    let info = read_info(&path).expect("the file itself still parses");
    assert_eq!(info.title, "", "premise: no ilst atom lofty can read");
    assert_eq!(info.artist, "", "no artist");
    assert_eq!(info.album, "", "no album");
    assert_eq!(title_or_stem(&path, Some(&info)), "lena");
    // Untagged is not unreadable: the duration still comes through, and this is
    // also the only stereo fixture in the set.
    assert!(info.duration.is_some(), "duration survives");
    let decoder = Decoder::try_from(std::fs::File::open(&path).unwrap()).expect("decodes");
    assert_eq!(decoder.channels(), 2, "premise: the only stereo fixture");
}

/// The one fixture that does not decode at all.
///
/// Opus has no codec crate in symphonia 0.5.5 — not in `all-codecs`, only the
/// Ogg container — so this is a genuine failure, `Unrecognized format`. The only
/// true tripwire of the three unlisted files, and the one that will break first
/// on a dependency bump, because rodio 0.22 / symphonia 0.6 is also the upgrade
/// that would matter for AIFF.
#[test]
fn symphonia_has_no_opus_codec() {
    let path = fixture("lena.opus");
    let err = Decoder::try_from(std::fs::File::open(&path).unwrap())
        .err()
        .expect("symphonia 0.5.5 cannot decode Opus");
    assert_eq!(err.to_string(), "Unrecognized format");
    // The rate is still readable from the container head, which is how the
    // "nothing here may assume 44.1 kHz" note stays honest: this one is 48 kHz.
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        bytes.windows(2).any(|w| w == [0x80, 0xBB]), // 48000 as a little-endian u16
        "premise: the Opus head names a 48 kHz rate"
    );
    // And the tag reader is unaffected — a track with no codec can still be
    // listed, which is why this is a *playback* limitation, not a listing one.
    assert!(read_info(&path).is_some(), "listing needs no codec");
}

/// ## Two corrections to the written record
///
/// Both of these were asserted in an earlier draft of this file, and in
/// `AGENTS.md`, and **measurement contradicts both.** They are kept as tests
/// because a corrected claim still needs a guard, and because the fix belongs
/// with the measurement rather than in a comment nobody re-reads.
///
/// **`lena.aac` decodes.** `AGENTS.md` says symphonia 0.5.5 has "no ADTS
/// demuxer", so raw AAC was presumed unplayable. It is not: symphonia parses the
/// ADTS stream, and the file plays.
///
/// What is actually wrong with it is subtler and is the real reason to leave it
/// out of `AUDIO_EXTENSIONS`: **ADTS carries no duration**, so lofty *estimates*
/// one from the bitrate and gets it 0.5 s long. The decoder is right (12.330 s)
/// and the tag reader is wrong (12.808 s), which is the reverse of the usual
/// arrangement — and the Library's Duration column reads lofty, so the row would
/// display a wrong length. So the exclusion stands, for a different reason.
#[test]
fn raw_aac_decodes_but_its_lofty_duration_is_an_estimate() {
    let path = fixture("lena.aac");
    let decoder = Decoder::try_from(std::fs::File::open(&path).unwrap())
        .expect("symphonia 0.5.5 DOES parse ADTS — the documented claim was wrong");
    assert_eq!(decoder.sample_rate(), 44_100);
    assert_eq!(decoder.channels(), 1);

    let decoded = decoder
        .total_duration()
        .expect("the decoder knows its length");
    let estimated = read_info(&path)
        .and_then(|i| i.duration)
        .expect("lofty reports a duration too");
    assert!(
        (decoded.as_secs_f64() - 12.33).abs() < 0.2,
        "the decoder counts frames: {:.3}s",
        decoded.as_secs_f64()
    );
    assert!(
        estimated.abs_diff(decoded) > Duration::from_millis(400),
        "premise: lofty's estimate is off by more than the {DECODE_TOLERANCE:?} every \
         other format passes. If this now agrees, ADTS gained a duration field and \
         \"aac\" can be added to AUDIO_EXTENSIONS and this test deleted."
    );
}

/// The tolerance every other format in this suite passes, named so the assertion
/// above can say what it is comparing against instead of repeating the number.
const DECODE_TOLERANCE: Duration = Duration::from_millis(250);

/// **The AIFF sample-rate bug is narrower than documented.** `AGENTS.md` says
/// symphonia "reads 8000 Hz as 3904 **and 44100 Hz as 11332**", and an earlier
/// draft of this file asserted the second half on the theory that a real 44.1 kHz
/// file would show the bug more clearly. It does not: a correctly encoded
/// 44.1 kHz AIFF decodes at **44100**.
///
/// So the bug is real and `library_tests.rs` proves it against an 8 kHz file, but
/// it is a property of that value rather than of the container — which is why
/// "add `aif`/`aiff` back to `AUDIO_EXTENSIONS`" is a decision that needs
/// checking per rate, not a blanket undo. This test pins the 44.1 kHz half so
/// nobody re-derives the wrong half.
#[test]
fn a_correctly_encoded_44100_aiff_is_not_rate_mangled() {
    let path = fixture("lena.aiff");
    let decoder =
        Decoder::try_from(std::fs::File::open(&path).unwrap()).expect("rodio decodes AIFF");
    assert_eq!(
        decoder.sample_rate(),
        44_100,
        "symphonia reads a correct 44.1 kHz AIFF rate correctly. If this ever changes, \
         the extended-float bug widened; re-check the claim in AGENTS.md before assuming \
         the 8 kHz case is the only one affected."
    );
    let dur = decoder.total_duration().expect("duration").as_secs_f64();
    assert!(
        (dur - EXPECTED.as_secs_f64()).abs() < 0.1,
        "and so its duration is right too — got {dur:.3}s, expected ~{:.3}s",
        EXPECTED.as_secs_f64()
    );
    // No text chunks at all, so lofty reads a blank title — the opposite of the
    // WAV, which has a LIST/INFO chunk. The same extension, two tag schemes.
    let info = read_info(&path).expect("the file parses");
    assert_eq!(info.title, "", "premise: this AIFF carries no NAME chunk");
    assert_eq!(info.artist, "");
}
