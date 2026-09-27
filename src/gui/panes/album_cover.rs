//! Album Cover pane — the playing track's art (embedded tag picture, else
//! `folder.jpg`/`cover.jpg` beside the file), letterboxed into the pane. The
//! decoded texture is cached in egui memory keyed by the track's path, so an
//! artless track reads nothing until the track changes. No art at all → the
//! theme's `nocover` placeholder icon.

use crate::app::TPlayApp;
use crate::gui::theme::{Icon, ThemeState};
use crate::tracks;
use eframe::egui;

/// egui Id for the decoded cover cache: `(path, Option<TextureHandle>)` — the
/// `None` half caches "checked, no art" so artless tracks don't re-scan.
fn cover_id() -> egui::Id {
    egui::Id::new("tplay.cover")
}

/// Cache value: the path the texture belongs to, plus the texture.
type CoverCache = (String, Option<egui::TextureHandle>);

pub fn album_cover_pane(app: &mut TPlayApp, themes: &ThemeState, ui: &mut egui::Ui) {
    let theme = themes.current().clone();
    let p = theme.palette;

    let rect = ui.available_rect_before_wrap();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, p.bg);

    let ctx = ui.ctx().clone();
    let cur = app.current_path().map(|p| p.to_path_buf());
    let cur_key = cur
        .as_ref()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Re-resolve only when the playing track changed (first frame included).
    let cached: Option<CoverCache> = ctx.data(|d| d.get_temp(cover_id()));
    if !cached.as_ref().is_some_and(|(k, _)| k == &cur_key) {
        // `tracks::cover` resolves internally, so this pane never learns that a
        // remote track's art lives in a spool-cache copy rather than at its URI.
        // The texture cache stays keyed by the track id either way. By the time a
        // remote track is the *current* one it has necessarily been spooled
        // (playback uses the same cache), so `None` here is not a state worth
        // re-resolving for later.
        //
        // Only the embedded picture is available this way. `read_cover`'s
        // sibling-file fallback (`folder.jpg`/`cover.jpg` beside the track) looks
        // beside the *cache* copy, where nothing lives, so folder art on a share
        // is missed — fetching it would mean a second SMB transfer with its own
        // event plumbing, for a rarer case than embedded art.
        let art = cur.as_deref().and_then(tracks::cover);
        let tex = art.and_then(|bytes| {
            image::load_from_memory(&bytes).ok().map(|img| {
                let rgba = img.to_rgba8();
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    &rgba,
                );
                ctx.load_texture("tplay-cover", color, egui::TextureOptions::LINEAR)
            })
        });
        ctx.data_mut(|d| d.insert_temp(cover_id(), (cur_key, tex)));
    }
    let tex: Option<egui::TextureHandle> = ctx
        .data(|d| d.get_temp::<CoverCache>(cover_id()))
        .and_then(|(_, t)| t);

    match tex {
        Some(tex) => {
            // Letterbox: aspect kept, centered, full pane otherwise.
            let size = tex.size_vec2();
            if size.x > 0.0 && size.y > 0.0 {
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                let draw = egui::Rect::from_center_size(rect.center(), size * scale);
                painter.image(
                    tex.id(),
                    draw,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
        None => {
            // No art: themed placeholder, centered at a pane-relative size.
            let size = rect.width().min(rect.height()) * 0.35;
            let center = rect.center();
            match themes.icon(Icon::NoCover) {
                Some(icon) => {
                    painter.image(
                        icon.id(),
                        egui::Rect::from_center_size(center, egui::vec2(size, size)),
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                None => {
                    painter.text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        Icon::NoCover.glyph(),
                        egui::FontId::proportional(size),
                        p.text_secondary,
                    );
                }
            }
        }
    }
}
