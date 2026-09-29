//! GUI panes — each is a free function taking (app, ui).

use eframe::egui;

pub mod album_cover;
pub mod equalizer;
pub mod library;
pub mod now_playing;
pub mod playlist;
pub mod visualizer;

/// Whether the row at the cursor is inside a scroll area's clip.
///
/// One definition for two lists, because the two are the same rule and a copy is
/// a rule that can be fixed in one place and not the other — and the symptom of
/// getting it wrong is *silent in the other direction too*: a row drawn outside
/// the clip is invisible, so the only visible failure is the one where a row that
/// should be there is not.
///
/// A `Ui`'s `clip_rect` is what the viewport will actually be drawn into, and it
/// is readable before the row is built, so this is one comparison rather than a
/// pass over the list — and it needs no index arithmetic that has to stay in step
/// with the filters, which is what a `show_rows`-style range would need.
///
/// **A row of slack either side**, so a partly visible row still draws: one row
/// too many costs nothing next to a row that should be there and is not. The
/// caller still has to reserve the row's height with `ui.allocate_space`, because
/// reserve nothing and the scrollbar collapses to the visible slice.
pub(crate) fn row_visible(ui: &egui::Ui, row_h: f32) -> bool {
    let clip = ui.clip_rect();
    let top = ui.cursor().max.y;
    top + row_h >= clip.min.y - row_h && top <= clip.max.y + row_h
}
