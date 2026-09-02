use super::*;
use crate::server::option_catalog::OPTION_CATALOG;
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
    assert_eq!(definition.scope, "session");
    assert_eq!(get_option_value(&app, "tab-colour"), "");

    apply_set_option(&mut app, "tab-colour", "#ff4040", false);
    assert_eq!(app.tab_colour, "#ff4040");
    assert_eq!(get_option_value(&app, "tab-colour"), "#ff4040");

    apply_set_option(&mut app, "tab-colour", "", false);
    assert_eq!(app.tab_colour, "");
    assert_eq!(get_option_value(&app, "tab-colour"), "");
}

#[test]
fn tab_colour_set_emits_slot_264_then_decac_once() {
    let mut output = Vec::new();
    let mut last = None;
    emit_host_tab_color(
        &mut output,
        Some("#ff4040".to_string()),
        &mut last,
        &host_colors(),
    );
    assert_eq!(output, b"\x1b]4;264;rgb:ff/40/40\x1b\\\x1b[2;263;264,|",);

    emit_host_tab_color(
        &mut output,
        Some("#ff4040".to_string()),
        &mut last,
        &host_colors(),
    );
    assert_eq!(
        output, b"\x1b]4;264;rgb:ff/40/40\x1b\\\x1b[2;263;264,|",
        "an unchanged value must not emit again",
    );
}

#[test]
fn tab_colour_clear_resets_slot_264_then_decac_once() {
    let mut output = Vec::new();
    let mut last = Some("#ff4040".to_string());
    emit_host_tab_color(&mut output, None, &mut last, &host_colors());
    assert_eq!(output, b"\x1b]104;264\x1b\\\x1b[2;263;264,|");

    emit_host_tab_color(&mut output, None, &mut last, &host_colors());
    assert_eq!(
        output, b"\x1b]104;264\x1b\\\x1b[2;263;264,|",
        "an unchanged clear state must not emit again",
    );
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
