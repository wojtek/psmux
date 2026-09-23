// A one-shot `send-keys` connection must keep reading after it enqueues the
// keys. `send_control` writes the command, then a `session-info` line, then
// half-closes and reads to EOF: the barrier's reply round-trips through the
// server's single FIFO event loop, so it is the proof that the keys were
// applied before the CLI returns. A command chained after send-keys on the
// same line has to run as well.
//
// The fork's send-keys arm used to end a one-shot connection straight after
// dispatch, so the barrier was never answered (the CLI returned while the keys
// were still queued) and any chained tail was dropped.
//
// These tests run the real connection handler on an ephemeral loopback socket
// with an in-memory event loop. They start no psmux server and touch no psmux
// registry, port file or data directory (see AGENTS.md).

use super::*;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, RwLock};
use std::time::Duration;

const KEY: &str = "one-shot-barrier-test-key";
const BARRIER_REPLY: &str = "barrier-answered";

/// Serve exactly one connection with `handle_connection`, feed it `AUTH`
/// followed by `lines`, half-close, and return everything the handler wrote
/// together with the requests it sent to the (fake) event loop, in order.
fn run_one_shot(lines: &str) -> (String, Vec<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::channel::<CtrlReq>();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let aliases = Arc::new(RwLock::new(std::collections::HashMap::new()));
        handle_connection(stream, tx, KEY, aliases);
    });

    // The handler owns the only sender, so this loop ends when it returns.
    let event_loop = std::thread::spawn(move || {
        let mut seen = Vec::new();
        while let Ok(req) = rx.recv_timeout(Duration::from_secs(10)) {
            match req {
                CtrlReq::SendKeys(keys, _) => seen.push(format!("keys:{}", keys.join(" "))),
                CtrlReq::SessionInfo(resp) => {
                    seen.push("session-info".to_string());
                    let _ = resp.send(BARRIER_REPLY.to_string());
                }
                CtrlReq::ClientActivity(_) => {}
                _ => seen.push("other".to_string()),
            }
        }
        seen
    });

    let mut client = TcpStream::connect(addr).expect("connect");
    client.set_read_timeout(Some(Duration::from_secs(10))).expect("read timeout");
    write!(client, "AUTH {}\n{}", KEY, lines).expect("write request");
    client.flush().expect("flush");
    client.shutdown(Shutdown::Write).expect("half-close");
    let mut out = String::new();
    let _ = client.read_to_string(&mut out);

    server.join().expect("handler thread");
    let seen = event_loop.join().expect("event loop thread");
    (out, seen)
}

#[test]
fn one_shot_send_keys_answers_the_execution_barrier() {
    // Exactly what `send_control` sends for `psmux send-keys -l hello`.
    let (out, seen) = run_one_shot("send-keys -l hello\nsession-info\n");
    assert!(
        out.contains(BARRIER_REPLY),
        "the session-info barrier after send-keys must be answered before EOF; got {:?}",
        out
    );
    assert_eq!(seen, vec!["keys:hello".to_string(), "session-info".to_string()]);
}

#[test]
fn one_shot_send_keys_runs_a_chained_tail() {
    let (out, seen) = run_one_shot("send-keys -l hello ; session-info\n");
    assert!(
        out.contains(BARRIER_REPLY),
        "a command chained after send-keys must still run; got {:?}",
        out
    );
    assert_eq!(seen, vec!["keys:hello".to_string(), "session-info".to_string()]);
}

// Every sub-command of a chain runs, once and in order. The connection loop
// took the next queued sub-command both at its bottom and again at its top, so
// with two or more queued, every other one was dropped: here "Y" never reached
// the pane although the barrier after it still answered.
#[test]
fn one_shot_chain_runs_every_sub_command_once() {
    let (out, seen) = run_one_shot("send-keys -l X ; send-keys -l Y ; session-info\n");
    assert!(out.contains(BARRIER_REPLY), "the barrier must still answer; got {:?}", out);
    assert_eq!(
        seen,
        vec!["keys:X".to_string(), "keys:Y".to_string(), "session-info".to_string()]
    );
}

#[test]
fn one_shot_send_keys_help_still_answers_without_input() {
    // Guard for the fork's help contract: help is printed, nothing reaches a pane.
    let (out, seen) = run_one_shot("send-keys --help\n");
    assert!(out.contains("send-keys"), "help text expected; got {:?}", out);
    assert!(
        seen.iter().all(|r| !r.starts_with("keys:")),
        "help must not send keys; requests were {:?}",
        seen
    );
}
