//! Crossfade fade curve, the arm decision, and the gapless source builder —
//! pure, headless-testable free functions.
//!
//! The arm used to be a 50-line block inside `TPlayApp::advance`, putting it out
//! of reach of every test — and its failure modes are silent (a skipped arm is
//! a gap, a wrong arm is a skipped track), so nothing would have caught a
//! regression in it. `arm_plan` is that block moved out whole: it reads a struct
//! of values and returns what to apply, so every guard below is reachable from
//! a plain test.

use crate::audio::balance::BalanceSource;
use crate::audio::eq::{EqShared, EqSource};
use crate::audio::viz::{TapSource, VizBuf};
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// How far before track end (seconds) to arm gapless/crossfade next track.
pub const PREROLL_SECS: f32 = 2.0;

/// Everything the arm decision reads, grouped so a test can build one literal
/// and vary a single field per case. `advance` fills this in; nothing here
/// reads app state, so the decision is reachable headless.
///
/// The sink-shape preconditions (`!is_paused`, one source queued, a current
/// track) stay in `advance` — `Sink` facts with no data equivalent, and the
/// cheap outer gate.
pub struct ArmInput<'a> {
    pub crossfade: bool,
    pub gapless: bool,
    /// Crossfade window in seconds; also the hold a short incoming track is
    /// measured against.
    pub crossfade_secs: f32,
    /// The outgoing track's total duration. `None` = unknown, never arm.
    pub total: Option<Duration>,
    /// How far into the outgoing track playback already is.
    pub pos: Duration,
    /// The candidate next track — playlist index plus its id — already chosen by
    /// the caller. Passed in rather than picked here because choosing *mutates*
    /// shuffle's `played` history: the caller peeks, then commits only if the
    /// arm lands (see `advance` and `peek_next_index`).
    pub next: Option<(usize, &'a Path)>,
    /// `tracks::is_ready(next)` — are the incoming bytes on hand right now?
    /// One `stat` either way, and this block runs every frame: a local track is
    /// `is_file()` on its own id, a remote one resolves the spool-cache path
    /// first, which is a second lookup.
    pub next_ready: bool,
    /// The candidate's tagged duration, if `tag_cache` has one.
    pub next_duration: Option<Duration>,
}

/// What a successful arm should apply. `out_total` is the *outgoing* track's
/// duration, carried even though it equals `ArmInput::total`: the caller has to
/// capture it before `total_duration` flips to the incoming track, and the fade
/// math needs both.
pub struct Armed {
    pub index: usize,
    pub track: PathBuf,
    pub out_total: Duration,
}

/// Decide whether to pre-buffer the next track, and for which one.
///
/// Every guard is load-bearing and each costs a *gap*, never a crash, so all of
/// them return `None` rather than aborting:
/// - no mode on, or the outgoing track's length unknown → never arm
/// - outside the window (crossfade within `crossfade_secs` of the end, and only
///   for a track longer than the window itself; gapless within `PREROLL_SECS`)
/// - no candidate next track
/// - the incoming bytes aren't on hand — a local file that isn't there, or a
///   remote track not yet spooled. It arrives later through the pending-spool
///   path, and an *already cached* remote track pre-buffers like any local one,
///   so this is a data-availability rule, not a source rule
/// - the incoming track is shorter than the hold/fade window: it would drain
///   muted (gapless) or mid-fade (crossfade) and be promoted empty, i.e.
///   silently skipped. An untagged track is allowed through — unknown length is
///   not a short one
pub fn arm_plan(input: &ArmInput) -> Option<Armed> {
    if !input.crossfade && !input.gapless {
        return None;
    }
    let total = input.total?;
    let remaining_secs = total.saturating_sub(input.pos).as_secs_f32();
    let cf = Duration::from_secs_f32(input.crossfade_secs);

    let in_window = if input.crossfade {
        remaining_secs <= input.crossfade_secs && total > cf
    } else {
        remaining_secs <= PREROLL_SECS
    };
    if !in_window {
        return None;
    }

    let (index, track) = input.next?;
    if !input.next_ready {
        return None;
    }
    let hold = if input.crossfade {
        cf
    } else {
        Duration::from_secs_f32(remaining_secs)
    };
    if input.next_duration.is_some_and(|d| d < hold) {
        return None;
    }
    Some(Armed {
        index,
        track: track.to_path_buf(),
        out_total: total,
    })
}

/// Seek a `Decoder<BufReader<File>>` to `target`, falling back to `skip_duration`
/// (eager decode) if the source reports NotSupported. Returns the (possibly
/// wrapped) source positioned at `target`.
pub fn seek_or_skip(
    mut decoder: Decoder<BufReader<File>>,
    target: Duration,
) -> Box<dyn Source<Item = f32> + Send + 'static> {
    match decoder.try_seek(target) {
        Ok(()) => Box::new(decoder),
        Err(rodio::source::SeekError::NotSupported { .. }) => {
            Box::new(decoder.skip_duration(target))
        }
        Err(e) => {
            // Any other error: best-effort fallback to skip.
            eprintln!("tplay: seek error {e:?}, falling back to skip");
            Box::new(decoder.skip_duration(target))
        }
    }
}

/// Equal-power crossfade gains (cos² + sin² = 1).
/// p ∈ [0, 1] is fade progress. Returns (out_gain, in_gain).
pub fn fade_gains(p: f32) -> (f32, f32) {
    let p = p.clamp(0.0, 1.0);
    let angle = p * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// Sink gains for a live gapless/crossfade overlap, from how much of the
/// **outgoing** track is left. The second half of the arm, pure for the same
/// reason `arm_plan` is: the curve was tested, but *when* it is called with
/// which `p` was not — and that is the half deciding whether a track is audible
/// or dropped.
///
/// Progress runs 0 (a full `crossfade_secs` still to go) → 1 (the outgoing track
/// has ended), linearly in remaining time — not a wall clock, so pause- and
/// seek-safe. Duration-probe drift can push `remaining` past the window, making
/// `p` overshoot [0, 1]; `fade_gains` clamps there, so this does not repeat it.
/// (`f32::FRAC_PI_2` also rounds up, so `fade_gains(1.0)` is `(-4.37e-8, 1.0)`
/// rather than exactly `(0.0, 1.0)` — the outgoing sink is set to that on the
/// last frame before the swap, i.e. −4e-8 of full volume, inaudible.)
///
/// Gapless has no fade to run: the incoming track is held at zero and the swap
/// is instant, so this returns `(1.0, 0.0)` — full out, silence in — for any
/// `remaining`. A pair rather than an `Option` keeps the caller's per-frame block
/// to three lines with no branch.
///
/// A zero-length crossfade is the ☰ slider's floor and reaches this as
/// `0/0` = NaN, which `clamp` propagates into both sinks' volume. With no window
/// there is nothing to fade through, so `p` is pinned to 1 — the swap point.
pub fn xf_gains(remaining: Duration, crossfade: bool, crossfade_secs: f32) -> (f32, f32) {
    if !crossfade {
        return (1.0, 0.0);
    }
    let p = if crossfade_secs > 0.0 {
        1.0 - remaining.as_secs_f32() / crossfade_secs
    } else {
        1.0
    };
    fade_gains(p)
}

/// A track's file as a fully-wrapped decoder (EQ → Tap → Balance), *buffered* so
/// decode happens off the audio thread. Used for the crossfade incoming track
/// and the gapless next track.
///
/// `track` is a track **id**, not a file path — for a remote track that is an
/// `smb://` URI, and no file on disk has that name. Resolution goes through
/// `tracks::open`, so this cannot be handed a URI and panic on it.
///
/// `None` if the track has no bytes on hand or they will not decode. Called from
/// `advance()` every frame, and a crossfade is a nicety: failing to pre-buffer
/// must cost a gap, not the app. This used to `.expect()`, so an unreadable file
/// aborted the process from inside a per-frame path.
pub fn build_gapless_next(
    track: &std::path::Path,
    eq_shared: Arc<RwLock<EqShared>>,
    balance: Arc<RwLock<f32>>,
    viz: VizBuf,
) -> Option<Box<dyn Source<Item = f32> + Send + 'static>> {
    let file = crate::tracks::open(track)?;
    let decoder = match Decoder::try_from(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("tplay: gapless/crossfade: decode {}: {e}", track.display());
            return None;
        }
    };
    let eq_source = EqSource::new(decoder, eq_shared);
    let tap_source = TapSource::new(eq_source, viz);
    Some(Box::new(BalanceSource::new(tap_source, balance).buffered()))
}
