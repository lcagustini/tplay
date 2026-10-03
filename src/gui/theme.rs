//! Theme system — data-driven Winamp-style token palettes.
//!
//! Themes live in `themes/<id>/theme.json` folders, found in three places
//! (highest priority first):
//!   1. `~/.config/tplay/themes/`   — user themes (win on id clash)
//!   2. `<exe_dir>/themes/`         — shipped with the app
//!   3. `./themes/`                 — dev convenience (`cargo run` from repo root)
//!
//! Each `theme.json` carries the 13-token palette, `base` (dark/light), and an
//! optional `metadata_font`. Icons are per-theme SVGs under `<theme>/icons/`,
//! rasterized through resvg at startup and on a skin switch, falling back to
//! the default theme's, then to unicode glyphs. Each theme's SVGs carry that
//! theme's own token baked into their `fill`. Selection persists in the `theme`
//! field of `~/.config/tplay/config.json`.
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
}

/// The two bundled font families, as a theme names them.
///
/// A theme may not spell `egui::FontFamily` directly: its serde derive sits
/// behind epaint's own feature, and this module must not depend on which
/// features eframe happened to turn on. `Base` above sets the precedent — a
/// two-variant theme enum with a conversion — so this is that, for the one
/// other closed set a theme picks from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    #[default]
    Proportional,
    Monospace,
}

impl From<Family> for FontFamily {
    fn from(f: Family) -> FontFamily {
        match f {
            Family::Proportional => FontFamily::Proportional,
            Family::Monospace => FontFamily::Monospace,
        }
    }
}

/// The type scale: which family, and how big each role is.
///
/// One number per *role* rather than per label, because egui already has the
/// roles — `Style::text_styles`. A theme that sets four numbers restyles every
/// heading, body label, metadata line and time in the app, including the ~19
/// labels that never build a `FontId` by hand.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Type {
    /// Family for headings, buttons and body labels.
    pub family: Family,
    /// Family for metadata and times. Monospace by default, because tabular
    /// digits are what stops a seek bar's two time labels from jittering.
    #[serde(default = "mono_default")]
    pub mono: Family,
    /// Pane titles, dialog titles, section labels.
    pub heading: f32,
    /// Buttons and ordinary labels.
    pub body: f32,
    /// Captions, tags, breadcrumb segments, anything de-emphasised.
    pub meta: f32,
    /// Durations and the transport clock.
    pub time: f32,
}

fn mono_default() -> Family {
    Family::Monospace
}

impl Default for Type {
    fn default() -> Self {
        // egui's own `default_text_styles` for heading and body, so an untouched
        // theme renders exactly as the app does today; meta and time are the
        // sizes `Layout::text_meta` / `text_time` already used.
        Self {
            family: Family::default(),
            mono: mono_default(),
            heading: 18.0,
            body: 13.0,
            meta: 12.0,
            time: 13.0,
        }
    }
}

/// Widget appearance: shape, the stroke widths, and the handful of `Visuals`
/// fields egui colours from outside the palette.
///
/// Every one of these reaches egui through `apply` alone, so this group is one
/// function's worth of change rather than a per-pane edit. `radius` is a single
/// number for all five widget states on purpose — a skin wants square or softly
/// rounded, not five independently chosen values — and `button_border` is
/// optional because a resting button today has no frame at all, so making it one
/// would change every button in the app.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Chrome {
    /// Corner radius, all five widget states.
    pub radius: f32,
    /// Width of a widget's frame (noninteractive / hovered / active / open).
    pub border_w: f32,
    /// Width of a hovered or active widget's text-and-knob stroke.
    pub fg_w: f32,
    /// Width of the focus ring.
    pub focus_w: f32,
    /// Frame colour for a button at rest. `None` leaves it frameless, which is
    /// what every button does today.
    pub button_border: Option<Color32>,
    /// Width of the text-edit caret. 0 hides it.
    pub cursor_w: f32,
    /// Slider and checkbox handle: 0 is a circle, anything else is a rectangle
    /// of that width-over-height ratio.
    pub handle_aspect: f32,
    /// Alpha applied to a widget the app has disabled.
    pub disabled_alpha: f32,
    /// How far an expanded widget's frame reaches past its bounds.
    pub expansion: f32,
    /// Colour of de-emphasised text egui dims itself. `None` keeps its default.
    pub weak_text: Option<Color32>,
    /// Text-edit background. `None` keeps `extreme_bg_color`.
    pub text_edit_bg: Option<Color32>,
    /// Window and popup outline. `None` keeps egui's.
    pub window_stroke: Option<Color32>,
    /// Corner radius of a window or popup frame.
    pub window_radius: f32,
    /// Dimming drawn behind a modal. `None` keeps egui's hardcoded black.
    pub backdrop: Option<Color32>,
    /// Seconds a widget colour cross-fade takes. 0 is the instant, Winamp-snappy
    /// switch the app ships with; a theme may raise it to animate.
    pub motion: f32,
    /// Horizontal gap between widgets, app-wide.
    pub item_gap: f32,
    /// Padding inside a button's frame.
    pub button_pad: f32,
    /// Padding between a menu and the window it drops from.
    pub menu_pad: f32,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            // 2.0 is what four of egui's five states already use; hovered is 3.
            radius: 2.0,
            border_w: 1.0,
            fg_w: 1.5,
            focus_w: 1.0,
            button_border: None,
            cursor_w: 2.0,
            handle_aspect: 0.75,
            disabled_alpha: 0.5,
            expansion: 0.0,
            weak_text: None,
            text_edit_bg: None,
            window_stroke: None,
            window_radius: 6.0,
            backdrop: None,
            motion: 0.0,
            item_gap: 8.0,
            button_pad: 6.0,
            menu_pad: 8.0,
        }
    }
}

/// List-row chrome, shared by the Playlist and Library panes.
///
/// These are five numbers for three row draws: the row height was written down
/// separately in the playlist, the file list and the share list, and the same
/// two values were spelled out for each. `theme::row` is the one function every
/// row passes through, so it is the only place these need reading.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Rows {
    /// Height of one track or folder row.
    pub height: f32,
    /// Corner radius of the row's fill and of the active-row tint over it.
    pub radius: f32,
    /// Width of the accent stripe down the active row's left edge.
    pub stripe_w: f32,
    /// Inset of row content from the row's left edge, which clears the stripe.
    pub inset: f32,
    /// Gap between two widgets inside a playlist row. The Library's file rows
    /// are tighter and read `cell_gap` instead, because their cells are columns.
    pub gap: f32,
    /// Width reserved at a row's right end when it has no artist·album cell:
    /// the ✕ button, the FORMAT cell and their gaps.
    pub trailing_w: f32,
    /// Gamma factor applied to a row's title when it is *not* the playing track.
    pub dim: f32,
    /// Alpha of the active row's accent overlay, 0 to leave it untinted.
    pub tint_alpha: f32,
    /// Height of the Library's sortable column-header strip.
    pub header_h: f32,
    /// Narrowest a row's title column may get before the row stops shrinking.
    pub title_floor: f32,
    /// Gap between two row cells.
    pub cell_gap: f32,
    /// Width of each sortable column, in `SORT_OPTIONS` order. A `0.0` column
    /// takes the leftover width, which is how Title flexes.
    pub columns: [f32; crate::library::SORT_OPTIONS.len()],
}

impl Default for Rows {
    fn default() -> Self {
        Self {
            height: 24.0,
            radius: 2.0,
            stripe_w: 3.0,
            inset: 6.0,
            gap: 8.0,
            trailing_w: 130.0,
            dim: 0.85,
            tint_alpha: 0.10,
            header_h: 22.0,
            title_floor: 40.0,
            cell_gap: 4.0,
            columns: [0.0, 90.0, 100.0, 44.0],
        }
    }
}

/// Per-pane sizing: icon sizes, the sidebar column, the top bar, and the few
/// fixed widths that are a taste call rather than a derived figure.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Panes {
    /// Height of the window's title bar.
    pub topbar_h: f32,
    /// The ☰ menu button. 18 is the logo's real budget; larger and the bolt and
    /// the play triangle merge into one mass.
    pub logo: f32,
    /// Minimize / maximize / restore.
    pub chrome_icon: f32,
    /// Transport, shuffle, repeat, gapless, crossfade.
    pub transport_icon: f32,
    /// A row's ✕ and the sidebar's ✕.
    pub small_icon: f32,
    /// A folder row's icon.
    pub folder_icon: f32,
    /// The smallest marks: the Library's sort arrow and the row rating star.
    pub tiny_icon: f32,
    /// Playlist toolbar buttons.
    pub toolbar_icon: f32,
    /// The volume speaker beside its slider.
    pub volume_icon: f32,
    /// The Library's Places / Volumes / Favorites column. Sized against the
    /// 320px minimum window, which is why it cannot grow much.
    pub sidebar_w: f32,
    /// Height of one sidebar row, and of the add-server form's text fields.
    pub sidebar_row_h: f32,
    /// Width of a sidebar row's label, leaving room for the ✕ on removable rows.
    pub sidebar_row_w: f32,
    /// Width of a Network row's label. Narrower than a Places row, because the
    /// ✕ sits inside the same column and host strings are longer than labels.
    pub sidebar_server_w: f32,
    /// Width of the add-server form's fields, inside the fixed-width column.
    pub sidebar_form_w: f32,
    /// Height of one breadcrumb segment.
    pub breadcrumb_h: f32,
    /// Cap on one breadcrumb segment, past which its label truncates.
    pub breadcrumb_max_w: f32,
    /// Width of a search or text field.
    pub field_w: f32,
    /// Narrowest the seek bar may get between its two time labels.
    pub seek_min_w: f32,
    /// Width of a row's artist·album cell.
    pub meta_w: f32,
    /// Widest the Equalizer's band column gets before the row shrinks to fit.
    pub eq_band_w_max: f32,
    /// Minimum width of the Yes/No dialog.
    pub confirm_w: f32,
    /// Width of the track editor's dialog, and of a text field within it. This is
    /// also the app's minimum window width.
    pub edit_w: f32,
    /// Width of the editor's label column, so its four inputs line up.
    pub edit_label_w: f32,
    /// The star size in the editor, bigger than a row's inline star because this
    /// is the one place a rating is *chosen* rather than scanned.
    pub edit_star: f32,
    /// The Album Cover pane's no-art placeholder, as a fraction of the pane's
    /// short side. It is painted at pane size rather than at a fixed raster, so
    /// a fraction is the only honest way to state it.
    pub nocover_frac: f32,
    /// Vertical gap between two stacked rows of controls.
    pub gap: f32,
    /// Padding between a pane's edge and its content.
    pub pad: f32,
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            topbar_h: 22.0,
            logo: 18.0,
            chrome_icon: 16.0,
            transport_icon: 18.0,
            small_icon: 13.0,
            folder_icon: 14.0,
            tiny_icon: 10.0,
            toolbar_icon: 16.0,
            volume_icon: 15.0,
            sidebar_w: 120.0,
            sidebar_row_h: 18.0,
            sidebar_row_w: 100.0,
            sidebar_server_w: 88.0,
            sidebar_form_w: 100.0,
            breadcrumb_h: 18.0,
            breadcrumb_max_w: 70.0,
            field_w: 220.0,
            seek_min_w: 40.0,
            meta_w: 150.0,
            eq_band_w_max: 80.0,
            confirm_w: 260.0,
            edit_w: 320.0,
            edit_label_w: 56.0,
            edit_star: 22.0,
            nocover_frac: 0.35,
            gap: 4.0,
            pad: 8.0,
        }
    }
}

/// Docking chrome: tab strips and splitters.
///
/// The largest hole the token system had. `egui_dock::Style` is rebuilt from the
/// live egui style every frame and then given three fields, so every other tab
/// and splitter colour and every corner radius was egui's own.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Dock {
    /// Height of a tab strip.
    pub tab_h: f32,
    /// Corner radius of a tab.
    pub tab_radius: f32,
    /// Fill of an inactive tab. `None` keeps egui's.
    pub tab_bg: Option<Color32>,
    /// Fill of the selected tab. `None` keeps egui's.
    pub tab_bg_active: Option<Color32>,
    /// Outline of a tab. `None` keeps egui's.
    pub tab_stroke: Option<Color32>,
    /// Fill of a tab while it is being dragged.
    pub drag_fill: Option<Color32>,
    /// Title of an inactive tab. `None` keeps egui's.
    pub tab_text: Option<Color32>,
    /// Title of the selected tab.
    pub tab_text_active: Color32,
    /// Width of a splitter between two panes.
    pub split_w: f32,
    /// Colour of a splitter at rest.
    pub split_color: Option<Color32>,
    /// Colour of a splitter being dragged.
    pub split_color_active: Option<Color32>,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            tab_h: 22.0,
            tab_radius: 2.0,
            tab_bg: None,
            tab_bg_active: None,
            tab_stroke: None,
            drag_fill: None,
            tab_text: None,
            tab_text_active: Color32::from_gray(230),
            split_w: 4.0,
            split_color: None,
            split_color_active: None,
        }
    }
}

/// The visualizer's palette, which reaches the GPU rather than a `Painter`.
///
/// Only two tokens, and that is a measured ceiling rather than a stopping
/// point. Ten of the thirteen palette tokens never left the CPU before this
/// group existed: the harness uploaded `bg`, `accent` and `progress_fill` and
/// every shipped shader read exactly those three, so overriding the first two
/// is what lets a skin paint the visualizer a colour the rest of the player
/// does not use.
///
/// **A third and fourth colour were cut, deliberately.** `peak` and `trail`
/// would each have been a `vec4` in the uniform block with no shipped shader
/// declaring them, which is the `u_level` mistake the `gpu.rs` module docs write
/// up: an input nobody reads is a cost with no capability attached. A view has to
/// want one before the block grows to carry it. The block has the headroom —
/// `downlevel_defaults` promises 16 KiB and the whole thing is 736 bytes — so
/// this is about not shipping a declaration with no reader, not about room.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Viz {
    /// Overrides `palette.bg` for the pane's own fill and every shader's
    /// background. `None` keeps the palette's.
    pub bg: Option<Color32>,
    /// Overrides `palette.accent` — the body colour of bars, the ring of radial,
    /// the glow of chladni, and the deposit trails leaves behind.
    pub accent: Option<Color32>,
}

/// Read one number out of a token group, falling back to `d` when the theme
/// omitted it.
///
/// `Option` rather than the `> 0.0` test `Layout::with_defaults` uses, because
/// several new tokens have a meaningful zero — a square `radius`, an instant
/// `motion`, no `expansion`, a circular slider handle, an untinted active row —
/// and a sentinel test would silently discard exactly the values a Winamp skin
/// wants most.
fn num(v: &serde_json::Value, k: &str, d: f32) -> f32 {
    v.get(k)
        .and_then(|x| x.as_f64())
        .map(|n| n as f32)
        .unwrap_or(d)
}

/// Read one optional colour out of a token group. A token that is present but
/// not a colour is a typo in a hand-editable file, so it says so rather than
/// quietly rendering the fallback forever.
fn tok(v: &serde_json::Value, k: &str) -> Option<Color32> {
    let s = v.get(k)?.as_str()?;
    match Color32::from_hex(s) {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("theme token `{k}` is not a colour ({s:?}: {e:?}); using the default");
            None
        }
    }
}

fn family(v: &serde_json::Value, k: &str, d: Family) -> Family {
    match v.get(k).and_then(|x| x.as_str()) {
        Some("monospace") => Family::Monospace,
        Some("proportional") => Family::Proportional,
        _ => d,
    }
}

/// Read a column-width array, filling whatever the theme left out from `d`.
///
/// `zip` rather than indexing, so a `columns` array of the wrong length fills
/// what it can instead of panicking on a `theme.json` this module is only meant
/// to warn about. The array is sized by `SORT_OPTIONS::len()`, so a fifth
/// sortable column is a compile error here rather than a silently unwritten row.
fn columns(
    v: &serde_json::Value,
    d: [f32; crate::library::SORT_OPTIONS.len()],
) -> [f32; crate::library::SORT_OPTIONS.len()] {
    let Some(a) = v.get("columns").and_then(|x| x.as_array()) else {
        return d;
    };
    let mut out = d;
    for (slot, n) in out.iter_mut().zip(a.iter().filter_map(|x| x.as_f64())) {
        *slot = n as f32;
    }
    out
}

impl Type {
    fn from_json(v: &serde_json::Value) -> Type {
        let d = Type::default();
        Type {
            family: family(v, "family", d.family),
            mono: family(v, "mono", d.mono),
            heading: num(v, "heading", d.heading),
            body: num(v, "body", d.body),
            meta: num(v, "meta", d.meta),
            time: num(v, "time", d.time),
        }
    }
}

impl Chrome {
    fn from_json(v: &serde_json::Value) -> Chrome {
        let d = Chrome::default();
        Chrome {
            radius: num(v, "radius", d.radius),
            border_w: num(v, "border_w", d.border_w),
            fg_w: num(v, "fg_w", d.fg_w),
            focus_w: num(v, "focus_w", d.focus_w),
            button_border: tok(v, "button_border"),
            cursor_w: num(v, "cursor_w", d.cursor_w),
            handle_aspect: num(v, "handle_aspect", d.handle_aspect),
            disabled_alpha: num(v, "disabled_alpha", d.disabled_alpha),
            expansion: num(v, "expansion", d.expansion),
            weak_text: tok(v, "weak_text"),
            text_edit_bg: tok(v, "text_edit_bg"),
            window_stroke: tok(v, "window_stroke"),
            window_radius: num(v, "window_radius", d.window_radius),
            backdrop: tok(v, "backdrop"),
            motion: num(v, "motion", d.motion),
            item_gap: num(v, "item_gap", d.item_gap),
            button_pad: num(v, "button_pad", d.button_pad),
            menu_pad: num(v, "menu_pad", d.menu_pad),
        }
    }
}

impl Rows {
    fn from_json(v: &serde_json::Value) -> Rows {
        let d = Rows::default();
        Rows {
            height: num(v, "height", d.height),
            radius: num(v, "radius", d.radius),
            stripe_w: num(v, "stripe_w", d.stripe_w),
            inset: num(v, "inset", d.inset),
            gap: num(v, "gap", d.gap),
            trailing_w: num(v, "trailing_w", d.trailing_w),
            dim: num(v, "dim", d.dim),
            tint_alpha: num(v, "tint_alpha", d.tint_alpha),
            header_h: num(v, "header_h", d.header_h),
            title_floor: num(v, "title_floor", d.title_floor),
            cell_gap: num(v, "cell_gap", d.cell_gap),
            columns: columns(v, d.columns),
        }
    }
}

impl Panes {
    fn from_json(v: &serde_json::Value) -> Panes {
        let d = Panes::default();
        Panes {
            topbar_h: num(v, "topbar_h", d.topbar_h),
            logo: num(v, "logo", d.logo),
            chrome_icon: num(v, "chrome_icon", d.chrome_icon),
            transport_icon: num(v, "transport_icon", d.transport_icon),
            small_icon: num(v, "small_icon", d.small_icon),
            folder_icon: num(v, "folder_icon", d.folder_icon),
            tiny_icon: num(v, "tiny_icon", d.tiny_icon),
            toolbar_icon: num(v, "toolbar_icon", d.toolbar_icon),
            volume_icon: num(v, "volume_icon", d.volume_icon),
            sidebar_w: num(v, "sidebar_w", d.sidebar_w),
            sidebar_row_h: num(v, "sidebar_row_h", d.sidebar_row_h),
            sidebar_row_w: num(v, "sidebar_row_w", d.sidebar_row_w),
            sidebar_server_w: num(v, "sidebar_server_w", d.sidebar_server_w),
            sidebar_form_w: num(v, "sidebar_form_w", d.sidebar_form_w),
            breadcrumb_h: num(v, "breadcrumb_h", d.breadcrumb_h),
            breadcrumb_max_w: num(v, "breadcrumb_max_w", d.breadcrumb_max_w),
            field_w: num(v, "field_w", d.field_w),
            seek_min_w: num(v, "seek_min_w", d.seek_min_w),
            meta_w: num(v, "meta_w", d.meta_w),
            eq_band_w_max: num(v, "eq_band_w_max", d.eq_band_w_max),
            confirm_w: num(v, "confirm_w", d.confirm_w),
            edit_w: num(v, "edit_w", d.edit_w),
            edit_label_w: num(v, "edit_label_w", d.edit_label_w),
            edit_star: num(v, "edit_star", d.edit_star),
            nocover_frac: num(v, "nocover_frac", d.nocover_frac),
            gap: num(v, "gap", d.gap),
            pad: num(v, "pad", d.pad),
        }
    }
}

impl Dock {
    fn from_json(v: &serde_json::Value) -> Dock {
        let d = Dock::default();
        Dock {
            tab_h: num(v, "tab_h", d.tab_h),
            tab_radius: num(v, "tab_radius", d.tab_radius),
            tab_bg: tok(v, "tab_bg"),
            tab_bg_active: tok(v, "tab_bg_active"),
            tab_stroke: tok(v, "tab_stroke"),
            drag_fill: tok(v, "drag_fill"),
            tab_text: tok(v, "tab_text"),
            tab_text_active: tok(v, "tab_text_active").unwrap_or(d.tab_text_active),
            split_w: num(v, "split_w", d.split_w),
            split_color: tok(v, "split_color"),
            split_color_active: tok(v, "split_color_active"),
        }
    }
}

impl Viz {
    fn from_json(v: &serde_json::Value) -> Viz {
        Viz {
            bg: tok(v, "bg"),
            accent: tok(v, "accent"),
        }
    }
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
    pub palette: Palette,
    /// Per-theme UI layout/sizing tokens. `from_json` runs
    /// `Layout::with_defaults` on the way in, so a pane may read these
    /// directly. Every field is `pub`, so a `Theme` built by struct literal
    /// rather than by `from_json` owns applying the defaults itself.
    pub layout: Layout,
    /// `<theme_dir>/icons`, when the folder exists.
    pub icons_dir: Option<PathBuf>,
    /// `<theme_dir>/<font>`, when the theme names a font file that exists. Not
    /// a token value: like `icons_dir` it is a path resolved against the theme's
    /// own folder, and a theme with no font file is the normal case.
    pub font_file: Option<PathBuf>,
    /// Font family, family and the four role sizes.
    pub type_: Type,
    /// Widget shape, stroke widths and the `Visuals` fields egui owns.
    pub chrome: Chrome,
    /// List-row chrome, shared by the two scrolling lists.
    pub rows: Rows,
    /// Per-pane sizing.
    pub panes: Panes,
    /// Docking chrome.
    pub dock: Dock,
    /// The visualizer's palette, which reaches the GPU rather than a `Painter`.
    pub viz: Viz,
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
                    }
                    .with_defaults()
                })
                .unwrap_or_else(|| Layout::default().with_defaults())
        };
        // Every group is optional. An absent group parses as `Null`, and every
        // `from_json` above reads through `num`/`tok`, which fall back per token.
        // So a `theme.json` written before these groups existed loads unchanged
        // and renders exactly as it did, which is what lets the three bundled
        // themes and any hand-written one keep working with no edits.
        let group = |k: &str| v.get(k).cloned().unwrap_or(serde_json::Value::Null);
        let font_file = v
            .get("font")
            .and_then(|x| x.as_str())
            .map(|name| dir.join(name))
            .filter(|p| p.is_file());

        Some(Theme {
            id,
            name,
            base,
            layout,
            palette,
            icons_dir,
            font_file,
            type_: Type::from_json(&group("type")),
            chrome: Chrome::from_json(&group("chrome")),
            rows: Rows::from_json(&group("rows")),
            panes: Panes::from_json(&group("panes")),
            dock: Dock::from_json(&group("dock")),
            viz: Viz::from_json(&group("viz")),
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
                    log::warn!("skipping the invalid theme in {}", theme_dir.display());
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

    /// The default theme (fallback icon source when a theme has no file of
    /// its own). `load()` guarantees a non-empty list.
    pub fn default(&self) -> &Arc<Theme> {
        self.get(DEFAULT_THEME_ID)
            .or_else(|| self.list.first())
            .expect("Themes::load always produces at least one theme")
    }

    /// Resolve an icon file for a theme: the theme's own file → the default
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

/// How many samples per axis each output pixel averages. resvg already
/// antialiases, but a 20px glyph straight off a 512 viewBox is a hard
/// minification — supersampling and box-averaging is what keeps a 1px diagonal
/// stroke from aliasing into dashes.
const ICON_SS: u32 = 4;

/// Rasterize one icon SVG to an egui image, or `None` if the file is missing,
/// unparseable or empty.
///
/// Public so `tests/gui_tests.rs` can assert every bundled icon really rasterizes
/// — see `every_icon_has_a_parseable_svg_source`.
///
/// The scale comes from the tree's own size rather than a hardcoded 512: the
/// game-icons files are `viewBox="0 0 512 512"` while the hand-authored ones are
/// `0 0 20 20`, and both still have to land on `px`.
///
/// The box-average runs on **premultiplied** RGBA, the only order that is correct
/// where a pixel is partly covered — averaging straight alpha darkens every
/// antialiased edge. `from_rgba_premultiplied` then hands egui exactly that, so
/// nothing is unpremultiplied on the way out.
pub fn rasterize_icon(path: &Path, px: u32) -> Option<egui::ColorImage> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }

    let big = px * ICON_SS;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(big, big)?;
    let scale = big as f32 / size.width().max(size.height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let src = pixmap.data();
    let area = (ICON_SS * ICON_SS) as u16;
    let mut out = vec![0u8; (px * px * 4) as usize];
    for (i, chunk) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % px, i as u32 / px);
        let mut acc = [0u16; 4];
        for sy in 0..ICON_SS {
            for sx in 0..ICON_SS {
                let at = (((y * ICON_SS + sy) * big + (x * ICON_SS + sx)) * 4) as usize;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src[at + c] as u16;
                }
            }
        }
        for (c, out) in chunk.iter_mut().enumerate() {
            *out = (acc[c] / area) as u8;
        }
    }
    Some(egui::ColorImage::from_rgba_premultiplied(
        [px as usize, px as usize],
        &out,
    ))
}

/// Rasterize a theme's icons (own SVG → default theme's) into GPU textures, one
/// per `Icon::ALL` slot. Missing/unparseable → `None`, and the pane draws a glyph.
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
            let path = themes.icon_path(theme, icon)?;
            let color = rasterize_icon(&path, icon.px())?;
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
/// it was the setter, which had to know that switching a theme means re-decoding
/// every icon texture and did it inline. That coupling belongs next to
/// `load_icons`, and `set` is where it lives.
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
        log::info!("switched to the {} theme", theme.name);
        self.current = Arc::clone(theme);
        self.icons = load_icons(ctx, &self.themes, &self.current);
        true
    }
}

/// Raster size of every toolbar icon, in pixels. The display size is the
/// caller's (`TOOLBAR_ICON` and friends) — this is what the texture is authored
/// at, which is what keeps a 20px glyph from aliasing.
pub const ICON_PX: u32 = 20;

/// The Album Cover pane's placeholder is painted at pane size, so it is
/// rasterized large rather than minified down.
pub const NOCOVER_PX: u32 = 128;

/// Pane icons. Each maps to a `<theme>/icons/<name>.svg` and a fallback glyph.
/// Fallback glyphs must exist in the bundled egui font stack (Ubuntu-Light /
/// NotoEmoji / emoji-icon-font) or they render as tofu boxes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    /// App logo — menu button in the top panel (per-theme file, fallback glyph).
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
    /// Window chrome: minimize / maximize / restore (text_primary, per theme).
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
    /// Window chrome: un-maximize a maximized window (text_primary, per theme).
    Restore,
}

impl Icon {
    pub const ALL: [Icon; 24] = [
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
        Icon::Restore,
    ];

    /// `(file name, last-resort glyph)` per `ALL` slot, indexed by
    /// `self as usize`. **Append-only**: a new `Icon` goes at the end of the enum
    /// and the end of both arrays, or every later icon decodes the wrong file.
    ///
    /// 13 of the 24 are [game-icons.net](https://game-icons.net) artwork under
    /// CC BY 3.0 (see `CREDITS`); the rest are original. Each is a per-theme SVG
    /// with that theme's own palette token baked into its `fill`, which is why a
    /// single file can never serve two themes. A glyph is only ever painted when
    /// neither the theme nor the default ships the file, and several glyphs are
    /// known to tofu in the bundled egui font — which is exactly why the SVGs
    /// exist.
    const DATA: [(&'static str, &'static str); 24] = [
        ("logo.svg", "☰"),
        ("play.svg", "▶"),
        ("pause.svg", "⏸"),
        ("stop.svg", "⏹"),
        ("prev.svg", "⏮"),
        ("next.svg", "⏭"),
        ("shuffle.svg", "🔀"),
        ("repeat.svg", "🔁"),
        ("volume.svg", "🔊"),
        ("remove.svg", "×"),
        ("sort_asc.svg", "⏶"),
        ("sort_desc.svg", "⏷"),
        ("star_on.svg", "★"),
        ("star_off.svg", "☆"),
        ("folder.svg", "📁"),
        ("minimize.svg", "🗕"),
        ("maximize.svg", "🗖"),
        ("nocover.svg", "🎵"),
        ("gapless.svg", "⏩"),
        ("crossfade.svg", "🔗"),
        ("reverse.svg", "⇅"),
        ("new_list.svg", "✳"),
        ("save.svg", "↓"),
        ("restore.svg", "⧉"),
    ];

    /// The raster size this icon is drawn at, in pixels. Everything is a 20px
    /// toolbar mark except the Album Cover pane's placeholder, which is painted
    /// at pane size and so has to be rasterized large to stay crisp.
    ///
    /// It cannot be one constant for all of them: rendering everything at 128 and
    /// letting `TextureOptions::LINEAR` minify 6.4x down to a 20px button
    /// shimmers on thin strokes, because there is no mip chain to pre-filter it.
    pub fn px(self) -> u32 {
        if self == Icon::NoCover {
            NOCOVER_PX
        } else {
            ICON_PX
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// The file this slot reads, `<name>.svg`. Public so
    /// `tests/gui_tests.rs::no_icon_file_is_without_a_slot` can ask the other
    /// direction: which files on disk have no slot behind them.
    pub fn file_name(self) -> &'static str {
        Self::DATA[self.index()].0
    }

    /// Last-resort glyph when neither this theme nor the default ships the SVG.
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

/// An always-enabled icon button whose *lit* state is one value — the mode
/// toggles: shuffle, repeat, gapless, crossfade.
///
/// `icon_button`'s two `bool`s sit at the end of a five-argument call where a
/// stray `true` is indistinguishable from the one beside it, and every one of
/// these sites read `true, app.shuffle()`. A sibling with a single `bool` is the
/// whole fix, and it is the same reason `sidebar::Row` exists rather than four
/// more positional parameters: a shape that cannot be filled in wrongly.
pub fn icon_toggle(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    icon: Icon,
    size: f32,
    on: bool,
) -> egui::Response {
    icon_button(ui, tex, icon, size, true, on)
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

/// The metadata role: `type.meta`'s size in the metadata family.
///
/// **Not `.small()`.** That carries `type.family`, but a theme whose `type.mono`
/// is monospace needs its captions *and its times* monospaced — which is the
/// whole reason retro looks like retro, since monospace digits are tabular and
/// a seek bar's two time labels would otherwise jitter as they count. egui has
/// no style slot for "small size, mono family", so this is that combination.
pub fn meta_font(t: &Type) -> egui::FontId {
    egui::FontId::new(t.meta, t.mono.into())
}

/// The time role: `type.time`'s size in the metadata family. See [`meta_font`].
pub fn time_font(t: &Type) -> egui::FontId {
    egui::FontId::new(t.time, t.mono.into())
}

/// Map the token groups onto egui's `Visuals` and `Style`. (egui has no 1:1
/// token slots, so nearby fields stand in; the ones that can't be expressed here
/// — row banding — are read straight from `theme.palette` by the panes.)
///
///   --bg              panel_fill / window_fill / extreme_bg_color
///   --panel-bg        faint_bg_color / noninteractive.bg_fill
///   --text-primary    override_text_color
///   --accent          hyperlink_color / active.bg_fill / text cursor
///   --progress-fill   selection.bg_fill (the slider's trailing fill)
///   --slider-track    inactive.bg_fill (the slider rail)
///   --slider-handle   inactive.fg_stroke (the slider knob outline)
///   --btn-hover       hovered.bg_fill
///   --border          noninteractive.bg_stroke
///   --focus-ring      selection.stroke (egui focus rectangles)
///
/// `chrome` and `type_` fill the gaps the palette cannot: corner radius and every
/// stroke width, the four optional colours egui owns, widget density, and the
/// text scale. **`Style::text_styles` is the largest single win here** — it is
/// what the ~19 labels that never build a `FontId` by hand resolve against, and
/// until now no theme could reach any of them.
///
/// Both styles are written, not just the active one. `set_visuals` touches only
/// whichever the system preference selects, so the other kept whatever it had
/// when egui built it; a mid-session OS theme flip then surfaced stale greys.
/// Every field here is idempotent, so writing both costs nothing.
pub fn apply(ctx: &egui::Context, theme: &Theme) {
    let p = theme.palette;
    let c = &theme.chrome;
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
    v.weak_text_color = c.weak_text;
    v.text_edit_bg_color = c.text_edit_bg;
    v.hyperlink_color = p.accent;
    v.selection = egui::style::Selection {
        bg_fill: p.progress_fill,
        stroke: Stroke::new(c.focus_w, p.focus_ring),
    };

    // An optional `Visuals` colour that a theme left out keeps egui's own value,
    // which for these three is a sensible neutral rather than a wrong one.
    v.window_stroke = Stroke::new(c.border_w, c.window_stroke.unwrap_or(p.border));
    v.window_corner_radius = c.window_radius.into();
    v.menu_corner_radius = c.window_radius.into();
    v.disabled_alpha = c.disabled_alpha;
    v.text_cursor.stroke = Stroke::new(c.cursor_w, p.accent);
    v.handle_shape = if c.handle_aspect > 0.0 {
        egui::style::HandleShape::Rect {
            aspect_ratio: c.handle_aspect,
        }
    } else {
        egui::style::HandleShape::Circle
    };

    let radius: egui::CornerRadius = c.radius.into();
    let border = Stroke::new(c.border_w, p.border);
    let fg = Stroke::new(c.fg_w, p.text_primary);

    v.widgets.noninteractive.bg_fill = p.panel_bg;
    v.widgets.noninteractive.bg_stroke = border;
    v.widgets.noninteractive.fg_stroke = Stroke::new(c.border_w, p.text_secondary);
    v.widgets.inactive.weak_bg_fill = p.btn_hover;
    v.widgets.inactive.bg_fill = p.slider_track;
    v.widgets.inactive.fg_stroke = Stroke::new(c.border_w, p.slider_handle);
    // A resting button has no frame today, so the border colour is optional
    // rather than defaulting to `--border`: making one the default would put a
    // ring around every button in the app.
    v.widgets.inactive.bg_stroke = c
        .button_border
        .map(|col| Stroke::new(c.border_w, col))
        .unwrap_or(Stroke::NONE);
    v.widgets.hovered.weak_bg_fill = p.btn_hover;
    v.widgets.hovered.bg_fill = p.btn_hover;
    v.widgets.hovered.fg_stroke = fg;
    v.widgets.hovered.bg_stroke = border;
    v.widgets.active.weak_bg_fill = p.accent;
    v.widgets.active.bg_fill = p.accent;
    v.widgets.active.fg_stroke = fg;
    v.widgets.active.bg_stroke = Stroke::new(c.border_w, p.focus_ring);
    // `open` had no mapping at all, so a ComboBox with its menu down was egui's
    // grey. It reads as hovered, which is what the state means.
    v.widgets.open = v.widgets.hovered;

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
        w.expansion = c.expansion;
    }

    let text_styles = text_styles(&theme.type_);
    let item_gap = c.item_gap;
    let button_pad = c.button_pad;
    let menu_pad = c.menu_pad;
    let motion = c.motion;

    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        s.text_styles = text_styles.clone();
        s.spacing.item_spacing.x = item_gap;
        s.spacing.button_padding = Vec2::splat(button_pad);
        let m = menu_pad.clamp(0.0, i8::MAX as f32) as i8;
        s.spacing.menu_margin = egui::Margin {
            left: m,
            right: m,
            top: m,
            bottom: m,
        };
        // Kill widget colour cross-fades so a mid-session switch lands on the
        // very next frame: the aesthetic is instant, Winamp-style, and the
        // default ~0.08s animation smears token colours across every widget. A
        // theme may raise `motion` to animate a switch instead.
        s.animation_time = motion;
    });
}

/// The four roles egui's `Style` already distinguishes, from one theme scale.
///
/// `Small` follows `meta` rather than keeping egui's 9pt, which is the visible
/// part of this: the breadcrumb segments, the "Playlist" row tag and the
/// section labels are all `.small()` and are all unreadable at 9pt.
fn text_styles(t: &Type) -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    use egui::TextStyle;
    [
        (
            TextStyle::Heading,
            egui::FontId::new(t.heading, t.family.into()),
        ),
        (TextStyle::Body, egui::FontId::new(t.body, t.family.into())),
        (
            TextStyle::Button,
            egui::FontId::new(t.body, t.family.into()),
        ),
        (TextStyle::Small, egui::FontId::new(t.meta, t.family.into())),
        (
            TextStyle::Monospace,
            egui::FontId::new(t.time, t.mono.into()),
        ),
    ]
    .into()
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

/// Put a theme's own font file at the front of both font families.
///
/// A theme may ship a typeface, which is the closest this reaches Winamp's
/// `TEXT.BMP` — a skin that names a font gets it, and one that does not is
/// unaffected. Returns whether the bytes loaded, so a bad or unreadable file
/// falls back to egui's bundled faces instead of leaving the app typeless.
///
/// Front, not back: a theme's font is the one it chose deliberately, so it wins
/// over egui's bundled set for every glyph it has. The system fallback installed
/// afterwards fills only what is left.
pub fn install_theme_font(ctx: &egui::Context, path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let mut defs = egui::FontDefinitions::default();
    defs.font_data.insert(
        "theme-font".to_owned(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        defs.families
            .entry(family)
            .or_default()
            .insert(0, "theme-font".to_owned());
    }
    ctx.set_fonts(defs);
    true
}

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

/// `(row_rect, child_ui)` with banding, accent tint, a left stripe and an inset.
/// `i` is the row index for even/odd banding. `is_current` marks the playing
/// track with a translucent `rows.tint_alpha` accent overlay over the banded bg —
/// which keeps text readable, unlike a gamma-multiplied fill that darkens past
/// the bg — plus the stripe.
///
/// Every row in the app comes through here, so the six numbers below are the
/// whole of list-row chrome. `row_h` stays a parameter because the three call
/// sites each know their own list's row height; `rows.height` is where those
/// three read it from.
pub fn row(
    ui: &mut egui::Ui,
    i: usize,
    is_current: bool,
    row_h: f32,
    theme: &Theme,
) -> (egui::Rect, egui::Ui) {
    let p = theme.palette;
    let r = &theme.rows;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    let bg = if i.is_multiple_of(2) {
        p.row_even
    } else {
        p.row_odd
    };
    let radius: egui::CornerRadius = r.radius.into();
    ui.painter().rect_filled(rect, radius, bg);
    if is_current {
        let tint = Color32::from_rgba_unmultiplied(
            p.accent.r(),
            p.accent.g(),
            p.accent.b(),
            (r.tint_alpha * 255.0).round() as u8,
        );
        ui.painter().rect_filled(rect, radius, tint);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(r.stripe_w, rect.height())),
            0.0,
            p.accent,
        );
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                rect.min + egui::vec2(r.inset, 0.0),
                rect.max,
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = r.gap;
    (rect, child)
}
