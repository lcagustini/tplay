//! Font fallback: a system font appended to egui's fallback chain renders
//! characters the bundled fonts lack (e.g. U+2010 HYPHEN in "Ne‐Yo") instead
//! of tofu boxes. Headless — no audio device needed.
//!
//! Two egui-0.36 facts shape this file, and both were found by the test failing
//! rather than by reading a changelog:
//!
//! - **The queries run inside `Context::run_ui`,** not a hand-rolled
//!   `begin_pass`/`end_pass` pair: a glyph query loads a font, which produces a
//!   `TexturesDelta`, and 0.36 turns dropping one unapplied into a panic. Every
//!   pass here therefore ends in `drop_without_applying_deltas()`, which is what
//!   egui's own doc example does.
//!
//! - **`Fonts::has_glyph` is not used, because it is wrong on the Monospace
//!   family.** It answers `resolve_face(c) != replacement_face_key`, and the
//!   replacement key is the first face in the chain owning U+FFFD. Monospace
//!   leads with Hack, which owns U+FFFD *and* `'A'`, so `has_glyph('A')` is
//!   false there — every query on that family is a false negative, which egui's
//!   own source flags as a known TODO. `Font::characters()` walks the same chain
//!   and maps each char to the faces that own it, so it is both correct and the
//!   stronger claim: it says *which* font supplies a glyph, not just that one
//!   does. The bundled Monospace chain has since grown U+2010 (Hack supplies it),
//!   which is why the post-install assertion is on Proportional.

use eframe::egui;
use tplay::gui::theme::{self, SYSTEM_FONT_CANDIDATES};

const HYPHEN: char = '\u{2010}';
const MIDDOT: char = '\u{00b7}';

/// The fonts in `family`'s chain that own `c`, or `None` if no face does.
fn owners(ctx: &egui::Context, family: &egui::FontFamily, c: char) -> Option<Vec<String>> {
    ctx.fonts_mut(|f| {
        let mut font = f.fonts.font(family);
        font.characters().get(&c).cloned()
    })
}

#[test]
fn default_fonts_miss_u2010() {
    let ctx = egui::Context::default();
    let mut seen = None;
    let out = ctx.run_ui(egui::RawInput::default(), |_ui| {
        seen = Some((
            owners(&ctx, &egui::FontFamily::Proportional, HYPHEN),
            owners(&ctx, &egui::FontFamily::Proportional, MIDDOT),
        ));
    });
    out.drop_without_applying_deltas();
    let (hyphen, middot) = seen.expect("the closure ran");
    assert_eq!(
        hyphen, None,
        "premise: the bundled proportional fonts still lack U+2010 — if this now has a \
         font, egui fixed the subset and the fallback may no longer be needed for it"
    );
    // The middot separator the panes use is covered, so it is not the culprit.
    assert!(middot.is_some());
}

#[test]
fn fallback_font_renders_u2010() {
    let ctx = egui::Context::default();
    // A first pass, because the font definitions have to be in place before the
    // atlas is built — the same reason the old test re-entered `begin_pass`.
    ctx.run_ui(egui::RawInput::default(), |_ui| {})
        .drop_without_applying_deltas();
    let installed = theme::install_fallback_fonts(&ctx, SYSTEM_FONT_CANDIDATES);
    if !installed {
        // No system font on this machine → nothing to install, skip assertions.
        return;
    }
    let mut out = Vec::new();
    let delta = ctx.run_ui(egui::RawInput::default(), |_ui| {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            // The claim is that the *appended* font supplies the glyph, so a
            // bundle that grew its own U+2010 does not pass this.
            let hyphen_owners = owners(&ctx, &family, HYPHEN).unwrap_or_default();
            if !hyphen_owners.iter().any(|f| f == "system-fallback") {
                out.push(format!(
                    "{family:?}: U+2010 owned by {hyphen_owners:?}, not the fallback"
                ));
            }
            if owners(&ctx, &family, MIDDOT).is_none() {
                out.push(format!("{family:?}: lost U+00B7"));
            }
        }
    });
    delta.drop_without_applying_deltas();
    assert!(out.is_empty(), "{}", out.join("; "));
}

#[test]
fn missing_candidates_are_a_noop() {
    let ctx = egui::Context::default();
    assert!(!theme::install_fallback_fonts(
        &ctx,
        &["/nonexistent/tplay-font.ttf"]
    ));
}
