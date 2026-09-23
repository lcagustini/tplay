//! Theme system — data-driven Winamp-style token palettes.
//!
//! Themes live in `themes/<id>/theme.json` folders, found in three places
//! (highest priority first):
//!   1. `~/.config/tplay/themes/`   — user themes (win on id clash)
//!   2. `<exe_dir>/themes/`         — shipped with the app
//!   3. `./themes/`                 — dev convenience (`cargo run` from repo root)
//!
//! Each `theme.json` carries the 14-token palette, `base` (dark/light), and an
//! optional `metadata_font`. Icons are PNGs under `<theme>/icons/`, per theme,
//! falling back to the default theme's PNGs, then to unicode glyphs. Selection
//! persists to `~/.config/tplay/theme.json` (`{"theme":"<id>"}`).
//!
//! `palette()` exposes the tokens for custom painting (playlist rows, metadata
//! text); `apply()` maps them onto egui `Visuals` each frame so a mid-session
//! switch lands instantly.

use eframe::egui::{self, Color32, FontFamily, Stroke, TextureHandle, Vec2};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default theme id, shipped with the app and used as the icon fallback.
pub const DEFAULT_THEME_ID: &str = "dark";

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Base {
    Dark,
    Light,
}

/// One value per CSS custom property in the reference spec.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Palette {
    // --bg
    pub bg: Color32,
    // --panel-bg
    pub panel_bg: Color32,
    // --text-primary
    pub text_primary: Color32,
    // --text-secondary
    pub text_secondary: Color32,
    // --accent
    pub accent: Color32,
    // --border
    pub border: Color32,
    // --progress-fill
    pub progress_fill: Color32,
    // --slider-track
    pub slider_track: Color32,
    // --slider-handle
    pub slider_handle: Color32,
    // --row-even
    pub row_even: Color32,
    // --row-odd
    pub row_odd: Color32,
    // --btn-hover
    pub btn_hover: Color32,
    // --focus-ring
    pub focus_ring: Color32,
}

/// Per-theme UI layout/sizing tokens (all optional; sensible defaults in code).
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    /// EQ slider minimum height (px). Below this, sliders become unusable.
    pub eq_slider_min_h: f32,
    /// EQ slider maximum height (px).
    pub eq_slider_max_h: f32,
    /// Minimum per-band width (px). Below `10 * eq_band_w_min` the band row
    /// clips (no scrollbars), so the dock floors the pane's width there.
    pub eq_band_w_min: f32,
    /// Gap between header and the first band row (px).
    pub eq_header_gap: f32,
    /// Gap between each band's slider and its frequency label (px).
    pub eq_band_gap: f32,
    /// Type scale: row metadata / caption label size (px).
    pub text_meta: f32,
    /// Type scale: time / duration label size (px).
    pub text_time: f32,
    /// Active-row highlight: alpha (0..1) of the full-row accent tint.
    pub row_tint_alpha: f32,
}

impl Layout {
    pub fn with_defaults(self) -> Self {
        Layout {
            eq_slider_min_h: if self.eq_slider_min_h > 0.0 { self.eq_slider_min_h } else { 60.0 },
            eq_slider_max_h: if self.eq_slider_max_h > 0.0 { self.eq_slider_max_h } else { 220.0 },
            eq_band_w_min: if self.eq_band_w_min > 0.0 { self.eq_band_w_min } else { 30.0 },
            eq_header_gap: if self.eq_header_gap > 0.0 { self.eq_header_gap } else { 6.0 },
            eq_band_gap: if self.eq_band_gap > 0.0 { self.eq_band_gap } else { 2.0 },
            text_meta: if self.text_meta > 0.0 { self.text_meta } else { 12.0 },
            text_time: if self.text_time > 0.0 { self.text_time } else { 13.0 },
            row_tint_alpha: if self.row_tint_alpha > 0.0 { self.row_tint_alpha } else { 0.10 },
        }
    }
}

/// One theme loaded from disk (or the built-in fallback).
#[derive(Debug)]
pub struct Theme {
    /// Stable id used in theme.json selection + as the skins directory name.
    pub id: String,
    /// Human-friendly name shown in the Theme dropdown.
    pub name: String,
    pub base: Base,
    /// Retro renders times/metadata in monospace (the pixel-era look).
    pub metadata_font: FontFamily,
    pub palette: Palette,
    /// Per-theme UI layout/sizing tokens.
    pub layout: Layout,
    /// `<theme_dir>/icons`, when the folder exists.
    pub icons_dir: Option<PathBuf>,
}

impl Theme {
    fn from_json(text: &str, dir: &Path) -> Option<Theme> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let id = v.get("id").and_then(|x| x.as_str())
            .or_else(|| dir.file_name().and_then(|n| n.to_str()))
            .unwrap_or(DEFAULT_THEME_ID)
            .to_string();
        let name = v.get("name").and_then(|x| x.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| id.clone());
        let base = match v.get("base").and_then(|x| x.as_str()) {
            Some("light") => Base::Light,
            _ => Base::Dark,
        };
        let metadata_font = match v.get("metadata_font").and_then(|x| x.as_str()) {
            Some("monospace") => FontFamily::Monospace,
            _ => FontFamily::Proportional,
        };
        let palette = Palette::from_json(v.get("palette")?)?;
        let icons_dir = dir.join("icons");
        let icons_dir = icons_dir.is_dir().then_some(icons_dir);
        let layout = v.get("layout").map_or(Layout::default(), |l| Layout {
            eq_slider_min_h: l.get("eq_slider_min_h").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            eq_slider_max_h: l.get("eq_slider_max_h").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            eq_band_w_min: l.get("eq_band_w_min").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            eq_header_gap: l.get("eq_header_gap").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            eq_band_gap: l.get("eq_band_gap").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            text_meta: l.get("text_meta").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            text_time: l.get("text_time").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
            row_tint_alpha: l.get("row_tint_alpha").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(0.0),
        }).with_defaults();

        Some(Theme { id, name, base, layout, metadata_font, palette, icons_dir })
    }

    /// Hardcoded dark palette used only when no themes/ folder exists anywhere
    /// (broken install) — the app never runs with zero themes.
    fn builtin_fallback() -> Theme {
        // Use include_str so the theme.json is the single source of truth.
        let json = include_str!("../../themes/dark/theme.json");
        Theme::from_json(json, Path::new("")).unwrap()
    }
}

impl Palette {
    fn from_json(v: &serde_json::Value) -> Option<Palette> {
        let tok = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(|s| Color32::from_hex(s).ok());
        Some(Palette {
            bg: tok("bg")?,
            panel_bg: tok("panel_bg")?,
            text_primary: tok("text_primary")?,
            text_secondary: tok("text_secondary")?,
            accent: tok("accent")?,
            border: tok("border")?,
            progress_fill: tok("progress_fill")?,
            slider_track: tok("slider_track")?,
            slider_handle: tok("slider_handle")?,
            row_even: tok("row_even")?,
            row_odd: tok("row_odd")?,
            btn_hover: tok("btn_hover")?,
            focus_ring: tok("focus_ring")?,
        })
    }
}

fn theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config) = dirs::config_dir() {
        dirs.push(config.join("tplay").join("themes"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("themes"));
        }
    }
    dirs.push(PathBuf::from("themes"));
    dirs
}

/// The set of loadable themes. Loaded once at startup; user themes in the
/// config dir override identically-named bundled ones.
pub struct Themes {
    list: Vec<Arc<Theme>>,
}

impl Themes {
    pub fn load() -> Themes {
        Self::load_from(&theme_dirs())
    }

    /// Scan the given directories (in priority order) and merge by theme id.
    /// Load all themes found across the given dirs (first dir wins on id
    /// clash) plus the hardcoded dark fallback.
    pub fn load_from(dirs: &[PathBuf]) -> Themes {
        let mut list: Vec<Arc<Theme>> = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let theme_dir = entry.path();
                if !theme_dir.is_dir() {
                    continue;
                }
                let Some(theme) = std::fs::read_to_string(theme_dir.join("theme.json"))
                    .ok()
                    .and_then(|json| Theme::from_json(&json, &theme_dir))
                else {
                    eprintln!("tplay: skipping invalid theme in {}", theme_dir.display());
                    continue;
                };
                // First dir scanned wins; later (lower-priority) dirs can't override.
                if !list.iter().any(|t| t.id == theme.id) {
                    list.push(Arc::new(theme));
                }
            }
        }
        if !list.iter().any(|t| t.id == DEFAULT_THEME_ID) {
            list.insert(0, Arc::new(Theme::builtin_fallback()));
        }
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Themes { list }
    }

    pub fn list(&self) -> &[Arc<Theme>] {
        &self.list
    }

    pub fn get(&self, id: &str) -> Option<&Arc<Theme>> {
        self.list.iter().find(|t| t.id == id)
    }

    /// The default theme (fallback icon source when a theme has no PNG of
    /// its own). `load()` guarantees a non-empty list.
    pub fn default(&self) -> &Arc<Theme> {
        self.get(DEFAULT_THEME_ID)
            .or_else(|| self.list.first())
            .expect("Themes::load always produces at least one theme")
    }

    /// Resolve an icon PNG for a theme: the theme's own file → the default
    /// theme's file → `None` (callers fall back to a unicode glyph).
    pub fn icon_path(&self, theme: &Theme, icon: Icon) -> Option<PathBuf> {
        let own = theme.icons_dir.as_ref().map(|d| d.join(icon.file_name()))
            .filter(|p| p.exists());
        own.or_else(|| {
            if theme.id == self.default().id {
                return None;
            }
            self.default().icons_dir.as_ref()
                .map(|d| d.join(icon.file_name()))
                .filter(|p| p.exists())
        })
    }
}

/// Decode a theme's icons (own PNG → default theme's PNG) into GPU textures,
/// one per `Icon::ALL` slot. Missing/undecodable icons are `None` → the panes
/// render a unicode glyph instead. Synchronous at startup/theme-switch: no
/// async loader, no pending-placeholder, buttons get exact sizes.
pub fn load_icons(
    ctx: &egui::Context,
    themes: &Themes,
    theme: &Arc<Theme>,
) -> Vec<Option<TextureHandle>> {
    Icon::ALL
        .iter()
        .map(|&icon| {
            let bytes = std::fs::read(themes.icon_path(theme, icon)?).ok()?;
            let rgba = image::load_from_memory(&bytes).ok()?.to_rgba8();
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                &rgba,
            );
            Some(ctx.load_texture(
                format!("tplay-{}-{}", theme.id, icon.file_name()),
                color,
                egui::TextureOptions::LINEAR,
            ))
        })
        .collect()
}

/// Pane icons. Each maps to a `<theme>/icons/<name>.png` and a fallback glyph.
/// Fallback glyphs must exist in the bundled egui font stack (Ubuntu-Light /
/// NotoEmoji / emoji-icon-font) or they render as tofu boxes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    /// App logo — menu button in the top panel (per-theme PNG, fallback glyph).
    Logo,
    Play,
    Pause,
    Stop,
    Prev,
    Next,
    Shuffle,
    Repeat,
    Volume,
    Remove,
    /// Library sort direction (accent-colored, per theme).
    SortAsc,
    SortDesc,
    /// Library favorite toggle (star_on = accent, star_off = secondary).
    StarOn,
    StarOff,
    /// Library folder row glyph.
    Folder,
    /// Window chrome: minimize / maximize (text_primary, per theme).
    Minimize,
    Maximize,
    /// Album Cover pane placeholder shown when a track has no art.
    NoCover,
}

impl Icon {
    pub const ALL: [Icon; 18] = [
        Icon::Logo,
        Icon::Play,
        Icon::Pause,
        Icon::Stop,
        Icon::Prev,
        Icon::Next,
        Icon::Shuffle,
        Icon::Repeat,
        Icon::Volume,
        Icon::Remove,
        Icon::SortAsc,
        Icon::SortDesc,
        Icon::StarOn,
        Icon::StarOff,
        Icon::Folder,
        Icon::Minimize,
        Icon::Maximize,
        Icon::NoCover,
    ];

    const DATA: [(&'static str, &'static str); 18] = [
        ("logo.png", "☰"),
        ("play.png", "▶"),
        ("pause.png", "⏸"),
        ("stop.png", "⏹"),
        ("prev.png", "⏮"),
        ("next.png", "⏭"),
        ("shuffle.png", "🔀"),
        ("repeat.png", "🔁"),
        ("volume.png", "🔊"),
        ("remove.png", "×"),
        ("sort_asc.png", "⏶"),
        ("sort_desc.png", "⏷"),
        ("star_on.png", "★"),
        ("star_off.png", "☆"),
        ("folder.png", "📁"),
        ("minimize.png", "🗕"),
        ("maximize.png", "🗖"),
        ("nocover.png", "🎵"),
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    fn file_name(self) -> &'static str {
        Self::DATA[self.index()].0
    }

    /// Last-resort glyph when neither this theme nor the default ships the PNG.
    pub fn glyph(self) -> &'static str {
        Self::DATA[self.index()].1
    }
}

/// Transport/action button wired to a theme icon texture, falling back to a
/// glyph when the icon failed to load. `selected` paints egui's selected
/// visuals (theme `--progress-fill`/`--focus-ring`) for mode toggles.
pub fn icon_button(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    icon: Icon,
    size: f32,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    let button = match tex {
        Some(h) => egui::Button::image(
            egui::Image::new(h).fit_to_exact_size(Vec2::splat(size)),
        ),
        None => egui::Button::new(icon.glyph()),
    };
    ui.add_enabled(enabled, button.selected(selected))
}

/// Standalone icon (e.g. the volume speaker beside its slider).
pub fn icon(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    icon: Icon,
    size: f32,
) -> egui::Response {
    match tex {
        Some(h) => ui.add(egui::Image::new(h).fit_to_exact_size(Vec2::splat(size))),
        None => ui.label(icon.glyph()),
    }
}

/// Map the token palette onto egui's `Visuals`. (egui has no 1:1 token
/// slots, so nearby fields stand in; the ones that can't be expressed here —
/// row banding, text-secondary, disabled text — are used directly from
/// `theme.palette` by the panes.)
///
///   --bg              panel_fill / window_fill / extreme_bg_color
///   --panel-bg        faint_bg_color / noninteractive.bg_fill
///   --text-primary    override_text_color
///   --accent          hyperlink_color / active.bg_fill
///   --progress-fill   selection.bg_fill (the slider's trailing fill)
///   --slider-track    inactive.bg_fill (the slider rail)
///   --slider-handle   inactive.fg_stroke (the slider knob outline)
///   --btn-hover       hovered.bg_fill
///   --border          noninteractive.bg_stroke
///   --focus-ring      selection.stroke (egui focus rectangles)
pub fn apply(ctx: &egui::Context, theme: &Theme) {
    let p = theme.palette;
    let mut v = match theme.base {
        Base::Light => egui::Visuals::light(),
        Base::Dark => egui::Visuals::dark(),
    };
    v.panel_fill = p.bg;
    v.window_fill = p.bg;
    v.extreme_bg_color = p.bg;
    v.faint_bg_color = p.panel_bg;
    v.code_bg_color = p.panel_bg;
    v.override_text_color = Some(p.text_primary);
    v.hyperlink_color = p.accent;
    v.selection = egui::style::Selection {
        bg_fill: p.progress_fill,
        stroke: Stroke::new(1.0_f32, p.focus_ring),
    };

    v.widgets.noninteractive.bg_fill = p.panel_bg;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text_secondary);
    v.widgets.inactive.weak_bg_fill = p.btn_hover;
    v.widgets.inactive.bg_fill = p.slider_track;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.slider_handle);
    v.widgets.hovered.weak_bg_fill = p.btn_hover;
    v.widgets.hovered.bg_fill = p.btn_hover;
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, p.text_primary);
    v.widgets.active.weak_bg_fill = p.accent;
    v.widgets.active.bg_fill = p.accent;
    v.widgets.active.fg_stroke = Stroke::new(1.5_f32, p.text_primary);

    ctx.set_visuals(v);

    // Kill widget color cross-fades so a mid-session theme switch lands on the
    // very next frame (this app's aesthetic is instant, Winamp-style; the
    // default ~0.08s animation smears token colors across every widget).
    ctx.style_mut(|s| s.animation_time = 0.0);
}

/// System font files probed (in order) for the text fallback — covers
/// characters egui's bundled fonts lack (e.g. U+2010 HYPHEN in "Ne‐Yo",
/// which Ubuntu-Light's subset only starts covering at U+2013). First
/// existing, valid file wins; `.ttc` collections are skipped (ab_glyph
/// reads a single face, index 0). App-wide, not per-theme.
pub const SYSTEM_FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
    "C:\\Windows\\Fonts\\segoeui.ttf",
];

/// Append the first available system font to the fallback chain of both font
/// families so characters the bundled fonts miss render instead of tofu
/// boxes. Called once at startup. Returns whether a font was installed.
pub fn install_fallback_fonts(ctx: &egui::Context, candidates: &[&str]) -> bool {
    for path in candidates {
        let Ok(bytes) = std::fs::read(Path::new(path)) else { continue };
        let magic = &bytes[..bytes.len().min(4)];
        if magic == [0, 1, 0, 0] || magic == b"OTTO" {
            let mut defs = egui::FontDefinitions::default();
            defs.font_data.insert(
                "system-fallback".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                defs.families
                    .entry(family)
                    .or_default()
                    .push("system-fallback".to_owned());
            }
            ctx.set_fonts(defs);
            return true;
        }
    }
    false
}

/// Returns (row_rect, child_ui) with banding, accent tint, 3px stripe, and 6px inset.
/// `i` is the row index for even/odd banding. `is_current` highlights the playing
/// track: a translucent `row_tint_alpha` accent overlay over the banded bg (keeps
/// text readable, unlike a gamma-multiplied fill which darkens past the bg) plus
/// a 3px accent stripe.
pub fn row(ui: &mut egui::Ui, i: usize, is_current: bool, row_h: f32, theme: &Theme) -> (egui::Rect, egui::Ui) {
    let p = theme.palette;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
    let bg = if i % 2 == 0 { p.row_even } else { p.row_odd };
    ui.painter().rect_filled(rect, 2.0, bg);
    if is_current {
        let alpha = theme.layout.with_defaults().row_tint_alpha;
        let tint = Color32::from_rgba_unmultiplied(
            p.accent.r(),
            p.accent.g(),
            p.accent.b(),
            (alpha * 255.0).round() as u8,
        );
        ui.painter().rect_filled(rect, 2.0, tint);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            0.0,
            p.accent,
        );
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(rect.min + egui::vec2(6.0, 0.0), rect.max))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    (rect, child)
}