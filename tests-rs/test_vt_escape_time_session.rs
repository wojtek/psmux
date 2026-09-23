// The console VT reader (SSH input) waits the attached session's `escape-time`
// before it treats a lone ESC as the Escape key.
//
// Two things went wrong. The value was queried through the routed
// PSMUX_TARGET_SESSION, but the client attaches to PSMUX_SESSION_NAME, and the
// two differ when `psmux pick` or a positional attach names a session other
// than the one command routing picked. And the reader thread took the value by
// copy once at startup, so switching to a session with a different
// escape-time kept the first session's value for the life of the client. A
// fragmented arrow key could then turn into Escape plus literal characters.
//
// These tests pin the session choice and the reader's shared timeout; they
// start no server and no reader thread (see AGENTS.md).

use super::*;
use std::collections::HashMap;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> =
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |key| map.get(key).cloned()
}

#[test]
fn the_escape_time_comes_from_the_attached_session_not_the_routed_one() {
    let env = env_of(&[("PSMUX_SESSION_NAME", "attached"), ("PSMUX_TARGET_SESSION", "recent")]);
    assert_eq!(attached_session_name_from(env), "attached");
}

#[test]
fn with_no_session_named_the_client_attaches_to_default() {
    assert_eq!(attached_session_name_from(env_of(&[])), "default");
}

#[test]
fn a_new_escape_time_reaches_the_running_reader() {
    // The reader thread holds its own handle for the life of the client.
    let client_handle = crate::ssh_input::EscapeTimeout::new(500);
    let reader_handle = client_handle.clone();
    client_handle.set(0);
    assert_eq!(reader_handle.get(), 0, "a switch must change the timeout the reader uses");
}
