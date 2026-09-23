use super::*;
use crate::server::option_catalog::{OptionScope, OPTION_CATALOG};
use crate::server::options::{apply_set_option, get_option_value};

fn host_colors() -> crate::types::HostColors {
    crate::types::HostColors::campbell()
}

#[test]
fn tab_colour_is_a_session_option_and_round_trips() {
    let mut app = crate::types::AppState::new("tab-colour-test".to_string());
    let definition = OPTION_CATALOG
        .iter()
        .find(|definition| definition.name == "tab-colour")
        .expect("tab-colour must be in the option catalog");
    assert_eq!(definition.scope, OptionScope::Session);
    assert_eq!(get_option_value(&app, "tab-colour"), "");

    apply_set_option(&mut app, "tab-colour", "#ff4040", false).unwrap();
    assert_eq!(app.tab_colour, "#ff4040");
    assert_eq!(get_option_value(&app, "tab-colour"), "#ff4040");

    apply_set_option(&mut app, "tab-colour", "", false).unwrap();
    assert_eq!(app.tab_colour, "");
    assert_eq!(get_option_value(&app, "tab-colour"), "");
}

#[test]
fn tab_colour_set_emits_slot_264_then_decac_once() {
    let mut output = Vec::new();
    let mut last = None;
    emit_host_tab_color(&mut output, Some("#ff4040".to_string()), &mut last, &host_colors());
    assert_eq!(output, b"\x1b]4;264;rgb:ff/40/40\x1b\\\x1b[2;263;264,|");

    emit_host_tab_color(&mut output, Some("#ff4040".to_string()), &mut last, &host_colors());
    assert_eq!(output, b"\x1b]4;264;rgb:ff/40/40\x1b\\\x1b[2;263;264,|");
}

#[test]
fn tab_colour_clear_resets_slot_264_then_decac_once() {
    let mut output = Vec::new();
    let mut last = Some("#ff4040".to_string());
    emit_host_tab_color(&mut output, None, &mut last, &host_colors());
    assert_eq!(output, b"\x1b]104;264\x1b\\\x1b[2;263;264,|");

    emit_host_tab_color(&mut output, None, &mut last, &host_colors());
    assert_eq!(output, b"\x1b]104;264\x1b\\\x1b[2;263;264,|");
}

// Each attachment (run_remote) starts with nothing emitted, while the host
// terminal keeps whatever colour the previous attachment left on it. Without a
// release when an attachment ends, switching to a session that has no
// tab-colour, or detaching, left the previous session's colour on the tab.
const TAB_COLOUR_CLEAR: &[u8] = b"\x1b]104;264\x1b\\\x1b[2;263;264,|";

#[test]
fn a_switch_to_a_session_without_a_colour_clears_the_last_one() {
    let host_colors = host_colors();
    let mut output = Vec::new();
    let mut attachment_a = None;
    emit_host_tab_color(&mut output, Some("#ff4040".to_string()), &mut attachment_a, &host_colors);
    release_host_tab_color(&mut output, &mut attachment_a, &host_colors);
    let mut attachment_b = None;
    emit_host_tab_color(&mut output, None, &mut attachment_b, &host_colors);
    assert!(
        output.ends_with(TAB_COLOUR_CLEAR),
        "the tab must not keep the previous session's colour: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn detaching_clears_the_colour() {
    let host_colors = host_colors();
    let mut output = Vec::new();
    let mut last = None;
    emit_host_tab_color(&mut output, Some("#ff4040".to_string()), &mut last, &host_colors);
    release_host_tab_color(&mut output, &mut last, &host_colors);
    assert!(output.ends_with(TAB_COLOUR_CLEAR), "{:?}", String::from_utf8_lossy(&output));
    assert_eq!(last, None);
}

#[test]
fn releasing_without_a_colour_writes_nothing() {
    let host_colors = host_colors();
    let mut output = Vec::new();
    release_host_tab_color(&mut output, &mut None, &host_colors);
    release_host_tab_color(&mut output, &mut Some(String::new()), &host_colors);
    assert!(output.is_empty());
}

#[test]
fn tab_colour_reuses_named_and_indexed_style_spellings() {
    assert_eq!(
        host_tab_color_sequence(Some("red"), &host_colors()).as_deref(),
        Some("\x1b]4;264;rgb:c5/0f/1f\x1b\\\x1b[2;263;264,|"),
    );
    assert_eq!(
        host_tab_color_sequence(Some("colour99"), &host_colors()).as_deref(),
        Some("\x1b]4;264;rgb:87/5f/ff\x1b\\\x1b[2;263;264,|"),
    );
    assert_eq!(
        host_tab_color_sequence(Some("color99"), &host_colors()).as_deref(),
        Some("\x1b]4;264;rgb:87/5f/ff\x1b\\\x1b[2;263;264,|"),
    );
}
