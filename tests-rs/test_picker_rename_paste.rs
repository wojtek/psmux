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

// The field's text after one Ctrl+V, whichever order Windows delivers it in.
// When the characters come first and a late Event::Paste repeats them (the
// ordering upstream d828af2 handles for the pane), the paste used to be
// appended again, doubling the name.

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Deliver `text` as key events 1 ms apart from `start`, through the same key
/// predicate and paste window the rename field uses. Returns the time after it.
fn key_burst(
    field: &mut String,
    burst: &mut PickerRenameBurst,
    text: &str,
    start: Instant,
    paste_window_until: Option<Instant>,
) -> Instant {
    let mut now = start;
    for c in text.chars() {
        if picker_rename_accepts_char(KeyModifiers::NONE, within_paste_suppress_window(paste_window_until, now)) {
            picker_rename_take_char(field, burst, c, now);
        }
        now += ms(1);
    }
    now
}

#[test]
fn characters_then_a_late_event_paste_leave_the_name_once() {
    let t0 = Instant::now();
    let (mut field, mut burst) = (String::new(), None);
    let after = key_burst(&mut field, &mut burst, "beta", t0, None);
    let taken = picker_rename_take_paste(&mut field, &mut burst, "beta", after + ms(20));
    assert_eq!(field, "beta");
    assert!(!taken);
}

#[test]
fn an_event_paste_then_its_characters_leave_the_name_once() {
    let t0 = Instant::now();
    let (mut field, mut burst) = (String::new(), None);
    assert!(picker_rename_take_paste(&mut field, &mut burst, "beta", t0));
    // The Event::Paste arm opens the 200 ms duplicate-paste window (#290).
    key_burst(&mut field, &mut burst, "beta", t0 + ms(5), Some(t0 + ms(200)));
    assert_eq!(field, "beta");
}

#[test]
fn a_late_paste_of_a_burst_typed_after_other_text_is_dropped() {
    let t0 = Instant::now();
    let (mut field, mut burst) = (String::new(), None);
    key_burst(&mut field, &mut burst, "x", t0, None);
    let after = key_burst(&mut field, &mut burst, "beta", t0 + ms(400), None);
    let taken = picker_rename_take_paste(&mut field, &mut burst, "beta", after + ms(20));
    assert_eq!(field, "xbeta");
    assert!(!taken);
}

#[test]
fn a_paste_of_different_text_is_taken() {
    let t0 = Instant::now();
    let (mut field, mut burst) = (String::new(), None);
    let after = key_burst(&mut field, &mut burst, "ab", t0, None);
    assert!(picker_rename_take_paste(&mut field, &mut burst, "cd", after + ms(20)));
    assert_eq!(field, "abcd");
}

#[test]
fn a_repeat_paste_well_after_the_typing_is_taken() {
    let t0 = Instant::now();
    let (mut field, mut burst) = (String::new(), None);
    let after = key_burst(&mut field, &mut burst, "beta", t0, None);
    assert!(picker_rename_take_paste(&mut field, &mut burst, "beta", after + ms(1000)));
    assert_eq!(field, "betabeta");
}

#[test]
fn the_paste_window_is_open_until_its_deadline() {
    let now = Instant::now();
    assert!(within_paste_suppress_window(Some(now + Duration::from_millis(200)), now));
    assert!(!within_paste_suppress_window(Some(now), now));
    assert!(!within_paste_suppress_window(None, now));
}
