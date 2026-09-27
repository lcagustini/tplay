//! Font fallback: a system font appended to egui's fallback chain renders
//! characters the bundled fonts lack (e.g. U+2010 HYPHEN in "Ne‐Yo") instead
//! of tofu boxes. Headless — no audio device needed.

use eframe::egui;
use tplay::gui::theme::{self, SYSTEM_FONT_CANDIDATES};

fn has(ctx: &egui::Context, fid: &egui::FontId, c: char) -> bool {
    ctx.fonts(|f| f.has_glyph(fid, c))
}

#[test]
fn default_fonts_miss_u2010() {
    let ctx = egui::Context::default();
    ctx.begin_pass(Default::default());
    // The bundled Ubuntu-Light subset's punctuation coverage starts at U+2013.
    assert!(!has(&ctx, &egui::FontId::proportional(13.0), '\u{2010}'));
    // The middot separator the panes use is covered, so it's not the culprit.
    assert!(has(&ctx, &egui::FontId::proportional(13.0), '\u{00b7}'));
    let _ = ctx.end_pass();
}

#[test]
fn fallback_font_renders_u2010() {
    let ctx = egui::Context::default();
    ctx.begin_pass(Default::default());
    let installed = theme::install_fallback_fonts(&ctx, SYSTEM_FONT_CANDIDATES);
    if installed {
        let _ = ctx.end_pass();
        ctx.begin_pass(Default::default());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let fid = egui::FontId::new(13.0, family.clone());
            assert!(
                has(&ctx, &fid, '\u{2010}'),
                "{family:?} still missing U+2010"
            );
            assert!(has(&ctx, &fid, '\u{00b7}'));
        }
        let _ = ctx.end_pass();
    }
    // No system font on this machine → nothing to install, skip assertions.
}

#[test]
fn missing_candidates_are_a_noop() {
    let ctx = egui::Context::default();
    assert!(!theme::install_fallback_fonts(
        &ctx,
        &["/nonexistent/tplay-font.ttf"]
    ));
}
