//! Visualizer views — one file per view, each exposing a single `draw` fn.
//! The registry (`VizView` + `VizView::ALL`) lives in app.rs; the pane
//! dispatches on it. Views own per-frame state in egui memory under their own
//! keys (see bars.rs), so a new view needs nothing shared.
//!
//! **A view is a `draw` fn and a shader string, and nothing else** — the
//! signature below is identical whether a view paints with egui or with a
//! fragment shader, so the dispatcher cannot tell the two apart and adding a
//! shader view is the same six steps as adding a CPU one. [`SHADER_VIEWS`] is
//! the registry of the shader half: a new shader view is one row here, and the
//! two test sweeps in `gui_tests.rs` run off the table rather than growing a
//! test body per view. It is deliberately *not* what the pane dispatches on —
//! `VizView` is, because that is what `config.json` stores.

use crate::audio::viz::VizBuf;
use crate::gui::theme::Palette;
use eframe::egui;

/// A view drawn by a fragment shader rather than by egui's painter.
pub struct ShaderView {
    /// The `VizView` variant's dropdown name, so a test can match the table
    /// against `VizView::ALL` without naming a Rust type.
    pub name: &'static str,
    /// Every GLSL body this view ships. Not complete shaders: the harness
    /// prefixes the version, the fragment output, and the uniforms each body
    /// references.
    ///
    /// A **slice**, not a single string, because a view may run more than one
    /// program — `Trails` accumulates and then presents — and one program per row
    /// would leave the second untested. Read by the sweeps in `gui_tests.rs`
    /// rather than by the pane, since a view passes its own sources to the
    /// harness and never reads them back out of the table. It is here so the
    /// shaders and the `draw` fn that ships them are one row, and a test cannot
    /// check a shader the app does not use.
    // The `main` binary compiles this module too, and nothing in the binary reads
    // it — which is the only reason it needs saying out loud.
    #[allow(dead_code)]
    pub frags: &'static [&'static str],
    pub draw: fn(&egui::Painter, egui::Rect, &VizBuf, &Palette),
}

/// Every shader view, in one table.
///
/// It exists so the two properties that are invisible in a screenshot are
/// checkable at all: that each shader view is reachable from the dispatcher, and
/// that no shader hardcodes a colour. A test that iterated a hand-written list
/// would be a list that could drift from what ships, which is worse than no
/// test — the registry the app itself uses cannot.
pub const SHADER_VIEWS: &[ShaderView] = &[
    ShaderView {
        name: "Nebula",
        frags: &[nebula::FRAG],
        draw: nebula::draw,
    },
    ShaderView {
        name: "Plasma",
        frags: &[plasma::FRAG],
        draw: plasma::draw,
    },
    ShaderView {
        name: "Chladni 3D",
        frags: &[chladni3d::FRAG],
        draw: chladni3d::draw,
    },
    // Both passes, because both are programs this view runs and both go through
    // the same colour and declaration-order sweeps.
    ShaderView {
        name: "Trails",
        frags: &[trails::ACCUMULATE_BODY, trails::PRESENT],
        draw: trails::draw,
    },
];

pub mod bars;
pub mod chladni;
pub mod chladni3d;
pub mod flame;
pub mod nebula;
pub mod plasma;
pub mod radial;
pub mod spectrogram;
pub mod trails;
pub mod vu;
pub mod wave;
