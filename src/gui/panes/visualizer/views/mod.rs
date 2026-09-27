//! Visualizer views — one file per view, each exposing a single `draw` fn.
//! The registry (`VizView` + `VizView::ALL`) lives in app.rs; the pane
//! dispatches on it. Views own per-frame state in egui memory under their own
//! keys (see bars.rs), so a new view needs nothing shared.

pub mod bars;
pub mod chladni;
pub mod flame;
pub mod radial;
pub mod spectrogram;
pub mod vu;
pub mod wave;
