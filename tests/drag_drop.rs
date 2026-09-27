//! Playlist drag-and-drop — the index math in `TPlayApp::move_track` /
//! `remove_track`, mirrored branch-for-branch so every adjustment path is
//! pinned. (The drag UI state machine itself lives in egui memory and is
//! exercised manually; the math it drives is the part worth locking down.)

/// Mirror of `TPlayApp::remove_track`'s current_index adjustment.
fn apply_remove(index: usize, current: Option<usize>) -> Option<usize> {
    current.and_then(|ci| {
        if ci == index {
            None
        } else if ci > index {
            Some(ci - 1)
        } else {
            Some(ci)
        }
    })
}

/// Mirror of `TPlayApp::move_track`'s current_index adjustment.
fn apply_move(from: usize, to: usize, current: Option<usize>) -> Option<usize> {
    current.map(|ci| {
        if ci == from {
            to
        } else if from < ci && ci <= to {
            ci - 1
        } else if to <= ci && ci < from {
            ci + 1
        } else {
            ci
        }
    })
}

#[test]
fn move_is_noop_when_from_equals_to() {
    assert_eq!(apply_move(2, 2, Some(2)), Some(2));
    assert_eq!(apply_move(0, 0, Some(3)), Some(3));
    assert_eq!(apply_move(1, 1, None), None);
}

#[test]
fn move_current_track_follows_to_target() {
    // Forward drag: current at from → lands on to
    assert_eq!(apply_move(2, 4, Some(2)), Some(4));
    // Backward drag
    assert_eq!(apply_move(4, 1, Some(4)), Some(1));
}

#[test]
fn moving_earlier_item_past_current_shifts_index_down() {
    // Drag 1 over 4 while 3 is playing: item 2..=4 slide down one.
    assert_eq!(apply_move(1, 4, Some(3)), Some(2));
    assert_eq!(apply_move(1, 4, Some(4)), Some(3));
    // Item at the destination itself also slides down.
    assert_eq!(apply_move(0, 3, Some(3)), Some(2));
}

#[test]
fn moving_later_item_before_current_shifts_index_up() {
    // Drag 4 before 1 while 3 is playing: items 1..=3 slide up one.
    assert_eq!(apply_move(4, 1, Some(3)), Some(4));
    assert_eq!(apply_move(4, 1, Some(1)), Some(2));
}

#[test]
fn moving_unrelated_item_keeps_index() {
    // Both dragged and current are entirely before the target…
    assert_eq!(apply_move(0, 1, Some(3)), Some(3));
    // …or entirely after it.
    assert_eq!(apply_move(4, 5, Some(3)), Some(3));
    assert_eq!(apply_move(3, 5, Some(1)), Some(1));
}

#[test]
fn remove_current_track_stops_playback() {
    assert_eq!(apply_remove(3, Some(3)), None);
    assert_eq!(apply_remove(0, Some(0)), None);
}

#[test]
fn remove_before_current_decrements() {
    assert_eq!(apply_remove(1, Some(3)), Some(2));
    assert_eq!(apply_remove(0, Some(1)), Some(0));
}

#[test]
fn remove_after_current_keeps_index() {
    assert_eq!(apply_remove(5, Some(3)), Some(3));
    assert_eq!(apply_remove(3, Some(0)), Some(0));
}

#[test]
fn remove_with_no_current_stays_none() {
    assert_eq!(apply_remove(2, None), None);
}

#[test]
fn remove_last_track_leaves_nothing_playing() {
    // Removing the only/current track empties both list position and current.
    assert_eq!(apply_remove(0, Some(0)), None);
}
