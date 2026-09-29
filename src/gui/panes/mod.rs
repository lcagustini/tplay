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
///
/// **The row's position is `available_rect_before_wrap().min.y`, and it must not
/// be `cursor().max.y`.** That was the bug that emptied the Library and the
/// Playlist at once: inside a `ScrollArea::show` the content `Ui`'s cursor is
/// `Pos2::INF` in its *min* corner — that is what "nothing placed yet" looks like
/// — so `cursor().max.y` is `inf` too, the second comparison is false for every
/// row, and both panes drew their chrome, counted their rows correctly, and
/// painted nothing. `available_rect_before_wrap()` reports the same position
/// finitely, because egui computes it from the placer's own rect rather than
/// from a cursor that has not been set yet.
///
/// Two things made it invisible, and both are worth remembering: the counts line
/// read non-zero, so the *listing* was provably fine and the eye went looking
/// for a data bug; and the culled path still reserves each row's height, so
/// `min_rect()` stayed full and the existing pane test — which asserts
/// `min_rect()` — passed with the bug present. A harness built on
/// `allocate_new_ui` also passes, because that gives a finite cursor; only
/// driving the real pane through a real `ScrollArea` reaches it. Guarded by
/// `both_lists_emit_the_rows_that_fit` in `tests/gui_tests.rs`, which is
/// mutation-checked on both halves: break the gate and it fails, delete the gate
/// and it fails.
pub(crate) fn row_visible(ui: &egui::Ui, row_h: f32) -> bool {
    let clip = ui.clip_rect();
    let top = ui.available_rect_before_wrap().min.y;
    top + row_h >= clip.min.y - row_h && top <= clip.max.y + row_h
}
