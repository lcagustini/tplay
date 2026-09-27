//! Theme system — data-driven Winamp-style token palettes.
//!
//! Themes live in `themes/<id>/theme.json` folders, found in three places
//! (highest priority first):
//!   1. `~/.config/tplay/themes/`   — user themes (win on id clash)
//!   2. `<exe_dir>/themes/`         — shipped with the app
//!   3. `./themes/`                 — dev convenience (`cargo run` from repo root)
//!
//! Each `theme.json` carries the 13-token palette, `base` (dark/light), and an
//! optional `metadata_font`. Icons are per-theme PNGs under `<theme>/icons/`,
//! falling back to the default theme's, then to unicode glyphs. Selection
//! persists in the `theme` field of `~/.config/tplay/config.json`.
//!
//! `palette` exposes the tokens for custom painting (playlist rows, metadata);
//! `apply` maps them onto egui `Visuals` each frame so a mid-session switch
//! lands instantly.

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
    /// Fill in every unset (zero or negative) token. `Theme::from_json` applies
    /// this once on the way in, so no pane re-derives it per frame.
    pub fn with_defaults(self) -> Self {
        // A token the theme omitted arrives as 0.0, so "not set" is "not
        // positive"; a negative value is rejected the same way, since no token
        // has a meaningful negative.
        let or = |v: f32, d: f32| if v > 0.0 { v } else { d };
        Layout {
            eq_slider_min_h: or(self.eq_slider_min_h, 60.0),
            eq_slider_max_h: or(self.eq_slider_max_h, 220.0),
            eq_band_w_min: or(self.eq_band_w_min, 30.0),
            eq_header_gap: or(self.eq_header_gap, 6.0),
            eq_band_gap: or(self.eq_band_gap, 2.0),
            text_meta: or(self.text_meta, 12.0),
            text_time: or(self.text_time, 13.0),
            row_tint_alpha: or(self.row_tint_alpha, 0.10),
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
    /// Per-theme UI layout/sizing tokens. `from_json` runs
    /// `Layout::with_defaults` on the way in, so a pane may read these
    /// directly. Every field is `pub`, so a `Theme` built by struct literal
    /// rather than by `from_json` owns applying the defaults itself.
    pub layout: Layout,
    /// `<theme_dir>/icons`, when the folder exists.
    pub icons_dir: Option<PathBuf>,
}

impl Theme {
    fn from_json(text: &str, dir: &Path) -> Option<Theme> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .or_else(|| dir.file_name().and_then(|n| n.to_str()))
            .unwrap_or(DEFAULT_THEME_ID)
            .to_string();
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
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
        let layout = {
            // Every token is optional; an absent one arrives as 0.0 and is
            // filled in by `with_defaults` on the way out.
            let num = |l: &serde_json::Value, k: &str| {
                l.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32
            };
            v.get("layout")
                .map(|l| {
                    Layout {
                        eq_slider_min_h: num(l, "eq_slider_min_h"),
                        eq_slider_max_h: num(l, "eq_slider_max_h"),
                        eq_band_w_min: num(l, "eq_band_w_min"),
                        eq_header_gap: num(l, "eq_header_gap"),
                        eq_band_gap: num(l, "eq_band_gap"),
                        text_meta: num(l, "text_meta"),
                        text_time: num(l, "text_time"),
                        row_tint_alpha: num(l, "row_tint_alpha"),
                    }
                    .with_defaults()
                })
                .unwrap_or_else(|| Layout::default().with_defaults())
        };

        Some(Theme {
            id,
            name,
            base,
            layout,
            metadata_font,
            palette,
            icons_dir,
        })
    }

    /// Hardcoded dark palette, inserted when no loaded theme has
    /// `id == "dark"` — no themes/ folder, every `theme.json` invalid, or a
    /// renamed id. The app never runs with zero themes.
    fn builtin_fallback() -> Theme {
        // Use include_str so the theme.json is the single source of truth.
        let json = include_str!("../../themes/dark/theme.json");
        Theme::from_json(json, Path::new("")).unwrap()
    }
}

impl Palette {
    fn from_json(v: &serde_json::Value) -> Option<Palette> {
        let tok = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .and_then(|s| Color32::from_hex(s).ok())
        };
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

    /// Load every theme found across the given dirs (first dir wins an id
    /// clash), plus the hardcoded dark fallback.
    pub fn load_from(dirs: &[PathBuf]) -> Themes {
        let mut list: Vec<Arc<Theme>> = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
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
        let own = theme
            .icons_dir
            .as_ref()
            .map(|d| d.join(icon.file_name()))
            .filter(|p| p.exists());
        own.or_else(|| {
            if theme.id == self.default().id {
                return None;
            }
            self.default()
                .icons_dir
                .as_ref()
                .map(|d| d.join(icon.file_name()))
                .filter(|p| p.exists())
        })
    }
}

/// Decode a theme's icons (own PNG → default theme's) into GPU textures, one per
/// `Icon::ALL` slot. Missing/undecodable → `None`, and the pane draws a glyph.
/// Synchronous at startup/theme-switch: the async loader path showed
/// pending/error placeholders and stretched buttons.
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

/// The selected theme, the list, and the loaded icon textures — one group.
///
/// These were three `TPlayApp` fields, and the awkward part was never the fields:
/// it was `set_theme`, which had to know that switching a theme means re-decoding
/// every icon texture and did it inline. That coupling belongs next to
/// `load_icons`.
pub struct ThemeState {
    current: Arc<Theme>,
    themes: Themes,
    icons: Vec<Option<TextureHandle>>,
}

impl ThemeState {
    /// Load `themes` off disk and decode `id`'s icons. An unknown or missing id
    /// falls back to the default theme, so the list is never empty (a broken
    /// install still renders).
    pub fn load(ctx: &egui::Context, themes: Themes, id: &str) -> Self {
        let current = themes
            .get(id)
            .cloned()
            .unwrap_or_else(|| themes.default().clone());
        let icons = load_icons(ctx, &themes, &current);
        Self {
            current,
            themes,
            icons,
        }
    }

    /// The active theme — what `apply` maps onto egui `Visuals` each frame.
    pub fn current(&self) -> &Arc<Theme> {
        &self.current
    }

    /// Every loadable theme, for the Theme dropdown.
    pub fn list(&self) -> &[Arc<Theme>] {
        self.themes.list()
    }

    /// Texture for a pane icon in the current theme (falls back to the default
    /// theme's), or `None` → the pane renders a unicode glyph.
    pub fn icon(&self, icon: Icon) -> Option<&TextureHandle> {
        self.icons.get(icon.index()).and_then(|t| t.as_ref())
    }

    /// Switch by id, re-decoding icons for the new palette. False for an
    /// unknown id or a re-select of the current one — neither is worth a
    /// re-decode, and neither should dirty the config.
    pub fn set(&mut self, ctx: &egui::Context, id: &str) -> bool {
        let Some(theme) = self.themes.get(id) else {
            return false;
        };
        if theme.id == self.current.id {
            return false;
        }
        self.current = Arc::clone(theme);
        self.icons = load_icons(ctx, &self.themes, &self.current);
        true
    }
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
    /// Gapless playback toggle (lit while active).
    Gapless,
    /// Crossfade playback toggle (lit while active).
    Crossfade,
    /// Playlist toolbar: play the list back to front.
    Reverse,
    /// Playlist toolbar: create a new (empty) playlist.
    NewList,
    /// Playlist toolbar: save the playlist to its file.
    Save,
}

impl Icon {
    pub const ALL: [Icon; 23] = [
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
        Icon::Gapless,
        Icon::Crossfade,
        Icon::Reverse,
        Icon::NewList,
        Icon::Save,
    ];

    /// `(file name, last-resort glyph)` per `ALL` slot, indexed by
    /// `self as usize`. **Append-only**: a new `Icon` goes at the end of the enum
    /// and the end of both arrays, or every later icon decodes the wrong file.
    ///
    /// The last three are written by `themes/generate_icons.py`; the rest are
    /// hand-drawn. A glyph is only ever painted when neither the theme nor the
    /// default ships the PNG, and several of them are known to tofu in the
    /// bundled egui font — which is exactly why the PNGs exist.
    const DATA: [(&'static str, &'static str); 23] = [
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
        ("gapless.png", "⏩"),
        ("crossfade.png", "🔗"),
        ("reverse.png", "⇅"),
        ("new_list.png", "✳"),
        ("save.png", "↓"),
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
        Some(h) => egui::Button::image(egui::Image::new(h).fit_to_exact_size(Vec2::splat(size))),
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

/// Map the token palette onto egui's `Visuals`. (egui has no 1:1 token slots, so
/// nearby fields stand in; the ones that can't be expressed here — row banding
/// and disabled text — are read straight from `theme.palette` by the panes.)
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

    // Kill widget color cross-fades so a mid-session switch lands on the very
    // next frame: the aesthetic is instant, Winamp-style, and the default
    // ~0.08s animation smears token colors across every widget.
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
        let Ok(bytes) = std::fs::read(Path::new(path)) else {
            continue;
        };
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

/// `(row_rect, child_ui)` with banding, accent tint, a 3px stripe and 6px inset.
/// `i` is the row index for even/odd banding. `is_current` marks the playing
/// track with a translucent `row_tint_alpha` accent overlay over the banded bg —
/// which keeps text readable, unlike a gamma-multiplied fill that darkens past
/// the bg — plus the 3px stripe.
pub fn row(
    ui: &mut egui::Ui,
    i: usize,
    is_current: bool,
    row_h: f32,
    theme: &Theme,
) -> (egui::Rect, egui::Ui) {
    let p = theme.palette;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    let bg = if i.is_multiple_of(2) {
        p.row_even
    } else {
        p.row_odd
    };
    ui.painter().rect_filled(rect, 2.0, bg);
    if is_current {
        let alpha = theme.layout.row_tint_alpha;
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
            .max_rect(egui::Rect::from_min_max(
                rect.min + egui::vec2(6.0, 0.0),
                rect.max,
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    (rect, child)
}
