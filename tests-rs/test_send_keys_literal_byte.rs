// `send-keys -H` is a byte channel: every operand is one hexadecimal byte value
// that reaches the pty verbatim (tmux flags those keys KEYC_LITERAL and
// input_key.c writes them as single bytes). psmux accepted the flag but never
// implemented it, so the operands fell through to the key-name lookup and were
// echoed into the pane as literal text -- typing `echo` produced "65 63 68 6f".
//
// These tests pin the two pure functions the fix rests on:
//   * decode_send_command  -- per-command operand decoding
//   * coalesce_send_commands -- merging consecutive sub-commands on one line
//
// They deliberately start no server (see AGENTS.md).

use super::*;

fn decode(line: &str) -> Option<(String, Vec<u8>)> {
    decode_send_command(line)
}

#[test]
fn literal_byte_operands_decode_to_raw_bytes() {
    // The repro: "echo" as bare hex bytes.
    let (target, bytes) = decode("send-keys -H -t %1 65 63 68 6f").expect("must decode");
    assert_eq!(target, "%1");
    assert_eq!(bytes, b"echo");
}

#[test]
fn literal_byte_carries_utf8_sequences_unchanged() {
    // 中 = E4 B8 AD. A byte channel must not re-encode these.
    let (_, bytes) = decode("send-keys -H -t %1 e4 b8 ad").expect("must decode");
    assert_eq!(bytes, vec![0xe4, 0xb8, 0xad]);
    assert_eq!(String::from_utf8(bytes).unwrap(), "中");
}

#[test]
fn literal_byte_accepts_bytes_that_are_not_valid_utf8() {
    // A caller may split a multi-byte character across chunked send-keys calls,
    // so individual commands are not required to be valid UTF-8.
    let (_, bytes) = decode("send-keys -H -t %1 41 00 ff 42").expect("must decode");
    assert_eq!(bytes, vec![0x41, 0x00, 0xff, 0x42]);
}

#[test]
fn hex_codepoint_operands_stay_codepoints() {
    // `0xNN` is the iTerm2 encoding for typed characters and means a codepoint,
    // so it is UTF-8 encoded. This is the opposite of -H and must stay that way.
    let (_, bytes) = decode("send -t %1 0x4e2d").expect("must decode");
    assert_eq!(bytes, vec![0xe4, 0xb8, 0xad], "0x4e2d is 中, encoded as UTF-8");

    // Latin-1 range: a codepoint, not a byte.
    let (_, bytes) = decode("send -t %1 0xe9").expect("must decode");
    assert_eq!(bytes, vec![0xc3, 0xa9], "0xe9 is é, encoded as UTF-8");
}

#[test]
fn malformed_literal_byte_operand_refuses_to_coalesce() {
    // Returning None leaves the command to the send-keys handler rather than
    // silently dropping or mangling it here.
    assert!(decode("send-keys -H -t %1 zz").is_none());
}

#[test]
fn coalescing_merges_mixed_encodings_into_one_write() {
    // iTerm2 splits an arrow key into three differently-encoded sub-commands.
    // They must merge into a single pty write, otherwise PSReadLine times out
    // between the ESC and the "[A" and prints them as literal characters.
    let parts = vec![
        "send -H -t %1 1b".to_string(),
        "send -t %1 0x5b".to_string(),
        "send -lt %1 A".to_string(),
    ];
    let merged = coalesce_send_commands(parts);
    assert_eq!(merged.len(), 1, "three sub-commands must merge into one");
    assert_eq!(merged[0], "send -H -t %1 1b 5b 41");
}

#[test]
fn coalesced_carrier_is_byte_exact_for_multibyte_input() {
    // The carrier used to be `send -l '<latin-1>'`, which mapped each byte
    // through `as char` and encoded already-UTF-8 bytes a second time: 中
    // arrived at the pane as "ä¸­". The -H carrier has no such round trip.
    let merged = coalesce_send_commands(vec!["send -t %1 0x4e2d".to_string()]);
    assert_eq!(merged, vec!["send -H -t %1 e4 b8 ad"]);

    // And the carrier re-decodes to exactly the bytes we started with.
    let (_, bytes) = decode(&merged[0]).expect("carrier must decode");
    assert_eq!(String::from_utf8(bytes).unwrap(), "中");
}

#[test]
fn coalescing_keeps_distinct_targets_apart() {
    let merged = coalesce_send_commands(vec![
        "send -H -t %1 41".to_string(),
        "send -H -t %2 42".to_string(),
    ]);
    assert_eq!(merged.len(), 2, "different panes must not be merged");
    assert_eq!(merged[0], "send -H -t %1 41");
    assert_eq!(merged[1], "send -H -t %2 42");
}

#[test]
fn non_send_commands_pass_through_untouched() {
    let parts = vec![
        "send -H -t %1 41".to_string(),
        "refresh-client -C 80,25".to_string(),
        "send -H -t %1 42".to_string(),
    ];
    let merged = coalesce_send_commands(parts);
    assert_eq!(
        merged,
        vec!["send -H -t %1 41", "refresh-client -C 80,25", "send -H -t %1 42"],
        "an unrelated command between two sends must break the run and survive verbatim"
    );
}

#[test]
fn option_separator_allows_a_leading_dash_key() {
    let (_, bytes) = decode("send-keys -l -- --help").expect("must decode");
    assert_eq!(bytes, b"--help");
}

#[test]
fn option_parsing_stops_at_the_first_key() {
    let (_, bytes) = decode("send-keys -l text -n").expect("must decode");
    assert_eq!(bytes, b"text-n");
}

#[test]
fn empty_key_operand_is_ignored() {
    let parsed = parse_send_keys_args(&["-l", "text", "", "-n"]);
    assert_eq!(parsed.operands, vec!["text", "-n"]);
}

#[test]
fn shared_parser_preserves_clusters_targets_and_later_dash_keys() {
    let parsed = parse_send_keys_args(&["-lt", "%1", "A", "-n"]);
    assert!(parsed.literal);
    assert_eq!(parsed.target, Some("%1"));
    assert_eq!(parsed.operands, vec!["A", "-n"]);
}

#[test]
fn unrecognized_single_dash_token_starts_key_input() {
    let parsed = parse_send_keys_args(&["-z", "-t", "%9"]);
    assert_eq!(parsed.target, None);
    assert_eq!(parsed.operands, vec!["-z", "-t", "%9"]);
}

#[test]
fn shared_dispatch_rejects_prefix_long_options_without_sending() {
    let (tx, rx) = mpsc::channel();
    let outcome = dispatch_send_keys(&["--hepl", "Enter"], &tx);
    assert_eq!(
        outcome,
        SendKeysDispatchOutcome::InvalidLongOption(
            "unknown send-keys option '--hepl'; to send it literally, use: psmux send -- --hepl".to_string()
        )
    );
    assert!(rx.try_recv().is_err(), "invalid prefix input must not reach the pane");

    let help = dispatch_send_keys(&["--help"], &tx);
    assert_eq!(help, SendKeysDispatchOutcome::Help);
    assert!(rx.try_recv().is_err(), "help must not reach the pane");
}

#[test]
fn reset_dispatches_before_following_keys() {
    let (tx, rx) = mpsc::channel();
    let outcome = dispatch_send_keys(&["-R", "Enter"], &tx);
    assert_eq!(outcome, SendKeysDispatchOutcome::Dispatched);
    assert!(matches!(rx.recv().unwrap(), CtrlReq::ResetTerminal));
    assert!(matches!(
        rx.recv().unwrap(),
        CtrlReq::SendKeys(keys, false) if keys == vec!["Enter"]
    ));
    assert!(rx.try_recv().is_err(), "reset plus one key must emit exactly two requests");
}

#[test]
fn shared_dispatch_sends_leading_dash_operands_unchanged() {
    let (tx, rx) = mpsc::channel();
    dispatch_send_keys(&["-l", "--", "--help", "-n"], &tx);

    match rx.recv().expect("one send request") {
        CtrlReq::SendKeys(keys, literal) => {
            assert!(literal);
            assert_eq!(keys, vec!["--help", "-n"]);
        }
        _ => panic!("expected SendKeys"),
    }
    assert!(rx.try_recv().is_err(), "dispatch must emit exactly one request");
}

#[test]
fn direct_cli_rebuild_protects_target_shaped_key_operands() {
    let command = crate::cli::build_send_keys_control_command(
        &["-t", "%1", "-l", "--", "-t", "%9"],
    );
    assert_eq!(command, "send-keys -l -- \"-t\" \"%9\"");
}

#[test]
fn flag_equals_normalization_stops_at_first_send_keys_operand() {
    let input = vec!["send-keys", "-l", "text", "-t=%9"];
    let normalized = crate::cli::normalize_flag_equals(
        input.iter().map(|arg| (*arg).to_string()).collect(),
    );
    assert_eq!(normalized, input);
}

#[test]
fn flag_equals_normalization_keeps_pre_boundary_send_keys_options() {
    let normalized = crate::cli::normalize_flag_equals(
        ["psmux", "send-keys", "-t=%1", "-N=2", "A"]
            .iter().map(|arg| (*arg).to_string()).collect(),
    );
    assert_eq!(normalized, vec!["psmux", "send-keys", "-t", "%1", "-N", "2", "A"]);

    let args: Vec<&str> = normalized.iter().skip(2).map(|arg| arg.as_str()).collect();
    let parsed = parse_send_keys_args(&args);
    assert_eq!(parsed.target, Some("%1"));
    assert_eq!(parsed.repeat_count, 2);
    assert_eq!(parsed.operands, vec!["A"]);
}

#[test]
fn send_keys_alias_expands_before_operand_normalization() {
    let parsed = expand_command_alias_and_normalize(
        parse_command_line("sk -t=%9"),
        Some("send-keys -l --"),
    );
    assert_eq!(parsed, vec!["send-keys", "-l", "--", "-t=%9"]);

    let args: Vec<&str> = parsed.iter().skip(1).map(|arg| arg.as_str()).collect();
    let send_keys = parse_send_keys_args(&args);
    assert_eq!(send_keys.target, None);
    assert_eq!(send_keys.operands, vec!["-t=%9"]);
}
