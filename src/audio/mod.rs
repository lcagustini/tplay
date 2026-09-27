//! Audio file handling — duration probing and visualization.

pub mod balance;
pub mod eq;
pub mod transition;
pub mod viz;

use std::fs::File;
use std::path::Path;
use std::time::Duration;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// ── Duration probe ─────────────────────────────────────────────────────────────

pub fn probe_duration(path: &Path) -> Option<Duration> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed.format.default_track()?;
    let tb = track.codec_params.time_base?;
    let n_frames = track.codec_params.n_frames?;
    let t = tb.calc_time(n_frames);
    Some(Duration::from_secs_f64(t.seconds as f64 + t.frac))
}
