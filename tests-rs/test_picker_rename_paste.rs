// A Ctrl+V into the picker's `$` rename dialog must insert the clipboard once,
// and an intentional paste must always go in.
//
// On Windows crossterm delivers one Ctrl+V as an Event::Paste AND as the
// per-character key events of the same text, in either order. Paste first: the
// Event::Paste arm puts the text into the rename field and opens the 200 ms
// duplicate-paste window that every other overlay's character arm honours
// (issue #290), so the key events that follow are not typed again. Characters
// first (the ordering upstream d828af2 handles for the pane): the field keeps
// its own record of the Ctrl+V gesture, opened by the Press and closed by the
// Release, and a late Event::Paste of a gesture that already delivered its
// characters is dropped. Ordinary typing and edits open no gesture, so they
// never suppress a paste; a text/time rule did (typing `beta` then pasting
// `beta` kept one, and typing `a`, Backspace, pasting `a` left the field empty).
//
// These tests drive the field's own functions with the events in the order the
// dialog receives them; they start no server (see AGENTS.md).

use super::*;
use std::time::{Duration, Instant};

#[test]
fn a_character_inside_the_paste_window_is_not_typed_again() {
    assert!(
        !picker_rename_accepts_char(KeyModifiers::NONE, true),
        "the Event::Paste already put this text into the field"
    );
}

#[test]
fn an_ordinary_character_is_typed() {
    assert!(picker_rename_accepts_char(KeyModifiers::NONE, false));
    assert!(picker_rename_accepts_char(KeyModifiers::SHIFT, false));
}

#[test]
fn a_control_chord_is_never_typed() {
    assert!(!picker_rename_accepts_char(KeyModifiers::CONTROL, false));
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Deliver `text` as key events 1 ms apart from `start`, through the same key
/// predicate and paste window the rename field uses. Returns the time after it.
fn keys(
    field: &mut String,
    gesture: &mut PickerRenameGesture,
    text: &str,
    start: Instant,
    paste_window_until: Option<Instant>,
) -> Instant {
    let mut now = start;
    for c in text.chars() {
        if picker_rename_accepts_char(KeyModifiers::NONE, within_paste_suppress_window(paste_window_until, now)) {
            picker_rename_take_char(field, gesture, c);
        }
        now += ms(1);
    }
    now
}

#[test]
fn characters_then_a_late_event_paste_leave_the_name_once() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    gesture.start(); // Ctrl+V Press
    keys(&mut field, &mut gesture, "beta", t0, None);
    let taken = picker_rename_take_paste(&mut field, &mut gesture, "beta");
    gesture.finish(); // Ctrl+V Release
    assert_eq!(field, "beta");
    assert!(!taken);
}

#[test]
fn an_event_paste_then_its_characters_leave_the_name_once() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    gesture.start();
    assert!(picker_rename_take_paste(&mut field, &mut gesture, "beta"));
    // The Event::Paste arm opens the 200 ms duplicate-paste window (#290).
    keys(&mut field, &mut gesture, "beta", t0 + ms(5), Some(t0 + ms(200)));
    gesture.finish();
    assert_eq!(field, "beta");
}

#[test]
fn a_gesture_after_other_typing_keeps_the_typing() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    keys(&mut field, &mut gesture, "x", t0, None);
    gesture.start();
    keys(&mut field, &mut gesture, "beta", t0 + ms(20), None);
    let taken = picker_rename_take_paste(&mut field, &mut gesture, "beta");
    gesture.finish();
    assert_eq!(field, "xbeta");
    assert!(!taken);
}

#[test]
fn typing_a_name_then_pasting_it_keeps_both() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    keys(&mut field, &mut gesture, "beta", t0, None);
    // A paste with no Ctrl+V behind it (a terminal's bracketed paste).
    let taken = picker_rename_take_paste(&mut field, &mut gesture, "beta");
    assert_eq!(field, "betabeta");
    assert!(taken);
}

#[test]
fn a_paste_after_backspace_is_taken() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    keys(&mut field, &mut gesture, "a", t0, None);
    picker_rename_backspace(&mut field, &mut gesture);
    let taken = picker_rename_take_paste(&mut field, &mut gesture, "a");
    assert_eq!(field, "a");
    assert!(taken);
}

#[test]
fn backspace_inside_a_gesture_ends_it() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    gesture.start();
    keys(&mut field, &mut gesture, "ab", t0, None);
    picker_rename_backspace(&mut field, &mut gesture);
    let taken = picker_rename_take_paste(&mut field, &mut gesture, "cd");
    assert_eq!(field, "acd");
    assert!(taken);
}

#[test]
fn a_second_ctrl_v_pastes_again() {
    let t0 = Instant::now();
    let (mut field, mut gesture) = (String::new(), PickerRenameGesture::default());
    gesture.start();
    keys(&mut field, &mut gesture, "beta", t0, None);
    gesture.finish();
    gesture.start();
    assert!(picker_rename_take_paste(&mut field, &mut gesture, "beta"));
    gesture.finish();
    assert_eq!(field, "betabeta");
}

#[test]
fn the_paste_window_is_open_until_its_deadline() {
    let now = Instant::now();
    assert!(within_paste_suppress_window(Some(now + Duration::from_millis(200)), now));
    assert!(!within_paste_suppress_window(Some(now), now));
    assert!(!within_paste_suppress_window(None, now));
}
