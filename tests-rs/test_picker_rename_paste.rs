// A Ctrl+V into the picker's `$` rename dialog must insert the clipboard once.
//
// On Windows crossterm delivers one Ctrl+V as an Event::Paste AND as the
// per-character key events of the same text. The Event::Paste arm puts the
// text into the rename field and opens the 200 ms duplicate-paste window that
// every other overlay's character arm honours (issue #290); the rename
// dialog's arm did not, so the key events typed the text a second time.
//
// These tests pin the two pure decisions; they start no server (see AGENTS.md).

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

#[test]
fn the_paste_window_is_open_until_its_deadline() {
    let now = Instant::now();
    assert!(within_paste_suppress_window(Some(now + Duration::from_millis(200)), now));
    assert!(!within_paste_suppress_window(Some(now), now));
    assert!(!within_paste_suppress_window(None, now));
}
