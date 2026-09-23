use super::*;

#[test]
fn successive_independent_same_text_pastes_are_forwarded() {
    let mut gesture = PasteGesture::default();
    assert!(take_terminal_paste(&mut gesture, "abc"));
    assert!(take_terminal_paste(&mut gesture, "abc"));
}

#[test]
fn successive_independent_different_text_pastes_are_forwarded() {
    let mut gesture = PasteGesture::default();
    assert!(take_terminal_paste(&mut gesture, "abc"));
    assert!(take_terminal_paste(&mut gesture, "def"));
}

#[test]
fn local_gesture_read_back_is_dropped_and_next_paste_is_forwarded() {
    let mut gesture = PasteGesture::default();
    gesture.start();
    gesture.record("abc");
    assert!(!take_terminal_paste(&mut gesture, "abc"));
    gesture.finish();
    assert!(take_terminal_paste(&mut gesture, "abc"));
}

#[test]
fn first_paste_after_start_is_forwarded() {
    let mut gesture = PasteGesture::default();
    gesture.start();
    assert!(take_terminal_paste(&mut gesture, "abc"));
}
