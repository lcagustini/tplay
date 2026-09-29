//! Visualizer views — one file per view, each exposing a single `draw` fn and a
//! GLSL body. The registry (`VizView` + `VizView::ALL`) lives in app.rs; the
//! pane dispatches through the table below. Views own per-frame state in egui
//! memory under their own keys (see bars.rs), so a new view needs nothing
//! shared.
//!
//! **Every view is a shader.** There is no CPU-drawing half left, and that is
//! the point rather than an accident of the migration: one code path means one
//! set of failure modes, and no question about which of eleven looks survive a
//! driver with no GL. The cost is that a driver which cannot compile a shader
//! now loses the whole pane rather than four of eleven views — which is a real
//! trade, and the reason [`ShaderView::frags`] is `pub` is so the sweeps in
//! `gui_tests.rs` can hold every shipped shader to the compiler.
//!
//! **A view is a `draw` fn and a shader string, and nothing else.** The table
//! below is both the registry the pane dispatches on and the table every test
//! sweep iterates, so a view the app does not use cannot be checked instead of
//! one it does. It is deliberately *not* keyed on the `VizView` enum — that is
//! what `config.json` stores, and `name()` is the join between them.

use crate::audio::viz::VizBuf;
use crate::gui::theme::Palette;
use eframe::egui;

/// A view drawn by a fragment shader.
///
/// Every view in the set is one of these; the name survives from when the table
/// held only the four that were shaders, because it is what `VizView::name()`
/// and `config.json` join on.
pub struct ShaderView {
    /// The `VizView` variant's dropdown name, so a test can match the table
    /// against `VizView::ALL` without naming a Rust type.
    pub name: &'static str,
    /// The view's file stem under `views/`.
    ///
    /// Here so no caller has to *guess* it from the name. Two tests need to read
    /// a view's source, and the guess — lowercase, strip spaces — was wrong the
    /// moment a name stopped being a filename, which "VU meter" was until the view
    /// it described was deleted. A wrong guess
    /// is a confusing IO error rather than a wrong answer, which is the worst
    /// kind: the test that was meant to catch a missing uniform failed for a
    /// reason that has nothing to do with uniforms. The name is a UI label and
    /// the file is a path; they are allowed to differ, and nothing should have to
    /// reconcile them.
    // The `main` binary compiles this module too, and nothing in the binary reads
    // it — which is the only reason it needs saying out loud.
    #[allow(dead_code)]
    pub file: &'static str,
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

/// Every view, in one table.
///
/// It exists so the properties that are invisible in a screenshot are checkable
/// at all: that every view is reachable from the dispatcher, that no shader
/// hardcodes a colour, and that no view paints anything itself. A test that
/// iterated a hand-written list would be a list that could drift from what
/// ships, which is worse than no test — this is the registry the app itself uses,
/// so a row here is a view that exists and a missing row is a view that does not.
///
/// **The order matches `VizView::ALL`,** because that is the order the dropdown
/// lists them in and the table is what the pane looks the choice up in. A row
/// out of order would draw a different view than the one named in the combo box.
pub const SHADER_VIEWS: &[ShaderView] = &[
    ShaderView {
        name: "Bars",
        file: "bars",
        frags: &[bars::FRAG],
        draw: bars::draw,
    },
    ShaderView {
        name: "Wave",
        file: "wave",
        frags: &[wave::FRAG],
        draw: wave::draw,
    },
    ShaderView {
        name: "Radial",
        file: "radial",
        frags: &[radial::FRAG],
        draw: radial::draw,
    },
    // Both passes, for the same reason `Trails` needs both: the accumulate has to
    // read *whole texels* of the history or it resamples and blurs it, and the
    // present has to *interpolate* the same texture because the quantised target
    // is a little larger than the pane. One program cannot be both, and the wrong
    // one of the two is a picture that looks plausible and is wrong.
    ShaderView {
        name: "Spectrogram",
        file: "spectrogram",
        frags: &[spectrogram::ACCUMULATE, spectrogram::PRESENT],
        draw: spectrogram::draw,
    },
    ShaderView {
        name: "Flame",
        file: "flame",
        frags: &[flame::FRAG],
        draw: flame::draw,
    },
    ShaderView {
        name: "Chladni",
        file: "chladni",
        frags: &[chladni::FRAG],
        draw: chladni::draw,
    },
    ShaderView {
        name: "Nebula",
        file: "nebula",
        frags: &[nebula::FRAG],
        draw: nebula::draw,
    },
    ShaderView {
        name: "Plasma",
        file: "plasma",
        frags: &[plasma::FRAG],
        draw: plasma::draw,
    },
    // Both passes, because both are programs this view runs and both go through
    // the same colour, declaration-order and compile sweeps.
    ShaderView {
        name: "Trails",
        file: "trails",
        frags: &[trails::FRAG, trails::PRESENT],
        draw: trails::draw,
    },
];

pub mod bars;
pub mod chladni;
pub mod flame;
pub mod nebula;
pub mod plasma;
pub mod radial;
pub mod spectrogram;
pub mod trails;
pub mod wave;
