//! Contrast verification for the bundled themes.
//!
//! Reads `themes/<id>/theme.json` directly and computes WCAG relative-luminance
//! ratios for the token pairs that carry real content — text on surfaces, and
//! UI components (focus ring, slider fill/handle) on their adjacent surfaces.
//! This locks the token values so a future palette edit can't silently regress
//! readability. (It is intentionally not "the full WCAG audit": decorative
//! pairs like border-on-bg and the knob-outline-on-fill edge are excluded.)

use std::path::PathBuf;

// `RGB` is the conventional name for red/green/blue and the fields are exactly
// that; `Rgb` would be strictly worse to read. This is the one lint in the tree
// that is allowed rather than fixed.
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone)]
struct RGB {
    r: f64,
    g: f64,
    b: f64,
}

fn parse_hex(s: &str) -> RGB {
    let s = s.trim_start_matches('#');
    assert_eq!(s.len(), 6, "expected 6-digit hex, got {s}");
    let v = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap() as f64;
    RGB { r: v(0), g: v(2), b: v(4) }
}

/// WCAG relative luminance (0..1).
fn luminance(c: &RGB) -> f64 {
    let lin = |x: f64| {
        let c = x / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
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
    RGB { r: c(fg.r, bg.r), g: c(fg.g, bg.g), b: c(fg.b, bg.b) }
}

#[derive(Clone)]
struct Palette {
    bg: RGB,
    panel_bg: RGB,
    text_primary: RGB,
    text_secondary: RGB,
    accent: RGB,
    progress_fill: RGB,
    slider_track: RGB,
    slider_handle: RGB,
    row_even: RGB,
    row_odd: RGB,
    focus_ring: RGB,
}

impl Palette {
    fn from_json(file: &std::path::Path) -> Palette {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let p = &v["palette"];
        let get = |k: &str| parse_hex(p[k].as_str().unwrap());
        Palette {
            bg: get("bg"),
            panel_bg: get("panel_bg"),
            text_primary: get("text_primary"),
            text_secondary: get("text_secondary"),
            accent: get("accent"),
            progress_fill: get("progress_fill"),
            slider_track: get("slider_track"),
            slider_handle: get("slider_handle"),
            row_even: get("row_even"),
            row_odd: get("row_odd"),
            focus_ring: get("focus_ring"),
        }
    }

    fn layout_alpha(&self, file: &std::path::Path, key: &str, default: f64) -> f64 {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        v["layout"][key].as_f64().unwrap_or(default)
    }
}

fn theme_file(id: &str) -> PathBuf {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    PathBuf::from(root).join("themes").join(id).join("theme.json")
}

const TEXT: f64 = 4.5; // normal text (accent doubles as text: sort header, tag, tab title)
const UI: f64 = 3.0; // non-text UI components

#[test]
fn bundled_themes_keep_text_readable() {
    for id in ["dark", "retro", "neon"] {
        let p = Palette::from_json(&theme_file(id));
        for (name, surf) in [("bg", &p.bg), ("panel_bg", &p.panel_bg), ("row_even", &p.row_even), ("row_odd", &p.row_odd)] {
            assert!(contrast(&p.text_primary, surf) >= TEXT, "{id}: text_primary on {name}");
            assert!(contrast(&p.text_secondary, surf) >= TEXT, "{id}: text_secondary on {name}");
            assert!(contrast(&p.accent, surf) >= TEXT, "{id}: accent on {name}");
        }
    }
}

#[test]
fn bundled_themes_keep_ui_components_visible() {
    for id in ["dark", "retro", "neon"] {
        let p = Palette::from_json(&theme_file(id));
        assert!(contrast(&p.focus_ring, &p.bg) >= UI, "{id}: focus_ring on bg");
        assert!(contrast(&p.progress_fill, &p.slider_track) >= UI, "{id}: progress_fill on slider_track");
        assert!(contrast(&p.slider_handle, &p.slider_track) >= UI, "{id}: slider_handle on slider_track");
    }
}

#[test]
fn active_row_tint_keeps_title_readable() {
    // The full-row accent tint is a deliberate highlight; its one hard
    // constraint is that the row's primary text stays readable on top of it.
    for id in ["dark", "retro", "neon"] {
        let p = Palette::from_json(&theme_file(id));
        let alpha = p.layout_alpha(&theme_file(id), "row_tint_alpha", 0.10);
        for (name, surf) in [("row_even", &p.row_even), ("row_odd", &p.row_odd)] {
            let tinted = composite(&p.accent, surf, alpha);
            assert!(contrast(&p.text_primary, &tinted) >= TEXT, "{id}: text_primary on tinted {name}");
        }
    }
}

#[test]
fn eq_band_floor_fits_minimum_window() {
    // The EQ pane floors its width at 10 × eq_band_w_min (no scrollbars), so
    // the floor must stay inside the 320px minimum window width.
    for id in ["dark", "retro", "neon"] {
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(theme_file(id)).unwrap(),
        )
        .unwrap();
        let w = v["layout"]["eq_band_w_min"].as_f64().unwrap_or(30.0);
        assert!(10.0 * w <= 320.0, "{id}: 10×eq_band_w_min {:.0} exceeds 320px minimum window", 10.0 * w);
    }
}