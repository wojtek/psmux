// Control mode merges consecutive `send`/`send-keys` sub-commands into one
// `send -H <bytes>` command (coalesce_send_commands) before any sub-command is
// classified. The fork's send-keys contract says `--help` prints help and an
// unknown long option is rejected, both without sending anything to a pane.
// The coalescer did not ask that classifier: a literal command such as
// `send -lt %1 --help` was turned into the bytes "--help", so a control-mode
// client typed the help flag (or a misspelled option) into the pane instead of
// getting help or an error. `--` still makes a dash-leading operand literal.
//
// These tests pin the pure coalescer; they start no server (see AGENTS.md).

use super::*;

fn coalesce(parts: &[&str]) -> Vec<String> {
    coalesce_send_commands(parts.iter().map(|p| p.to_string()).collect())
}

#[test]
fn literal_help_reaches_the_classifier_intact() {
    assert_eq!(coalesce(&["send-keys -l --help"]), vec!["send-keys -l --help".to_string()]);
    assert_eq!(coalesce(&["send -lt %1 --help"]), vec!["send -lt %1 --help".to_string()]);
}

#[test]
fn literal_unknown_long_option_reaches_the_classifier_intact() {
    assert_eq!(coalesce(&["send -lt %1 --bogus"]), vec!["send -lt %1 --bogus".to_string()]);
}

#[test]
fn a_help_request_breaks_a_run_of_literal_sends() {
    assert_eq!(
        coalesce(&["send -lt %1 a", "send -lt %1 --help", "send -lt %1 b"]),
        vec![
            "send -H -t %1 61".to_string(),
            "send -lt %1 --help".to_string(),
            "send -H -t %1 62".to_string(),
        ]
    );
}

#[test]
fn a_dash_leading_operand_after_double_dash_stays_literal() {
    // `--` ends option parsing, so this is text to type, not a help request.
    assert_eq!(
        coalesce(&["send -lt %1 -- --help"]),
        vec!["send -H -t %1 2d 2d 68 65 6c 70".to_string()]
    );
}

#[test]
fn non_literal_help_was_never_coalesced() {
    assert_eq!(coalesce(&["send-keys --help"]), vec!["send-keys --help".to_string()]);
}
