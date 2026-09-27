//! tplay library — public API for testing.

pub mod app;
pub mod audio;
pub mod config;
pub mod gui;
pub mod library;
pub mod network;
pub mod tracks;

/// Re-exported so a test can build the audio `Mixer` that `TPlayApp::new`
/// takes, without a dev-dependency that could drift from the real one.
pub use rodio;
