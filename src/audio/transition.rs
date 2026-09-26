//! Crossfade fade curve and gapless source builder — pure, headless-testable free functions.

use crate::audio::eq::{EqSource, EqShared};
use crate::audio::viz::{TapSource, VizBuf};
use crate::audio::balance::BalanceSource;
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Try to seek a `Decoder<BufReader<File>>` to `target`; if the inner source reports
/// NotSupported, fall back to `skip_duration` (eager decode).
/// Returns the (possibly wrapped) source positioned at `target`.
pub fn seek_or_skip(mut decoder: Decoder<BufReader<File>>, target: Duration) -> Box<dyn Source<Item = f32> + Send + 'static> {
    // We attempt the fast path first; on NotSupported we drop back to skip_duration.
    match decoder.try_seek(target) {
        Ok(()) => Box::new(decoder),
        Err(rodio::source::SeekError::NotSupported { .. }) => Box::new(decoder.skip_duration(target)),
        Err(e) => {
            // Any other error: best-effort fallback to skip
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

/// Open a file and return a fully-wrapped decoder (EQ → Tap → Balance),
/// *buffered* so decode happens off the audio thread.
/// Used for the crossfade incoming track and gapless next track.
///
/// Returns `None` if the file cannot be opened or decoded. This is called from
/// `advance()` every frame, and a crossfade is a nicety: failing to pre-buffer
/// must cost a gap, not the whole app. It previously `.expect()`ed, which meant
/// an unreadable file aborted the process from inside a per-frame path.
pub fn build_gapless_next(
    path: &std::path::Path,
    eq_shared: Arc<RwLock<EqShared>>,
    balance: Arc<RwLock<f32>>,
    viz: VizBuf,
) -> Option<Box<dyn Source<Item = f32> + Send + 'static>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("tplay: gapless/crossfade: open {}: {e}", path.display());
            return None;
        }
    };
    let decoder = match Decoder::try_from(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("tplay: gapless/crossfade: decode {}: {e}", path.display());
            return None;
        }
    };
    let eq_source = EqSource::new(decoder, eq_shared);
    let tap_source = TapSource::new(eq_source, viz);
    Some(Box::new(BalanceSource::new(tap_source, balance).buffered()))
}