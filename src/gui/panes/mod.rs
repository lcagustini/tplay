//! GUI panes — each is a free function taking (app, ui).

use crate::app::TPlayApp;
use crate::gui::theme;
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

/// The "To iPod" button and the line that reports what it did. Reports whether it
/// was clicked; the caller collects the tracks and arms the confirm.
///
/// **The caller builds the track list, not this.** Both call sites would
/// otherwise walk their whole list on every frame to hand a `Vec<PathBuf>` to a
/// dialog that may never open — the defect `file_list_ui` already had once, and
/// whose symptom was the whole application feeling slow rather than one pane.
///
/// A text label rather than an icon, like "Add All" beside it: a new icon is
/// four theme files, a recolour row and an `Icon::DATA` append, and this control
/// is one more word in a row of words.
/// Which way the button runs.
///
/// One button whose label follows the pane, rather than two buttons of which one
/// is always greyed out: the Library is the one pane that can be browsing *either*
/// side of the transfer, and a permanently-disabled button next to an enabled
/// one reads as a broken app rather than as an inapplicable action.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpodDirection {
    /// The folder being shown is on a connected iPod: take its tracks off.
    Remove,
    /// The folder is local: put its tracks on.
    Add,
}

pub(crate) fn ipod_button(
    ui: &mut egui::Ui,
    app: &TPlayApp,
    p: theme::Palette,
    direction: IpodDirection,
) -> bool {
    let job = app.ipod();
    let (idle, busy, hint) = match direction {
        IpodDirection::Remove => (
            "From iPod",
            "Taking off the iPod",
            "Take this folder's tracks off the connected iPod",
        ),
        IpodDirection::Add => (
            "To iPod",
            "Copying to iPod",
            "Copy this to a connected iPod",
        ),
    };
    let clicked = if job.busy() {
        ui.add_enabled(false, egui::Button::new(busy))
            .on_hover_text("Wait for the current copy to finish")
            .clicked()
    } else {
        ui.button(idle).on_hover_text(hint).clicked()
    };
    if let Some(outcome) = job.result() {
        // The palette has no error colour, so a failure speaks in its own words
        // rather than in a shade. The success line sits in the secondary tone,
        // which is what the counts beside it use.
        let (text, detail) = match outcome {
            Ok(done) => (done.describe(), done.detail()),
            Err(e) => (e.clone(), e.clone()),
        };
        ui.add(
            egui::Label::new(egui::RichText::new(text).small().color(p.text_secondary)).truncate(),
        )
        .on_hover_text(detail);
    }
    clicked
}
