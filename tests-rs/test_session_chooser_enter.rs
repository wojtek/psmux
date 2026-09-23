// Enter in the session chooser on the session this client is already attached
// to closes the chooser in place. Nothing changes on the server, so no new
// frame arrives, and an idle client skips drawing when neither a frame nor a
// local change asks for it: the chooser was closed but stayed painted on
// screen. The next key both cleared it and went to the program in the pane,
// so choosing the current session "needed another key" and leaked one.
// (Inherited from upstream; `psmux pick` makes it the common case.) Escape
// already asks for the repaint; Enter now does too.
//
// These tests pin session_chooser_enter; they start no server (see AGENTS.md).

use super::*;

fn entries() -> Vec<(String, String)> {
    vec![
        ("main".to_string(), "1 windows (attached)".to_string()),
        ("work".to_string(), "2 windows".to_string()),
        ("other".to_string(), "1 windows".to_string()),
    ]
}

#[test]
fn enter_on_the_attached_session_closes_the_chooser_and_repaints() {
    let enter = session_chooser_enter(&entries(), "", 0, "", "main");
    assert_eq!(
        enter,
        SessionChooserEnter { switch_to: None, close: true, redraw: true },
        "closing in place gets no frame from the server, so the client must repaint itself"
    );
}

#[test]
fn enter_on_another_session_switches_to_it() {
    let enter = session_chooser_enter(&entries(), "", 1, "", "main");
    assert_eq!(enter.switch_to.as_deref(), Some("work"));
    assert!(enter.close);
}

#[test]
fn a_typed_number_wins_over_the_arrow_cursor() {
    let enter = session_chooser_enter(&entries(), "", 0, "3", "main");
    assert_eq!(enter.switch_to.as_deref(), Some("other"));
}

#[test]
fn a_typed_number_naming_the_attached_session_repaints_too() {
    let enter = session_chooser_enter(&entries(), "", 2, "1", "main");
    assert_eq!(enter, SessionChooserEnter { switch_to: None, close: true, redraw: true });
}

#[test]
fn an_out_of_range_number_leaves_the_chooser_open() {
    assert_eq!(session_chooser_enter(&entries(), "", 0, "7", "main"), SessionChooserEnter::default());
}

#[test]
fn the_filter_selects_among_matching_rows() {
    let enter = session_chooser_enter(&entries(), "oth", 0, "", "main");
    assert_eq!(enter.switch_to.as_deref(), Some("other"));
}
