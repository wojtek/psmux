// A request addressed to a named session carries no window/pane target.
//
// PSMUX_TARGET_FULL holds the full `-t` target (session:window.pane) of the
// command line that started the client. It is set once and never cleared when
// an attached client switches sessions. The console VT reader asks the session
// it switches to for its `escape-time` by name, and that query carried the
// stale `TARGET A:1` of a startup `attach -t A:1` with it: the new session
// validated window 1 before answering, a session without that window answered
// with an error instead of the number, and the switch ended the client.
//
// These tests run the real request exchange against a loopback listener that
// records what it receives. They start no psmux server and touch no psmux
// registry, port file or data directory (see AGENTS.md).

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

const KEY: &str = "control-target-test-key";

/// A client that started with `attach -t A:1` and has since switched sessions.
fn stale_startup_env(key: &str) -> Option<String> {
    (key == "PSMUX_TARGET_FULL").then(|| "A:1".to_string())
}

/// Serve one exchange: take the whole request (the client half-closes after
/// sending it), answer `reply` after the AUTH ack, and hand back what arrived.
fn exchange_with_recorder(
    full_target: Option<String>,
    line: &str,
    reply: &str,
) -> (std::io::Result<String>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("local addr");
    let reply = reply.to_string();
    let recorder = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("read timeout");
        let mut received = String::new();
        let _ = stream.read_to_string(&mut received);
        let _ = write!(stream, "OK\n{}", reply);
        received
    });
    let result = exchange_control_request(&addr, KEY, full_target.as_deref(), line);
    let received = recorder.join().expect("recorder thread");
    (result, received)
}

#[test]
fn a_query_addressed_to_the_switched_to_session_sends_no_stale_target() {
    let full_target = control_full_target(ControlRouting::Session, stale_startup_env);
    let (result, received) =
        exchange_with_recorder(full_target, "show-options -gv escape-time\n", "0\n");
    assert_eq!(
        received,
        format!("AUTH {}\nshow-options -gv escape-time\n", KEY),
        "the startup target must not reach the session the client switched to"
    );
    assert_eq!(result.expect("the reply").trim(), "0");
}

#[test]
fn a_routed_request_still_carries_the_command_line_target() {
    let full_target = control_full_target(ControlRouting::Routed, stale_startup_env);
    let (_, received) = exchange_with_recorder(full_target, "list-panes\n", "");
    assert_eq!(received, format!("AUTH {}\nTARGET A:1\nlist-panes\n", KEY));
}

#[test]
fn a_routed_request_without_a_target_sends_none() {
    assert_eq!(control_full_target(ControlRouting::Routed, |_| None), None);
}
