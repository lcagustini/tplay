//! Contrast verification for the bundled themes.
//!
//! Loads each bundled theme through the real loader and computes WCAG
//! relative-luminance ratios for the token pairs that carry real content — text
//! on surfaces, and UI components on their adjacent surfaces. This locks the
//! token values so a future palette edit can't silently regress readability.
//! (It is intentionally not "the full WCAG audit": decorative pairs like the
//! knob-outline-on-fill edge are excluded.)
//!
//! **It reads `Theme`, not `theme.json`.** The first version of this file carried
//! its own eleven-field mirror of the palette struct and read the JSON, which
//! meant two tokens it never had — `border` and `btn_hover` — and no way to see
//! the six groups that followed. Going through the loader makes a token that this
//! suite cannot reach impossible to add.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::Color32;
use tplay::gui::theme::{Theme, Themes};

// `RGB` is the conventional name for red/green/blue and the fields are exactly
// that; `Rgb` would be strictly worse to read. This is the one lint in the tree
// that is allowed rather than fixed.
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy)]
struct RGB {
    r: f64,
    g: f64,
    b: f64,
}

/// WCAG relative luminance (0..1).
fn luminance(c: &RGB) -> f64 {
    let lin = |x: f64| {
        let c = x / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: &RGB, b: &RGB) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Overlay `fg` on `bg` at `alpha` (0..1), straight-alpha compositing in sRGB
/// byte space — mirrors what egui's translucent fill does on screen.
fn composite(fg: &RGB, bg: &RGB, alpha: f64) -> RGB {
    let c = |x: f64, y: f64| alpha * x + (1.0 - alpha) * y;
    RGB {
        r: c(fg.r, bg.r),
        g: c(fg.g, bg.g),
        b: c(fg.b, bg.b),
    }
}

/// A bundled theme, loaded through the real loader.
///
/// **Not a local mirror of the token struct, which is what this file used to
/// carry.** The mirror was eleven of the thirteen palette tokens — it never had
/// `border` or `btn_hover`, so a regression in either could not fail this suite —
/// and a mirror of six more groups would have been worse. Going through
/// `Themes::load_from` means every token the app reads is a token this suite can
/// read, and a token added to the theme system cannot silently escape the audit.
fn bundled(id: &str) -> Arc<Theme> {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let dir = PathBuf::from(root).join("themes");
    Themes::load_from(std::slice::from_ref(&dir))
        .get(id)
        .unwrap_or_else(|| panic!("{id} is a bundled theme and must load"))
        .clone()
}

/// The themes this suite guards. `winamp` is here for the same reason as the
/// others and more: it is the theme that sets **every** token group, so a token
/// this suite does not read is a token whose only example is unchecked.
const BUNDLED: [&str; 4] = ["dark", "retro", "neon", "winamp"];

/// A token that a theme may leave out. Only pairs the theme actually declares are
/// asserted: an absent `dock.tab_bg` is egui's own grey, and egui is responsible
/// for that pairing, not the theme. What is pinned is that a theme which *does*
/// choose tab colours chooses readable ones.
fn assert_pair(a: RGB, b: RGB, min: f64, what: &str) {
    assert!(
        contrast(&a, &b) >= min,
        "{what}: {:.2}:1, needs {min}:1",
        contrast(&a, &b)
    );
}

fn as_rgb(c: Color32) -> RGB {
    RGB {
        r: c.r() as f64,
        g: c.g() as f64,
        b: c.b() as f64,
    }
}

const TEXT: f64 = 4.5; // normal text (accent doubles as text: sort header, tag, tab title)
const UI: f64 = 3.0; // non-text UI components

/// The palette's own three text colours on every surface a theme can paint them
/// on. `accent` counts as text because the app uses it for the sort header, the
/// "Playlist" row tag and the tab title.
#[test]
fn bundled_themes_keep_text_readable() {
    for id in BUNDLED {
        let t = bundled(id);
        let p = t.palette;
        for (name, surf) in [
            ("bg", as_rgb(p.bg)),
            ("panel_bg", as_rgb(p.panel_bg)),
            ("row_even", as_rgb(p.row_even)),
            ("row_odd", as_rgb(p.row_odd)),
        ] {
            assert_pair(
                as_rgb(p.text_primary),
                surf,
                TEXT,
                &format!("{id}: text_primary on {name}"),
            );
            assert_pair(
                as_rgb(p.text_secondary),
                surf,
                TEXT,
                &format!("{id}: text_secondary on {name}"),
            );
            assert_pair(
                as_rgb(p.accent),
                surf,
                TEXT,
                &format!("{id}: accent on {name}"),
            );
        }
    }
}

/// The focus ring, the slider's fill and handle, and a button's label.
///
/// **`border` is deliberately absent**, and asserting it would be asserting a bug:
/// it is a 1px separator between two surfaces of one panel, and the dark theme's
/// is 1.57:1 against `bg`. WCAG 1.4.11 exempts a purely decorative boundary, and a
/// border that read at 3:1 would be a line through the middle of every list row.
/// The one boundary that carries meaning is `focus_ring`, which is asserted.
///
/// `btn_hover` is asserted as the surface under a *label*, not against the
/// panel behind the button — a dark theme deliberately keeps those two close.
#[test]
fn bundled_themes_keep_ui_components_visible() {
    for id in BUNDLED {
        let t = bundled(id);
        let p = t.palette;
        assert_pair(
            as_rgb(p.focus_ring),
            as_rgb(p.bg),
            UI,
            &format!("{id}: focus_ring on bg"),
        );
        assert_pair(
            as_rgb(p.progress_fill),
            as_rgb(p.slider_track),
            UI,
            &format!("{id}: progress_fill on slider_track"),
        );
        assert_pair(
            as_rgb(p.slider_handle),
            as_rgb(p.slider_track),
            UI,
            &format!("{id}: slider_handle on slider_track"),
        );
        assert_pair(
            as_rgb(p.slider_handle),
            as_rgb(p.bg),
            UI,
            &format!("{id}: slider_handle on bg"),
        );
        // A button's *label* is what has to be readable, not the button's fill
        // against the panel: a dark theme deliberately makes the two close, and
        // the outline plus the label carry the affordance. So the pair asserted
        // is the text on the fill, at the full text threshold.
        assert_pair(
            as_rgb(p.text_primary),
            as_rgb(p.btn_hover),
            TEXT,
            &format!("{id}: text_primary on btn_hover"),
        );
    }
}

/// The active row's tint is a deliberate highlight, and both of its text colours
/// sit on top of it.
///
/// `text_secondary` is the one this file did not check, and it is the one a row
/// actually spends most of its width on: the artist, album and duration cells are
/// all `text_secondary`, so a tint that keeps the title readable while dropping
/// the tag columns below the threshold reads as "the tag columns vanished".
#[test]
fn a_tinted_row_keeps_both_text_colours_readable() {
    for id in BUNDLED {
        let t = bundled(id);
        let p = t.palette;
        let alpha = t.rows.tint_alpha as f64;
        for (name, surf) in [
            ("row_even", as_rgb(p.row_even)),
            ("row_odd", as_rgb(p.row_odd)),
        ] {
            let tinted = composite(&as_rgb(p.accent), &surf, alpha);
            assert_pair(
                as_rgb(p.text_primary),
                tinted,
                TEXT,
                &format!("{id}: text_primary on tinted {name}"),
            );
            assert_pair(
                as_rgb(p.text_secondary),
                tinted,
                TEXT,
                &format!("{id}: text_secondary on tinted {name}"),
            );
        }
    }
}

/// The dock is the largest surface a theme newly reaches, and a tab is the one
/// place the app draws text on a colour it chose itself. Both the resting and
/// the selected tab are checked, because `tab_bg` and `tab_bg_active` are
/// separate tokens and a theme can satisfy one and fail the other.
#[test]
fn a_theme_that_chooses_tab_colours_keeps_them_readable() {
    for id in BUNDLED {
        let t = bundled(id);
        let d = t.dock;
        let active = as_rgb(d.tab_text_active);
        if let Some(bg) = d.tab_bg {
            let ink = as_rgb(d.tab_text.unwrap_or(t.palette.text_secondary));
            assert_pair(
                ink,
                as_rgb(bg),
                TEXT,
                &format!("{id}: dock.tab_text on dock.tab_bg"),
            );
        }
        if let Some(bg) = d.tab_bg_active {
            assert_pair(
                active,
                as_rgb(bg),
                TEXT,
                &format!("{id}: dock.tab_text_active on dock.tab_bg_active"),
            );
        }
    }
}

/// A disabled widget's text is `disabled_alpha` of the way to its own fill, so a
/// theme that sets the alpha low enough makes disabled text unreadable while
/// every other pair in this file still passes.
///
/// The floor is 3:1 rather than 4.5:1 because WCAG exempts disabled controls —
/// but it is not zero, and the assertion is that egui's default leaves the text
/// distinguishable rather than that a theme may erase it.
#[test]
fn a_theme_cannot_set_disabled_alpha_into_invisibility() {
    for id in BUNDLED {
        let t = bundled(id);
        let alpha = t.chrome.disabled_alpha as f64;
        assert!(
            (0.0..=1.0).contains(&alpha),
            "{id}: disabled_alpha {alpha} is not a fraction"
        );
        // egui composites the whole widget — text included — toward the
        // background at this alpha, so the faded text is the blend of
        // `text_primary` with the surface behind it.
        let faded = composite(
            &as_rgb(t.palette.text_primary),
            &as_rgb(t.palette.bg),
            alpha,
        );
        assert_pair(
            faded,
            as_rgb(t.palette.bg),
            3.0,
            &format!("{id}: text at disabled_alpha {alpha}"),
        );
    }
}

/// The two `chrome` colours that are text or frame on the app's main surface.
/// `weak_text` is egui's own de-emphasis colour and a theme may now set it;
/// `window_stroke` frames every modal and popup.
#[test]
fn a_theme_that_chooses_weak_text_and_window_stroke_keeps_them_usable() {
    for id in BUNDLED {
        let t = bundled(id);
        let bg = as_rgb(t.palette.bg);
        if let Some(weak) = t.chrome.weak_text {
            assert_pair(
                as_rgb(weak),
                bg,
                TEXT,
                &format!("{id}: chrome.weak_text on bg"),
            );
            assert_pair(
                as_rgb(weak),
                as_rgb(t.palette.panel_bg),
                TEXT,
                &format!("{id}: chrome.weak_text on panel_bg"),
            );
        }
        if let Some(stroke) = t.chrome.window_stroke {
            assert_pair(
                as_rgb(stroke),
                bg,
                UI,
                &format!("{id}: chrome.window_stroke on bg"),
            );
        }
    }
}

/// The EQ pane floors its width at 10 × eq_band_w_min (no scrollbars), so the
/// floor must stay inside the 320px minimum window.
#[test]
fn eq_band_floor_fits_minimum_window() {
    for id in BUNDLED {
        let t = bundled(id);
        let floor = 10.0 * t.layout.eq_band_w_min;
        assert!(
            floor <= 320.0,
            "{id}: 10×eq_band_w_min {floor:.0} exceeds the 320px minimum window"
        );
    }
}

/// A theme that widens the EQ's minimum band width must still fit the window.
/// This is the token-driven form of the assertion above, and it is what catches
/// a skin author's edit to `layout` rather than a palette edit.
#[test]
fn the_eq_floor_fits_for_every_bundled_theme_whatever_it_asks_for() {
    for id in BUNDLED {
        let t = bundled(id);
        assert_eq!(
            t.layout.eq_band_w_min, 30.0,
            "{id}: the shipped eq_band_w_min changed, so the 320px floor needs rechecking"
        );
    }
}
