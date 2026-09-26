//! Crossfade/gapless tests — the `arm_plan` decision (pure), `fade_gains`
//! curve properties (pure) and the shared xf source builder on a real WAV file
//! (both sinks play the same full-track source; the two-sink playback logic
//! itself is headless-boundary, mirrored only where pure).

mod common;

use tplay::audio::transition::{arm_plan, build_gapless_next, fade_gains, ArmInput, PREROLL_SECS};
use tplay::audio::eq::{EqShared, EqSource, EQ_FREQUENCIES};
use tplay::audio::viz::{TapSource, VizBuf};
use tplay::audio::balance::{BalanceSource, balance_gains};
use rodio::buffer::SamplesBuffer;
use rodio::Source;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

fn dummy_shared() -> (Arc<RwLock<EqShared>>, Arc<RwLock<f32>>, VizBuf) {
    let eq_shared = Arc::new(RwLock::new(EqShared {
        gains: [0.0; 10],
        enabled: true,
    }));
    let balance = Arc::new(RwLock::new(0.0f32));
    let viz = VizBuf::new();
    (eq_shared, balance, viz)
}

/// Constant-power invariant: out² + in² = 1 for every progress point.
#[test]
fn fade_gains_constant_power() {
    for i in 0..=100 {
        let p = i as f32 / 100.0;
        let (out, inc) = fade_gains(p);
        let sum = out * out + inc * inc;
        assert!((sum - 1.0).abs() < 1e-5, "p={p}: out²+in²={sum}");
    }
}

#[test]
fn fade_gains_endpoints() {
    let (o0, i0) = fade_gains(0.0);
    assert!((o0 - 1.0).abs() < 1e-6 && i0.abs() < 1e-6, "p=0 should be (1, 0)");
    let (o1, i1) = fade_gains(1.0);
    assert!(o1.abs() < 1e-6 && (i1 - 1.0).abs() < 1e-6, "p=1 should be (0, 1)");
}

#[test]
fn fade_gains_midpoint_equal_power() {
    let (out, inc) = fade_gains(0.5);
    // cos(π/4) = sin(π/4) ≈ 0.7071
    assert!((out - inc).abs() < 1e-6, "midpoint gains must be equal");
    assert!((out - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn fade_gains_monotonic() {
    let mut prev_out = 1.0f32;
    let mut prev_in = 0.0f32;
    for i in 0..=100 {
        let p = i as f32 / 100.0;
        let (out, inc) = fade_gains(p);
        assert!(out <= prev_out + 1e-6, "out gain must not increase (p={p})");
        assert!(inc >= prev_in - 1e-6, "in gain must not decrease (p={p})");
        prev_out = out;
        prev_in = inc;
    }
}

#[test]
fn fade_gains_clamps_out_of_range() {
    let (o_lo, i_lo) = fade_gains(-3.0);
    assert!((o_lo - 1.0).abs() < 1e-6 && i_lo.abs() < 1e-6);
    let (o_hi, i_hi) = fade_gains(7.0);
    assert!(o_hi.abs() < 1e-6 && (i_hi - 1.0).abs() < 1e-6);
}

/// The xf/gapless builder is a plain buffered full-track source (EQ → Tap →
/// Balance) — decode a real WAV and verify it reports the file's shape.
#[test]
fn build_gapless_next_produces_full_track_source() {
    let dir = common::test_dir("gapless_xf_source");
    let wav = dir.join("a.wav");
    common::write_wav(&wav);
    let (eq_shared, balance, viz) = dummy_shared();

    let mut src = build_gapless_next(&wav, eq_shared, balance, viz)
        .expect("a real wav must build");
    assert_eq!(src.channels(), 2); // BalanceSource upmixes mono → stereo
    assert_eq!(src.sample_rate(), 8000);
    common::assert_duration_approx(src.total_duration(), Duration::from_secs(1), "wav duration");

    // Not truncated, not a mix — a full track flows through (stereo frames).
    let mut count = 0usize;
    while src.next().is_some() && count < 100_000 {
        count += 1;
    }
    assert!(count >= 15_998, "full track must drain ~16000 stereo samples, got {count}");
}

/// A build failure must cost a gap, not the app.
///
/// `build_gapless_next` is called from `advance()` every frame, and it used to
/// `.expect()` on the open and the decode. Any file it could not read therefore
/// aborted the process from inside a per-frame path. The reachable cases are not
/// exotic: a track deleted between the availability check and the open, a
/// truncated or non-audio file, and — the one that actually bit — an `smb://` URI
/// passed where a file path was expected, since there is no such file on disk.
#[test]
fn build_gapless_next_fails_instead_of_panicking() {
    let dir = common::test_dir("gapless_xf_unreadable");

    // Missing file.
    let (eq_shared, balance, viz) = dummy_shared();
    assert!(
        build_gapless_next(&dir.join("nope.wav"), eq_shared, balance, viz).is_none(),
        "a missing file must return None, not panic"
    );

    // Present but not decodable — the open succeeds, the decode fails.
    let junk = dir.join("junk.mp3");
    std::fs::write(&junk, b"this is definitely not audio").unwrap();
    let (eq_shared, balance, viz) = dummy_shared();
    assert!(
        build_gapless_next(&junk, eq_shared, balance, viz).is_none(),
        "an undecodable file must return None, not panic"
    );

    // The `smb://` URI itself: the exact input that crashed the xf arm. It is
    // not a local path, so it cannot be opened. The caller's job is to hand over
    // a resolved local file (`tracks::open`); if one ever gets here,
    // the result is a skipped crossfade, not a dead app.
    let (eq_shared, balance, viz) = dummy_shared();
    assert!(
        build_gapless_next(Path::new("smb://nas/media/song.mp3"), eq_shared, balance, viz).is_none(),
        "an smb:// URI is not a file — must return None, not panic"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// 0 dB EQ + centered balance = bit-transparent passthrough of the decoded file.
#[test]
fn build_gapless_next_identity_when_flags_nominal() {
    let dir = common::test_dir("gapless_xf_identity");
    let wav = dir.join("a.wav");
    common::write_wav(&wav);
    let (eq_shared, balance, viz) = dummy_shared();

    let mut src = build_gapless_next(&wav, eq_shared, balance, viz)
        .expect("a real wav must build");
    let first = src.next().expect("sample");
    // write_wav's first sample is i16 value 0.
    assert!(first.abs() < 1e-4, "first sample should be silence, got {first}");
    // Pull 4000 stereo frames (8000 samples) — the 4000th frame's mono sample
    // is i16 4000, duplicated to L and R by the balance upmix.
    for _ in 0..4000 * 2 {
        let _ = src.next();
    }
    let s4k = src.next().expect("sample");
    assert!((s4k - 4000.0 / 32768.0).abs() < 5e-3, "frame 4000 should be 4000/32768, got {s4k}");
}

/// The wrapper chain preserves stereo (balance outputs stereo from mono) —
/// exercises the same source stack the xf sink uses.
#[test]
fn source_chain_stereo_output() {
    let (eq_shared, balance, viz) = dummy_shared();
    let interleaved: Vec<f32> = (0..100).flat_map(|i| [i as f32 * 0.01, i as f32 * 0.01 + 0.5]).collect();
    let buf = SamplesBuffer::new(2, 44100, interleaved);
    let eq = EqSource::new(buf, eq_shared);
    let tap = TapSource::new(eq, viz);
    let bal = BalanceSource::new(tap, balance);
    assert_eq!(bal.channels(), 2);
}

#[test]
fn balance_gains_curve() {
    // Center = exact passthrough; hard left/right silence the opposite side.
    let (l, r) = balance_gains(0.0);
    assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    let (l, r) = balance_gains(-1.0);
    assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    let (l, r) = balance_gains(1.0);
    assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
}

/// EQ band frequencies still drive both filters and labels — 10 bands, ascending.
#[test]
fn eq_frequencies_ten_ascending() {
    assert_eq!(EQ_FREQUENCIES.len(), 10);
    assert!(EQ_FREQUENCIES.windows(2).all(|w| w[0] < w[1]));
}

// ── arm_plan: the pre-buffer decision ───────────────────────────────────────
//
// These guards used to live inside `TPlayApp::advance`, unreachable from any
// test because `TPlayApp::new` needs an audio device. Every one of them costs a
// *gap* rather than a crash, so none of them was ever caught by a failure —
// they had to be pinned deliberately. A default case that arms, then one case
// per guard.

const NEXT: &str = "/music/next.mp3";

/// An armable crossfade case: 3s window, 2s left of a 3-minute track.
fn armable<'a>(next: Option<(usize, &'a Path)>) -> ArmInput<'a> {
    ArmInput {
        crossfade: true,
        gapless: false,
        crossfade_secs: 3.0,
        total: Some(Duration::from_secs(180)),
        pos: Duration::from_secs(178),
        next,
        next_ready: true,
        next_duration: Some(Duration::from_secs(200)),
    }
}

#[test]
fn arm_fires_in_the_default_case() {
    let armed = arm_plan(&armable(Some((1, Path::new(NEXT))))).expect("2s left of a 3s window");
    assert_eq!(armed.index, 1);
    assert_eq!(armed.track, Path::new(NEXT));
    // The OUTGOING duration — `advance` flips `total_duration` to the incoming
    // track right after, so the fade math needs this saved separately.
    assert_eq!(armed.out_total, Duration::from_secs(180));
}

#[test]
fn arm_needs_a_mode_enabled() {
    let mut i = armable(Some((1, Path::new(NEXT))));
    i.crossfade = false;
    i.gapless = false;
    assert!(arm_plan(&i).is_none(), "no mode on → never arm");
}

#[test]
fn arm_needs_a_known_outgoing_length() {
    let mut i = armable(Some((1, Path::new(NEXT))));
    i.total = None;
    assert!(arm_plan(&i).is_none(), "unknown duration → never arm");

    // The gapless arm has no `total > window` guard to fall back on, so this
    // check is the only thing stopping an unknown-length track from arming on
    // its very first frame.
    i.crossfade = false;
    i.gapless = true;
    assert!(arm_plan(&i).is_none(), "unknown duration, gapless → never arm");
}

#[test]
fn arm_needs_a_candidate() {
    assert!(arm_plan(&armable(None)).is_none(), "no next track → never arm");
}

#[test]
fn crossfade_window_is_inclusive_at_the_boundary() {
    let mut i = armable(Some((1, Path::new(NEXT))));
    // remaining == crossfade_secs exactly → arm (the boundary is inclusive).
    i.pos = Duration::from_secs(180 - 3);
    assert!(arm_plan(&i).is_some(), "remaining == cf must arm");

    // A hair beyond the window → hold off.
    i.pos = Duration::from_millis(180_000 - 3_001);
    assert!(arm_plan(&i).is_none(), "remaining > cf must not arm");
}

#[test]
fn a_track_shorter_than_the_fade_never_arms() {
    let mut i = armable(Some((1, Path::new(NEXT))));
    i.pos = Duration::from_secs(0);

    // The boundary that matters is `total == crossfade_secs`: the track is over
    // exactly as the fade ends, so there is nothing to fade into. Strictly
    // shorter than the window, too.
    i.total = Some(Duration::from_secs(3));
    assert!(
        arm_plan(&i).is_none(),
        "a track exactly as long as the fade window has nothing to overlap — never arm"
    );
    i.total = Some(Duration::from_millis(2_900));
    assert!(arm_plan(&i).is_none(), "a track shorter than the fade window — never arm");

    // One frame longer and it is worth pre-buffering (1s in, so 2.1s left —
    // inside the window, and the track outlasts the window).
    i.total = Some(Duration::from_millis(3_100));
    i.pos = Duration::from_secs(1);
    assert!(arm_plan(&i).is_some(), "a track longer than the window arms");
}

/// Gapless arms on `PREROLL_SECS`, not on the crossfade window. The premise
/// matters: `crossfade_secs` is set to 30s here, so a plan that reached for the
/// wrong constant would arm at 2.5s remaining instead of 2.0s.
#[test]
fn gapless_uses_preroll_not_the_crossfade_window() {
    let mut i = armable(Some((1, Path::new(NEXT))));
    i.crossfade = false;
    i.gapless = true;
    i.crossfade_secs = 30.0;

    i.pos = Duration::from_millis(180_000 - (PREROLL_SECS * 1000.0) as u64);
    assert!(arm_plan(&i).is_some(), "remaining == PREROLL_SECS must arm");

    i.pos = Duration::from_millis(180_000 - 2_500);
    assert!(
        arm_plan(&i).is_none(),
        "2.5s left is outside the {PREROLL_SECS}s gapless preroll"
    );
}

/// The pre-buffer question is "are the bytes on hand right now", not "where did
/// it come from". A local track with no file and an unspooled remote track both
/// read `false` here, and an already-cached remote track reads `true` — which is
/// why gapless/crossfade are a data-availability rule and not a source rule.
#[test]
fn arm_is_skipped_when_the_bytes_are_not_on_hand() {
    let mut i = armable(Some((1, Path::new("smb://nas/media/song.mp3"))));
    i.next_ready = false;
    assert!(
        arm_plan(&i).is_none(),
        "an unspooled remote track cannot be pre-buffered — skip the arm, natural advance takes it"
    );
}

/// A track shorter than the hold/fade window drains muted (gapless) or mid-fade
/// (crossfade) and would be promoted empty — silently skipped. An untagged
/// track is allowed through: unknown length is not a short one.
#[test]
fn a_short_incoming_track_is_skipped_but_an_untagged_one_is_not() {
    let mut i = armable(Some((1, Path::new(NEXT))));

    i.next_duration = Some(Duration::from_secs(2));
    assert!(arm_plan(&i).is_none(), "2s track under a 3s fade window → skip");

    // Exactly the window is not "shorter than" it, so it arms.
    i.next_duration = Some(Duration::from_secs(3));
    assert!(arm_plan(&i).is_some(), "a track exactly as long as the window arms");

    i.next_duration = None;
    assert!(arm_plan(&i).is_some(), "an untagged track has no known length — allow it");
}

/// The hold gapless measures against is the *remaining* time, not the crossfade
/// window: a 2s track still plays when only 2s of preroll is left, where the
/// same track under crossfade (hold = 3s) would be rejected.
#[test]
fn the_gapless_hold_is_the_remaining_time_not_the_crossfade_window() {
    let mut i = armable(Some((1, Path::new(NEXT))));
    i.crossfade = false;
    i.gapless = true;
    i.next_duration = Some(Duration::from_secs(2));
    // remaining is 2.0s (the default fixture), so hold = 2.0s and a 2s track
    // is not shorter than it.
    assert!(arm_plan(&i).is_some(), "hold = remaining = 2s, track is 2s → arm");

    // One frame earlier the hold is smaller still, so the same track is fine.
    i.pos = Duration::from_millis(180_000 - 1_500);
    assert!(arm_plan(&i).is_some(), "hold shrinks with remaining → still arm");
}