//! Timestamped event logging on stderr, through the `log` facade.
//!
//! **The facade is the point, not the formatting.** Nothing in this crate
//! installs a logger today, and `log` records are dropped when nobody does — so
//! wgpu's own diagnostics, which is where "the driver refused this pipeline"
//! lives, went nowhere. A shader that failed to build and a shader that worked
//! both render as the pane background, so the log is the only instrument that
//! tells them apart. `src/logging.rs` is the whole fix and it is 30 lines.
//!
//! **The timestamp is elapsed time, not a wall clock.** A wall clock needs
//! civil-time conversion that the standard library does not offer, and the
//! question a GUI session asks is "what happened four seconds ago" rather than
//! "at what time". Records that come from this crate print no target, because a
//! driver message needs its origin and `tplay::app` on every line is noise.
//!
//! stderr only: a launcher-launched app writes into the systemd journal, so
//! `journalctl -f tplay` reads this. Nothing is written to a file, which means a
//! place where stderr goes nowhere also loses the log.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

/// What the sink accepts. `Info` is the floor for an event worth a line; the
/// failures this app already reports are `Warn` or `Error`.
pub const LEVEL_FILTER: LevelFilter = LevelFilter::Info;

static START: OnceLock<Instant> = OnceLock::new();

struct Stderr;

impl Log for Stderr {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        // The level filter decides this, in one place. Answering `true` here
        // cannot widen it: `log` consults `max_level` before it calls back.
        true
    }

    fn log(&self, record: &Record) {
        // `let _ =` because a log call must not be able to end the process. A
        // closed stderr is not worth a panic, and `eprintln!` panics on it.
        let _ = writeln!(
            std::io::stderr().lock(),
            "{}",
            line(
                elapsed(),
                record.level(),
                record.target(),
                &record.args().to_string()
            )
        );
    }

    fn flush(&self) {}
}

/// Seconds since `init`, so a record carries its own order and spacing.
fn elapsed() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// One record as one line: `+   12.345s WARN  no file for /x.mp3`.
///
/// Pure, so the format is testable without a stderr to read. A target under this
/// crate is dropped, because the prefix already says who spoke.
pub fn line(elapsed: f64, level: Level, target: &str, msg: &str) -> String {
    // `tplay` or `tplay::something`, not merely a crate whose name starts with
    // those letters.
    let ours = target == "tplay" || target.starts_with("tplay::");
    if ours {
        format!("+{elapsed:>9.3}s {:<5} {msg}", level.as_str())
    } else {
        format!("+{elapsed:>9.3}s {:<5} {target}: {msg}", level.as_str())
    }
}

/// Install the sink. Call this before anything that can log.
///
/// `log::set_logger` panics if a logger is already installed, and this is the
/// one call site in the process.
pub fn init() {
    log::set_logger(&Stderr).expect("the logger is installed exactly once, in main");
    log::set_max_level(LEVEL_FILTER);
    // Seeded here rather than at the first record, so the first line reads
    // near zero instead of near the time the first record happened to arrive.
    START.get_or_init(Instant::now);
}
