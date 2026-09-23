// `send-keys -R -t <pane> ...` resets the targeted pane and must deliver its
// keys to that same pane.
//
// A `-t` command runs under a temporary focus: the connection first asks the
// server to focus the target (FocusTargetTemp), and the server restores the
// user's focus as soon as it has handled the first request that ends the
// temporary focus. `-R` is dispatched as its own ResetTerminal request ahead of
// the input, so when the reset ended the temporary focus, the reset hit the
// target but the keys went to whichever pane had been active before: a command
// meant for one pane could be typed into another.
//
// These tests replay the server's rule (`request_ends_temp_focus`) over the
// exact requests the real dispatcher sends. They start no server (see
// AGENTS.md).

use super::*;

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Reset,
    Keys,
    Bytes,
    CopyMode,
    Paste,
    Other,
}

fn step(req: &CtrlReq) -> Step {
    match req {
        CtrlReq::ResetTerminal { .. } => Step::Reset,
        CtrlReq::SendKeys(..) => Step::Keys,
        CtrlReq::SendBytes(_) => Step::Bytes,
        CtrlReq::SendKeysX(_) => Step::CopyMode,
        CtrlReq::SendPaste(_) => Step::Paste,
        _ => Step::Other,
    }
}

/// Dispatch one send-keys command as if its `-t` target had just been focused
/// and replay the server's temporary-focus rule over what it sends. Each
/// request is paired with whether it runs while the target still has focus;
/// the flag returned last says whether the temporary focus outlived the
/// command.
fn replay_after_target_focus(args: &[&str]) -> (Vec<(Step, bool)>, bool) {
    let (tx, rx) = mpsc::channel();
    assert_eq!(dispatch_send_keys(args, &tx), SendKeysDispatchOutcome::Dispatched);
    drop(tx);
    let mut on_target = true;
    let mut steps = Vec::new();
    for req in rx.iter() {
        steps.push((step(&req), on_target));
        if crate::server::request_ends_temp_focus(&req) {
            on_target = false;
        }
    }
    (steps, on_target)
}

#[test]
fn reset_then_keys_both_run_on_the_targeted_pane() {
    let (steps, still_held) = replay_after_target_focus(&["-R", "hello", "Enter"]);
    assert_eq!(steps, vec![(Step::Reset, true), (Step::Keys, true)]);
    assert!(!still_held, "the temporary focus must end with the command");
}

#[test]
fn reset_then_hex_bytes_run_on_the_targeted_pane() {
    let (steps, still_held) = replay_after_target_focus(&["-R", "-H", "41", "42"]);
    assert_eq!(steps, vec![(Step::Reset, true), (Step::Bytes, true)]);
    assert!(!still_held);
}

#[test]
fn reset_then_copy_mode_command_runs_on_the_targeted_pane() {
    let (steps, still_held) = replay_after_target_focus(&["-R", "-X", "cancel"]);
    assert_eq!(steps, vec![(Step::Reset, true), (Step::CopyMode, true)]);
    assert!(!still_held);
}

#[test]
fn reset_then_paste_runs_on_the_targeted_pane() {
    let (steps, still_held) = replay_after_target_focus(&["-R", "-p", "some text"]);
    assert_eq!(steps, vec![(Step::Reset, true), (Step::Paste, true)]);
    assert!(!still_held);
}

#[test]
fn a_bare_reset_runs_on_the_target_and_releases_it() {
    let (steps, still_held) = replay_after_target_focus(&["-R"]);
    assert_eq!(steps.first(), Some(&(Step::Reset, true)));
    assert!(steps.iter().all(|(_, on_target)| *on_target), "steps: {:?}", steps);
    assert!(!still_held, "a reset with nothing to type must not keep the target focused");
}

#[test]
fn a_reset_whose_hex_operands_are_all_malformed_releases_the_target() {
    // Nothing follows the reset, so the reset itself has to end the focus.
    let (steps, still_held) = replay_after_target_focus(&["-R", "-H", "zz"]);
    assert_eq!(steps, vec![(Step::Reset, true)]);
    assert!(!still_held);
}
