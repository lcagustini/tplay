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
pub fn build_gapless_next(
    path: std::path::PathBuf,
    eq_shared: Arc<RwLock<EqShared>>,
    balance: Arc<RwLock<f32>>,
    viz: VizBuf,
) -> impl Source<Item = f32> + Send + 'static {
    let file = File::open(path).expect("gapless/crossfade: file open failed");
    let decoder = Decoder::try_from(file).expect("gapless/crossfade: decode failed");
    let eq_source = EqSource::new(decoder, eq_shared);
    let tap_source = TapSource::new(eq_source, viz);
    BalanceSource::new(tap_source, balance).buffered()
}