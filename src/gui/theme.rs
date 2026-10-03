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
use std::collections::BTreeMap;
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

/// How one 9-sliced source image is cut.
///
/// Winamp states this in `.TXT` files beside each bitmap — two rectangles of the
/// source, one for the tiled/whole case and one for the frame — so the same two
/// numbers appear here: `w`/`h` of the source, and how much of each edge is
/// fixed rather than stretched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stretch {
    /// Fixed pixels along the left edge.
    pub left: u32,
    /// Fixed pixels along the top edge.
    pub top: u32,
    /// Fixed pixels along the right edge.
    pub right: u32,
    /// Fixed pixels along the bottom edge.
    pub bottom: u32,
}

/// One 9-sliced frame: which file, how it is cut, and how it is filtered.
///
/// `src` is resolved inside the theme's own folder, like `icons_dir`, so a theme
/// carries its art with it and a user theme that omits a frame falls back to
/// nothing at all rather than to another theme's.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Frame {
    /// File name inside `<theme>/frames/`.
    pub src: String,
    pub stretch: Stretch,
    /// `false` is nearest-neighbour, which is what pixel art wants; `true` is
    /// linear, for art authored at a larger size than it is drawn.
    ///
    /// The default is nearest because the reference format is a 1x bitmap
    /// magnified onto a HiDPI screen, and the icons already had to learn this the
    /// hard way — see `ICON_PX`.
    #[serde(default)]
    pub smooth: bool,
}

/// The theme's own artwork: the surfaces a skin paints rather than colours.
///
/// **Winamp's substrate, on egui.** A real skin ships `MAIN.BMP`, `PLEDIT.BMP`
/// and `TITLEBAR.BMP` and reads coordinates out of the `.TXT` beside each one;
/// this is the same idea with the coordinates in `theme.json`, drawn by
/// `nine_slice`. What it cannot reach is a per-*element* bitmap — a tab strip,
/// a button face — because those are egui_dock and egui widgets, which paint
/// themselves. Two surfaces ship: the title bar and the pane body.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Frames {
    /// The window's title bar.
    pub titlebar: Option<Frame>,
    /// The docked pane body behind a pane's content.
    pub pane: Option<Frame>,
    /// The Equalizer's own backdrop, behind its controls.
    pub eq: Option<Frame>,
}

impl Frames {
    fn from_json(v: &serde_json::Value) -> Frames {
        let one = |k: &str| -> Option<Frame> {
            let f = v.get(k)?;
            let src = f.get("src")?.as_str()?.to_string();
            let d = Stretch::default();
            let num = |o: &str, dflt: u32| {
                f.get("stretch")
                    .and_then(|s| s.get(o))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(dflt as u64) as u32
            };
            Some(Frame {
                src,
                stretch: Stretch {
                    left: num("left", d.left),
                    top: num("top", d.top),
                    right: num("right", d.right),
                    bottom: num("bottom", d.bottom),
                },
                smooth: f.get("smooth").and_then(|x| x.as_bool()).unwrap_or(false),
            })
        };
        Frames {
            titlebar: one("titlebar"),
            pane: one("pane"),
            eq: one("eq"),
        }
    }
}

/// Draw a 9-sliced frame into `rect`.
///
/// Walks three rows by three columns and paints each cell's slice of the source
/// at its own scale, so the corners keep their size while the edges and the
/// middle stretch. That is the whole mechanism; there is no third mode and no
/// special case for a degenerate row.
///
/// **The centre cell is what makes this cheap.** A theme whose `stretch` covers
/// the whole source has nothing to stretch, and every view that wants a solid
/// surface wants exactly that.
///
/// `rect` smaller than the source is handled by clamping rather than by dividing:
/// a pane narrower than its own frame must still draw, and a negative cell size
/// would panic in the `Painter`.
pub fn nine_slice(
    painter: &egui::Painter,
    rect: egui::Rect,
    tex: egui::TextureHandle,
    st: Stretch,
) {
    let size = tex.size();
    let [sw, sh] = size;
    let (sw, sh) = (sw as f32, sh as f32);
    if sw <= 0.0 || sh <= 0.0 {
        return;
    }
    // Clamp each inset against both the source and the target, then against what
    // its opposite partner already claimed. Without the third step two insets
    // larger than the source put the middle cell at a negative width.
    let l = (st.left as f32).min(sw).min(rect.width());
    let t = (st.top as f32).min(sh).min(rect.height());
    let r = (st.right as f32).min(sw - l).min(rect.width() - l);
    let b = (st.bottom as f32).min(sh - t).min(rect.height() - t);

    // Source and destination edges for the four cuts: the outside pair is the
    // image's own edge, the inside pair is the fixed inset.
    let sx = [0.0, l, sw - r, sw];
    let sy = [0.0, t, sh - b, sh];
    let dx = [rect.min.x, rect.min.x + l, rect.max.x - r, rect.max.x];
    let dy = [rect.min.y, rect.min.y + t, rect.max.y - b, rect.max.y];

    for row in 0..3 {
        for col in 0..3 {
            let w = dx[col + 1] - dx[col];
            let h = dy[row + 1] - dy[row];
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            // A cell's UV is its own slice of the source, normalised. Computed
            // from the edges rather than a width so the last cell cannot round
            // past 1.0 and wrap.
            let uv = egui::Rect::from_min_max(
                egui::pos2(sx[col] / sw, sy[row] / sh),
                egui::pos2(sx[col + 1] / sw, sy[row + 1] / sh),
            );
            painter.image(
                tex.id(),
                egui::Rect::from_min_max(
                    egui::pos2(dx[col], dy[row]),
                    egui::pos2(dx[col + 1], dy[row + 1]),
                ),
                uv,
                egui::Color32::WHITE,
            );
        }
    }
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
    /// `<theme_dir>/frames`, when the folder exists.
    ///
    /// **Its own field, not derived from `icons_dir`.** The first version walked
    /// up from the icons folder, so a theme that shipped frames and no icons got
    /// nothing — which is the whole point of a frame, since a theme whose art is
    /// 9-sliced bitmaps has no use for 24 per-icon SVGs and falls back to the
    /// default theme's. Icons fall back to another theme; frames never do, because
    /// a frame is a whole surface and borrowing another theme's would be a picture
    /// from a skin nobody chose.
    pub frames_dir: Option<PathBuf>,
    /// `<theme_dir>/faces`, when the folder exists. Its own field for the same
    /// reason as `frames_dir`: a theme shipping button art and no icon SVGs is
    /// the normal case for a face.
    pub faces_dir: Option<PathBuf>,
    /// What `frames` said, before any file was read.
    pub frames: Frames,
    /// What `faces` said, before any file was read.
    pub faces: Faces,
    /// The slider knobs, in the same shape as `faces` and keyed by `Knob`.
    pub knobs: Knobs,
    /// The digit strip, if the theme names one. A sibling of `faces` rather than
    /// one of its slots, because it addresses characters rather than icons.
    pub digits: Option<Digits>,
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
        // Every group is optional. An absent group parses as `Null`, and every
        // `from_json` reads through `num`/`tok`, which fall back per token. So a
        // `theme.json` written before these groups existed loads unchanged and
        // renders exactly as it did, which is what lets the bundled themes and any
        // hand-written one keep working with no edits.
        let group = |k: &str| v.get(k).cloned().unwrap_or(serde_json::Value::Null);
        let frames = Frames::from_json(&group("frames"));
        let faces = Faces::from_json(&group("faces"));
        let knobs = Knobs::from_json(&group("knobs"));
        let digits = {
            let d = group("digits");
            match (
                d.get("src").and_then(|x| x.as_str()),
                d.get("chars").and_then(|x| x.as_str()),
            ) {
                (Some(src), Some(chars)) => Some(Digits {
                    src: src.to_string(),
                    chars: chars.to_string(),
                }),
                _ => None,
            }
        };
        let icons_dir = dir.join("icons");
        let icons_dir = icons_dir.is_dir().then_some(icons_dir);
        let frames_dir = dir.join("frames");
        let frames_dir = frames_dir.is_dir().then_some(frames_dir);
        let faces_dir = dir.join("faces");
        let faces_dir = faces_dir.is_dir().then_some(faces_dir);
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
            frames,
            faces,
            knobs,
            digits,
            icons_dir,
            frames_dir,
            faces_dir,
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

/// A theme's 9-sliced artwork, decoded into textures.
///
/// Kept apart from the `Frames` token struct on purpose: that one is *what the
/// file said*, and this one is *what the GPU holds*. They are different shapes —
/// a theme may name a frame that is missing, and a missing frame is not an error
/// — and conflating them is how a decode failure turns into a paint failure.
#[derive(Clone, Default)]
pub struct DecodedFrames {
    pub titlebar: Option<(TextureHandle, Stretch)>,
    pub pane: Option<(TextureHandle, Stretch)>,
    pub eq: Option<(TextureHandle, Stretch)>,
}

/// One cell's rectangle inside a source image, in UV.
///
/// **The whole of a button's address, and pure.** Winamp's `CBUTTONS.BMP` is a
/// grid of small bitmaps, so a theme names the file, the grid, and which cells are
/// which state — and the arithmetic turning an index into a rectangle is the one
/// thing here that can be wrong without anything failing loudly. A face drawn
/// from a half-cell rect is a cropped button, which looks like an art problem.
#[derive(Clone)]
pub struct Cell {
    /// Source image. Every face in a theme may share one file, which is what
    /// makes a grid worth having.
    pub tex: TextureHandle,
    /// The cell, already in 0..1 UV space.
    pub uv: egui::Rect,
}

/// A themed button face: four source rectangles out of one grid.
///
/// **The four states are Winamp's, not egui's.** egui draws a button's face as a
/// `WidgetVisuals` fill, which a bitmap cannot be — so when a theme ships a face
/// the button is *not* an egui `Button` at all (see `face_button`) and these four
/// rectangles are the whole of its chrome. `lit` falls back to `pressed`, because
/// a mode toggle's "on" face is the pressed artwork and a theme that means it
/// differently can say so.
#[derive(Clone)]
pub struct Face {
    pub normal: Cell,
    pub pressed: Cell,
    pub disabled: Cell,
    pub lit: Cell,
}

impl std::fmt::Debug for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `TextureHandle` is neither `Copy` nor `Debug`, and a face test cares
        // about the UV it resolves to, not the handle it came from.
        f.debug_struct("Cell").field("uv", &self.uv).finish()
    }
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face")
            .field("normal", &self.normal)
            .field("pressed", &self.pressed)
            .field("disabled", &self.disabled)
            .field("lit", &self.lit)
            .finish()
    }
}

impl Face {
    /// The cell for an interaction state. `Ui`-free and `Painter`-free, so a test
    /// can ask which of the four a given state picks without a frame.
    ///
    /// Hover and press share the pressed artwork, which is Winamp's own choice:
    /// `CBUTTONS.BMP` carries normal and pressed rows and nothing between. A
    /// theme that wants a distinct hover says so by naming `pressed` as the
    /// pressed cell and accepting the share, not by getting a fourth.
    pub fn cell_for(&self, enabled: bool, hovered: bool, pressed: bool, lit: bool) -> &Cell {
        if !enabled {
            &self.disabled
        } else if lit {
            &self.lit
        } else if pressed || hovered {
            &self.pressed
        } else {
            &self.normal
        }
    }
}

/// Divide a source image into a grid and name the cells a button uses.
///
/// The four indices are optional and each falls back to `cell`, so the smallest
/// useful face is one line — a theme shipping a single static mark is the common
/// case and should not have to invent a pressed variant to say so.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FaceSlot {
    /// File name inside `<theme>/faces/`.
    pub src: String,
    /// `[columns, rows]`. Omit for a single cell that fills the image.
    #[serde(default)]
    pub grid: Option<[u32; 2]>,
    /// The resting face.
    #[serde(default)]
    pub cell: Option<u32>,
    /// The held-down face. Defaults to `cell`.
    #[serde(default)]
    pub pressed: Option<u32>,
    /// The greyed face. Defaults to `cell`.
    #[serde(default)]
    pub disabled: Option<u32>,
    /// The lit face for a mode toggle. Defaults to `pressed`.
    #[serde(default)]
    pub lit: Option<u32>,
}

/// Every per-element bitmap a theme ships, keyed by the `Icon` slot it replaces.
///
/// **The key is an `Icon` variant name, so a face is looked up exactly where an
/// icon texture is** — one call, and a theme that names six of them gets the other
/// eighteen from the icon set. A grid rather than one file per element is
/// Winamp's own shape: `CBUTTONS.BMP` holds the five transport marks in three
/// states each, so a theme ships one image and a table of indices.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Faces {
    /// Keyed by `Icon::face_key`. Public so a test can count what a theme named
    /// without reading the decoded `ThemeState` map, which only holds the ones
    /// that resolved.
    pub slots: BTreeMap<String, FaceSlot>,
}

/// Which slider a knob belongs to.
///
/// **One role per slider in the app, and no more.** `Eq` is one role serving ten
/// sliders: the bands are identical in size and shape, so a theme wanting ten
/// different knob bitmaps would need a per-band index — and no Winamp skin has one.
/// A role per slider means a theme can reach every knob in the player, which is
/// the whole point of the group.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Knob {
    Volume,
    Balance,
    Seek,
    Eq,
    Crossfade,
}

impl Knob {
    pub const ALL: [Knob; 5] = [
        Knob::Volume,
        Knob::Balance,
        Knob::Seek,
        Knob::Eq,
        Knob::Crossfade,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Knob::Volume => "volume",
            Knob::Balance => "balance",
            Knob::Seek => "seek",
            Knob::Eq => "eq",
            Knob::Crossfade => "crossfade",
        }
    }
}

/// A theme's slider knobs, in the same shape as `faces`.
///
/// A knob is a *face* by another name: one cell of a grid, four states. The only
/// difference is what it decorates and how it is looked up — by slider rather than
/// by icon — because a `Face` is a button's whole chrome and a knob is a handle's.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Knobs {
    pub slots: BTreeMap<String, FaceSlot>,
}

/// The `src`/`grid`/index table a `faces` or `knobs` group is, as a map.
///
/// **One reader for both.** The shape is identical and the fallback rules live in
/// the decoder below, so two copies of either would be two copies to keep in
/// step — and a knob table that disagreed with a face table would be the kind of
/// drift nothing else can see.
fn slot_table(v: &serde_json::Value) -> BTreeMap<String, FaceSlot> {
    let mut slots = BTreeMap::new();
    if let Some(map) = v.as_object() {
        for (k, f) in map {
            let Some(src) = f.get("src").and_then(|x| x.as_str()) else {
                continue;
            };
            let num = |key: &str| f.get(key).and_then(|x| x.as_u64()).map(|n| n as u32);
            let grid = f
                .get("grid")
                .and_then(|x| x.as_array())
                .and_then(|a| Some([a.first()?.as_u64()? as u32, a.get(1)?.as_u64()? as u32]));
            slots.insert(
                k.clone(),
                FaceSlot {
                    src: src.to_string(),
                    grid,
                    cell: num("cell"),
                    pressed: num("pressed"),
                    disabled: num("disabled"),
                    lit: num("lit"),
                },
            );
        }
    }
    slots
}

impl Faces {
    fn from_json(v: &serde_json::Value) -> Faces {
        Faces {
            slots: slot_table(v),
        }
    }
}

impl Knobs {
    fn from_json(v: &serde_json::Value) -> Knobs {
        Knobs {
            slots: slot_table(v),
        }
    }
}

/// The UV rectangle of grid cell `index`, or `None` when the index is outside the
/// grid.
///
/// `None` rather than a clamped rectangle on purpose: a theme whose `cell` points
/// past its `grid` gets **no face**, so the button falls back to the icon — which
/// is a legible mistake — rather than a silently cropped one, which is not.
pub fn cell_to_rect(size: [usize; 2], grid: Option<[u32; 2]>, index: u32) -> Option<egui::Rect> {
    let g = grid.unwrap_or([1, 1]);
    let (cols, rows) = (g[0], g[1]);
    if cols == 0 || rows == 0 {
        return None;
    }
    let (cols, rows) = (cols as usize, rows as usize);
    if (index as usize) >= cols * rows {
        return None;
    }
    let (w, h) = (size[0] as f32, size[1] as f32);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (cx, cy) = (index as usize % cols, index as usize / cols);
    let (cw, ch) = (w / cols as f32, h / rows as f32);
    Some(egui::Rect::from_min_max(
        egui::pos2(cx as f32 * cw / w, cy as f32 * ch / h),
        egui::pos2((cx + 1) as f32 * cw / w, (cy + 1) as f32 * ch / h),
    ))
}

impl Faces {
    /// Decode every named slot, and build a `Face` from each complete one.
    ///
    /// A slot missing its file, or naming an index outside its grid, is skipped
    /// rather than fatal: the button keeps the icon it had, which is what a theme
    /// that got the table slightly wrong should look like.
    /// Decode the digit strip, if the theme names one.
    fn load_digits(
        ctx: &egui::Context,
        theme: &Theme,
        d: &Digits,
    ) -> Option<(TextureHandle, String)> {
        let dir = theme.faces_dir.as_deref()?;
        let path = dir.join(&d.src);
        let img = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let fmt = image::ImageFormat::from_path(&path).map_err(|e| e.to_string())?;
                image::load_from_memory_with_format(&bytes, fmt)
                    .map(|i| i.into_rgba8())
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| {
                log::warn!("digit strip {} did not decode ({e})", path.display());
                e
            })
            .ok()?;
        let (w, h) = (img.width() as usize, img.height() as usize);
        let tex = ctx.load_texture(
            format!("tplay-digits-{}", theme.id),
            egui::ColorImage::from_rgba_premultiplied([w, h], img.as_raw()),
            egui::TextureOptions::NEAREST,
        );
        Some((tex, d.chars.clone()))
    }

    fn load(ctx: &egui::Context, theme: &Theme) -> BTreeMap<Icon, Face> {
        decode_slots(ctx, theme, &theme.faces.slots, "faces", Icon::from_face_key)
    }
}

/// Decode a `faces` or `knobs` table, resolving each key through `key_of`.
///
/// **One decoder for both, on purpose.** The fallback rules — each state defaults
/// to `cell`, `lit` to `pressed` — are small, and two copies of them are two
/// copies to keep in step. A theme's knob table cannot drift from its face table.
///
/// A slot missing its file, or naming an index outside its grid, is skipped rather
/// than fatal: the button keeps the icon it had and the slider keeps egui's
/// handle, which is what a theme that got the table slightly wrong should look like.
fn decode_slots<K: Ord>(
    ctx: &egui::Context,
    theme: &Theme,
    slots: &BTreeMap<String, FaceSlot>,
    what: &str,
    key_of: impl Fn(&str) -> Option<K>,
) -> BTreeMap<K, Face> {
    let mut out = BTreeMap::new();
    let Some(dir) = theme.faces_dir.as_deref() else {
        return out;
    };
    // One decode per file, however many slots name it — a grid is the whole
    // reason a theme can ship five transport marks in one image.
    let mut textures: BTreeMap<String, TextureHandle> = BTreeMap::new();
    for (key, slot) in slots {
        let Some(k) = key_of(key) else {
            log::warn!("{what}.{key} is not a slot this app knows; ignoring it");
            continue;
        };
        if !textures.contains_key(&slot.src) {
            let path = dir.join(&slot.src);
            match decode_bitmap(&path).map(|(w, h, px)| {
                ctx.load_texture(
                    format!("tplay-{what}-{}-{}", theme.id, slot.src),
                    egui::ColorImage::from_rgba_premultiplied([w, h], &px),
                    // Nearest by default, like every other bitmap a theme ships:
                    // this is 1x art on a HiDPI screen.
                    egui::TextureOptions::NEAREST,
                )
            }) {
                Ok(tex) => {
                    textures.insert(slot.src.clone(), tex);
                }
                Err(e) => {
                    log::warn!(
                        "{what} art {} did not decode ({e}); keeping the default",
                        path.display()
                    );
                    continue;
                }
            }
        }
        let tex = textures[&slot.src].clone();
        let cell = |i: u32| {
            cell_to_rect(tex.size(), slot.grid, i).map(|uv| Cell {
                tex: tex.clone(),
                uv,
            })
        };
        let Some(normal) = slot.cell.and_then(cell) else {
            log::warn!("{what}.{key} names no usable cell; keeping the default");
            continue;
        };
        out.insert(
            k,
            Face {
                pressed: slot
                    .pressed
                    .and_then(cell)
                    .unwrap_or_else(|| normal.clone()),
                disabled: slot
                    .disabled
                    .and_then(cell)
                    .unwrap_or_else(|| normal.clone()),
                lit: slot
                    .lit
                    .and_then(cell)
                    .or_else(|| slot.pressed.and_then(cell))
                    .unwrap_or_else(|| normal.clone()),
                normal,
            },
        );
    }
    out
}

impl Knobs {
    fn load(ctx: &egui::Context, theme: &Theme) -> BTreeMap<Knob, Face> {
        decode_slots(ctx, theme, &theme.knobs.slots, "knobs", |k| {
            Knob::ALL.into_iter().find(|n| n.key() == k)
        })
    }
}

/// Read a bitmap as RGBA bytes with its size.
///
/// Format from the extension rather than sniffed: a theme author chose `.bmp` or
/// `.png` deliberately, and a sniff would silently accept a mislabelled file
/// instead of naming the mismatch.
fn decode_bitmap(path: &Path) -> Result<(usize, usize, Vec<u8>), image::ImageError> {
    let fmt = image::ImageFormat::from_path(path)?;
    let img = image::load_from_memory_with_format(
        &std::fs::read(path).map_err(|_| {
            image::ImageError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path.display().to_string(),
            ))
        })?,
        fmt,
    )?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    Ok((w, h, img.into_rgba8().into_raw()))
}

/// The characters a digit strip carries, in cell order.
///
/// **A string rather than a table of indices**, because it is the same thing and
/// it is self-describing: `"0123456789: "` says what the strip holds without a
/// reader counting cells, and a theme with no colon simply omits the character.
/// A character the string does not name has no cell, which is what makes
/// [`digits`] able to decline a string instead of drawing a wrong glyph.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Digits {
    /// File name inside `<theme>/faces/`.
    pub src: String,
    /// The strip's cells, left to right.
    pub chars: String,
}

/// Width `text` occupies in a digit strip, at height `h`.
///
/// **Separate from the painter, because the seek bar's width is computed from it.**
/// A layout that measured with egui's font and painted with a bitmap would size
/// the bar to one number and fill it with another, and the mismatch is only
/// visible with a skin loaded — the same "measure it once" rule the EQ pane's
/// slider budget follows.
pub fn digit_width(tex: &TextureHandle, chars: &str, text: &str, h: f32) -> Option<f32> {
    let n = chars.chars().count();
    if n == 0 || h <= 0.0 {
        return None;
    }
    let [w, th] = tex.size();
    if w == 0 || th == 0 {
        return None;
    }
    let cell_w = w as f32 / n as f32;
    let cell_h = th as f32;
    let adv = h * cell_w / cell_h;
    text.chars()
        .map(|c| chars.find(c).map(|_| adv))
        .collect::<Option<Vec<f32>>>()
        .map(|v| v.iter().sum())
}

/// A label drawn as a row of bitmap digits, clickable like any other.
///
/// The elapsed-time label toggles remaining mode on a click, so it cannot be
/// inert art — and that is why this is a widget and not a `painter` call. It
/// returns `None` when the theme ships no strip or when the text has a character
/// the strip lacks, and the caller then draws the ordinary label, so a skin
/// without digits and a time string with a colon both land somewhere legible.
pub fn digits_button(
    ui: &mut egui::Ui,
    digits: Option<(&TextureHandle, &str)>,
    text: &str,
    height: f32,
) -> Option<egui::Response> {
    let (tex, chars) = digits?;
    let w = digit_width(tex, chars, text, height)?;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, height), egui::Sense::hover());
    let resp = ui.interact(rect, ui.next_auto_id(), egui::Sense::click());
    let n = chars.chars().count();
    let mut x = rect.min.x;
    for c in text.chars() {
        let idx = chars.find(c)? as u32;
        let uv = cell_to_rect(tex.size(), Some([n as u32, 1]), idx)?;
        let cell = rect.height() * (uv.width() / uv.height());
        ui.painter().image(
            tex.id(),
            egui::Rect::from_min_max(egui::pos2(x, rect.min.y), egui::pos2(x + cell, rect.max.y)),
            uv,
            Color32::WHITE,
        );
        x += cell;
    }
    Some(resp)
}

/// The size egui gives a slider that carries nothing but the slider.
///
/// **A mirror of two private rules**, and both are made of public API: `Slider`
/// asks for `spacing().slider_width` across its travel axis, and across the other
/// one for `text_style_height(Body)` floored at `spacing().interact_size.y`. The
/// app already sets `slider_width` per slider for layout reasons (the seek bar
/// spans its row, an EQ band is `eq_band_w_max` tall), so reading it back is the
/// same number egui used rather than a second guess at it.
///
/// Needed because **a `Slider`'s `Response.rect` is not the slider.** egui wraps
/// the widget in a `horizontal`/`vertical` layout and unions the two responses, so
/// the rect that comes back spans the whole available row or column. `knob`
/// captures the cursor instead, which is the slider's own top-left.
pub fn slider_size(ui: &egui::Ui, vertical: bool) -> egui::Vec2 {
    let cross = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.spacing().interact_size.y);
    if vertical {
        egui::vec2(cross, ui.spacing().slider_width)
    } else {
        egui::vec2(ui.spacing().slider_width, cross)
    }
}

/// Where egui put a slider's handle, for a value already normalised to 0..=1.
///
/// **Also a mirror of egui's private geometry, and it is pinned rather than
/// hopeful.** `Slider::position_range` shrinks the slider rect by `handle_radius`
/// and lerps along what is left, and `marker_center` drops the handle onto the
/// rail's centre line — none of which egui hands out, so a bitmap knob has no
/// other way to line up with the widget it decorates. The alternative is
/// reimplementing `egui::Slider` to own the geometry, which costs the value
/// readout, keyboard handling and every layout behaviour the app gets for free.
///
/// The risk a mirror carries is drift, so the drift is made into a test:
/// `a_knob_lands_on_eguis_own_handle` renders real sliders, finds the shape egui
/// painted for its handle, and asserts this agrees. If egui changes its formula
/// that test fails and names this function — instead of every knob in the app
/// sitting a few pixels off its own rail with nothing to say so.
///
/// `aspect` is `visuals.handle_shape`'s, and a `Rect` handle multiplies its radius
/// by it exactly as egui does. `0.0` means a circular handle.
pub fn knob_center(rect: egui::Rect, value01: f32, vertical: bool, aspect: f32) -> egui::Pos2 {
    let t = value01.clamp(0.0, 1.0);
    let cross = if vertical {
        rect.width()
    } else {
        rect.height()
    } / 2.5;
    let radius = if aspect > 0.0 { cross * aspect } else { cross };
    if vertical {
        // egui: `y_range().shrink(r).flip()`, so the largest value is the lowest y.
        let hi = rect.max.y - radius;
        let lo = rect.min.y + radius;
        egui::pos2(rect.center().x, hi + (lo - hi) * t)
    } else {
        let lo = rect.min.x + radius;
        let hi = rect.max.x - radius;
        egui::pos2(lo + (hi - lo) * t, rect.center().y)
    }
}

/// Add a slider, then paint a knob over its handle if the theme ships one.
///
/// **Over, not instead of.** egui has already painted its handle by the time
/// `Slider` returns, and suppressing it means either killing the rail as well —
/// the rail is `widgets.inactive.bg_fill` and the handle is `interact(..).bg_fill`,
/// so one clear hits both — or a per-widget `Style` clone with `bg_fill` and
/// `fg_stroke` cleared on three interaction states. Painting over needs neither, and
/// it means a theme with no knob, or a knob that failed to load, still gets egui's
/// handle rather than no handle at all.
///
/// `value01` is the value already normalised, because the four roles have four
/// different ranges and the knob only ever needs a position. `resp` is the slider's
/// own response, so the pressed and disabled cells come from this frame's
/// interaction rather than last frame's guess.
pub fn slider(
    ui: &mut egui::Ui,
    slider: egui::Slider<'_>,
    value01: f32,
    vertical: bool,
    knob: Option<&Face>,
) -> egui::Response {
    let at = ui.cursor().min;
    let size = slider_size(ui, vertical);
    let resp = ui.add(slider);
    paint_knob(
        ui,
        egui::Rect::from_min_size(at, size),
        &resp,
        value01,
        vertical,
        knob,
        None,
    );
    resp
}

/// Paint one knob cell centred on a slider's handle.
///
/// `size` is the drawn size in points; `None` uses the handle's own box, so a knob
/// with no explicit size sits exactly over what it covers.
pub fn paint_knob(
    ui: &egui::Ui,
    rect: egui::Rect,
    resp: &egui::Response,
    value01: f32,
    vertical: bool,
    face: Option<&Face>,
    size: Option<egui::Vec2>,
) {
    let Some(face) = face else { return };
    let aspect = match ui.visuals().handle_shape {
        egui::style::HandleShape::Circle => 0.0,
        egui::style::HandleShape::Rect { aspect_ratio } => aspect_ratio,
    };
    let d = (if vertical {
        rect.width()
    } else {
        rect.height()
    } / 2.5)
        * 2.0;
    // egui draws a Rect handle as `Vec2::new(r*aspect, r)` on its side and
    // `Vec2::new(r, r*aspect)` upright, doubled for the rect's extent — so the
    // aspect lands on the **travel** axis in both cases, and the knob has to put
    // it on the same one or it covers half of egui's handle and none of the rest.
    let on_travel = d * if aspect > 0.0 { aspect } else { 1.0 };
    let default = if vertical {
        egui::vec2(d, on_travel)
    } else {
        egui::vec2(on_travel, d)
    };
    let cell = face.cell_for(
        resp.enabled(),
        resp.hovered(),
        resp.is_pointer_button_down_on(),
        false,
    );
    let center = knob_center(rect, value01, vertical, aspect);
    ui.painter().image(
        cell.tex.id(),
        egui::Rect::from_center_size(center, size.unwrap_or(default)),
        cell.uv,
        Color32::WHITE,
    );
}

/// Which of the theme's two frame surfaces a caller wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    /// The window's title bar, above the dock.
    Titlebar,
    /// A docked pane's body, behind its content.
    Pane,
    /// The Equalizer's own backdrop, behind its controls rather than around them.
    ///
    /// Its own slot because the EQ is the one pane whose *background* is artwork
    /// in a Winamp skin — `EQMAIN.BMP` — while the playlist and library share
    /// the window frame.
    Eq,
}

impl std::fmt::Debug for DecodedFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Theme` derives Debug and this is a field of it, but `TextureHandle`
        // does not implement Debug and the handle's identity is its texture id.
        // What a reader wants here is which frames loaded, not their ids.
        f.debug_struct("DecodedFrames")
            .field("titlebar", &self.titlebar.is_some())
            .field("pane", &self.pane.is_some())
            .field("eq", &self.eq.is_some())
            .finish()
    }
}

impl DecodedFrames {
    /// Decode both surfaces from `<theme>/frames/`.
    ///
    /// A missing file is not a failure: a theme with no frames is the normal case
    /// and the pane just paints its own surface colour. A file that exists but
    /// does not decode says so, because that is a theme author's mistake rather
    /// than a theme that simply does not use frames.
    fn load(ctx: &egui::Context, theme: &Theme) -> DecodedFrames {
        let Some(dir) = theme.frames_dir.as_deref() else {
            return DecodedFrames::default();
        };
        let one = |f: &Option<Frame>, name: &str| -> Option<(TextureHandle, Stretch)> {
            let f = f.as_ref()?;
            let path = dir.join(&f.src);
            let Ok(bytes) = std::fs::read(&path) else {
                return None;
            };
            let opts = if f.smooth {
                egui::TextureOptions::LINEAR
            } else {
                egui::TextureOptions::NEAREST
            };
            // Format from the extension rather than sniffed: a theme author
            // chose `.bmp` or `.png` deliberately, and the sniff would silently
            // accept a mislabelled file instead of naming the mismatch.
            let Ok(fmt) = image::ImageFormat::from_path(&path) else {
                log::warn!(
                    "theme frame {} has no format the decoder knows; drawing without it",
                    path.display()
                );
                return None;
            };
            match image::load_from_memory_with_format(&bytes, fmt) {
                Ok(img) => {
                    let img = img.into_rgba8();
                    let (w, h) = (img.width() as usize, img.height() as usize);
                    let tex = ctx.load_texture(
                        format!("tplay-frame-{}-{}", theme.id, name),
                        egui::ColorImage::from_rgba_premultiplied([w, h], img.as_raw()),
                        opts,
                    );
                    Some((tex, f.stretch))
                }
                Err(e) => {
                    log::warn!(
                        "theme frame {} did not decode ({e}); drawing without it",
                        path.display()
                    );
                    None
                }
            }
        };
        DecodedFrames {
            titlebar: one(&theme.frames.titlebar, "titlebar"),
            pane: one(&theme.frames.pane, "pane"),
            eq: one(&theme.frames.eq, "eq"),
        }
    }
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
    frames: DecodedFrames,
    faces: BTreeMap<Icon, Face>,
    knobs: BTreeMap<Knob, Face>,
    digits: Option<(TextureHandle, String)>,
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
        let frames = DecodedFrames::load(ctx, &current);
        let faces = Faces::load(ctx, &current);
        let knobs = Knobs::load(ctx, &current);
        let digits = current
            .digits
            .as_ref()
            .and_then(|d| Faces::load_digits(ctx, &current, d));
        Self {
            current,
            themes,
            icons,
            frames,
            faces,
            knobs,
            digits,
        }
    }

    /// The theme's knob for a slider role, or `None` to keep egui's handle.
    pub fn knob(&self, which: Knob) -> Option<&Face> {
        self.knobs.get(&which)
    }

    /// The theme's digit strip and its cell order, or `None` for a text label.
    pub fn digits(&self) -> Option<(&TextureHandle, &str)> {
        self.digits.as_ref().map(|(t, c)| (t, c.as_str()))
    }

    /// The theme's bitmap face for a slot, or `None` for the icon it replaces.
    ///
    /// **No fallback to the default theme, and that is deliberate.** Icons fall
    /// back, because a missing mark is better drawn as egui's version than as a
    /// glyph. A face does not: it is a button's whole chrome, so a borrowed one
    /// would be a button in another skin's clothes, sitting next to this theme's
    /// own faces.
    pub fn face(&self, icon: Icon) -> Option<&Face> {
        self.faces.get(&icon)
    }

    /// Both halves of a slot in one lookup, for the button helpers.
    pub fn slot(&self, icon: Icon) -> Slot {
        Slot {
            face: self.face(icon).cloned(),
            icon: self.icon(icon).cloned(),
        }
    }

    /// The theme's 9-sliced artwork, for `nine_slice`. `None` for a surface the
    /// theme does not draw, which is every surface for a theme with no frames.
    pub fn frame(&self, which: Surface) -> Option<(&TextureHandle, Stretch)> {
        let slot = match which {
            Surface::Titlebar => &self.frames.titlebar,
            Surface::Pane => &self.frames.pane,
            Surface::Eq => &self.frames.eq,
        };
        slot.as_ref().map(|(t, s)| (t, *s))
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
        self.frames = DecodedFrames::load(ctx, &self.current);
        self.faces = Faces::load(ctx, &self.current);
        self.knobs = Knobs::load(ctx, &self.current);
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
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

    /// The `faces` token this slot is looked up by — the icon's file stem, so
    /// `play.svg` and a face named `play` are the same thing.
    ///
    /// Deriving it from `DATA` rather than a second table is the point: a key
    /// that could disagree with the icon it names is a key that eventually does.
    pub fn face_key(self) -> &'static str {
        Self::DATA[self.index()]
            .0
            .strip_suffix(".svg")
            .expect("every icon file ends in .svg")
    }

    /// The inverse of [`Icon::face_key`]. `None` for a name that is not an icon
    /// slot, so a typo in a theme's `faces` table is reported rather than drawn.
    pub fn from_face_key(key: &str) -> Option<Icon> {
        Icon::ALL.into_iter().find(|i| i.face_key() == key)
    }
}

/// What to draw for one icon slot: the theme's bitmap face if it has one, and
/// its icon texture either way.
///
/// **Owned rather than borrowed, and the reason is a borrow.** The ☰ menu closure
/// needs `&mut ThemeState` to switch themes, so a `Slot` that lent out
/// `&ThemeState` would conflict with the very lookup the window buttons need.
/// Cloning is four refcount bumps per call — a `Face` is four `TextureHandle`s —
/// which is nothing next to re-decoding them, and it leaves the coordinator's
/// existing hoist-the-textures pattern untouched.
#[derive(Clone, Default)]
pub struct Slot {
    pub face: Option<Face>,
    pub icon: Option<TextureHandle>,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot")
            .field("face", &self.face)
            .field("icon", &self.icon.is_some())
            .finish()
    }
}

/// A button whose face is a bitmap cell rather than an egui widget.
///
/// **This is not an egui `Button` on purpose, and that is what makes it cheap.**
/// egui paints a button's face as a `WidgetVisuals` fill, so a bitmap under one
/// would have to be drawn *through* it — `bg_fill` and `weak_bg_fill` suppressed
/// per widget, on a cloned `Style` (`Ui::style_mut` is clone-on-write), or the
/// hover fill covering the art. Owning the whole widget means none of that: the
/// face is drawn, the click is taken, and nothing else paints over it.
///
/// Hover needs no frame of latency either, because the state is read from this
/// frame's own `interact` rather than inferred from last frame's `Response`.
pub fn face_button(
    ui: &mut egui::Ui,
    face: &Face,
    size: f32,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    // A disabled button gets `Sense::hover`, so `clicked()` is false by
    // construction rather than by a guard the call sites would each need.
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.interact(rect, ui.next_auto_id(), sense);
    let cell = face.cell_for(
        enabled,
        resp.hovered(),
        resp.is_pointer_button_down_on(),
        selected,
    );
    ui.painter()
        .image(cell.tex.id(), rect, cell.uv, Color32::WHITE);
    resp
}

/// Transport/action button: the theme's bitmap face for the slot if it ships one,
/// its icon texture if not, and a glyph if the icon failed to load. `selected`
/// paints egui's selected visuals (theme `--progress-fill`/`--focus-ring`) for
/// mode toggles — or selects the face's `lit` cell.
pub fn icon_button(
    ui: &mut egui::Ui,
    slot: Slot,
    icon: Icon,
    size: f32,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    if let Some(face) = &slot.face {
        return face_button(ui, face, size, enabled, selected);
    }
    let button = match &slot.icon {
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
    slot: Slot,
    icon: Icon,
    size: f32,
    on: bool,
) -> egui::Response {
    icon_button(ui, slot, icon, size, true, on)
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
