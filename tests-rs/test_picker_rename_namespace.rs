// A picker rename is planned in the namespace of the server being renamed.
//
// A server registers as `<namespace>__<name>`, or as a bare `<name>` in the
// default namespace, and the namespace is the `-L` value unchanged, which may
// itself contain `__`. The rename used to plan with the client's startup `-L`,
// which is not the selected server's namespace when the client attached to a
// namespaced session by its full registry name. Splitting the registry name at
// its first `__` instead broke namespaces that contain `__`: `team__qa__alpha`
// renamed to `beta` was expected as `team__beta` while the server became
// `team__qa__beta`. In both cases the client waited for a registry entry the
// server never writes and reported a successful rename as a failure.
//
// The plan now takes the session name the selected server reports for itself
// (its `session-info`); the namespace is exactly what precedes `__<name>`.
//
// These tests pin the pure plan; they start no server (see AGENTS.md).

use super::*;

fn plan(old_base: &str, server_name: &str, entered: &str) -> Result<(String, String), String> {
    picker_rename_plan(old_base, server_name, entered)
}

fn names(logical: &str, registry: &str) -> Result<(String, String), String> {
    Ok((logical.to_string(), registry.to_string()))
}

#[test]
fn a_namespaced_server_is_expected_under_its_own_namespace() {
    // Reached by full registry name from a client started without -L.
    assert_eq!(plan("team__alpha", "alpha", "beta"), names("beta", "team__beta"));
}

#[test]
fn a_namespace_that_contains_a_double_underscore_is_kept_whole() {
    // Created with `-L team__qa`; whatever -L the renaming client has.
    assert_eq!(plan("team__qa__alpha", "alpha", "beta"), names("beta", "team__qa__beta"));
}

#[test]
fn a_typed_namespace_prefix_is_not_doubled() {
    assert_eq!(plan("team__alpha", "alpha", "team__beta"), names("beta", "team__beta"));
    assert_eq!(plan("team__qa__alpha", "alpha", "team__qa__beta"), names("beta", "team__qa__beta"));
}

#[test]
fn a_session_name_that_contains_a_double_underscore_keeps_its_namespace() {
    assert_eq!(plan("team__a__b", "a__b", "c"), names("c", "team__c"));
}

#[test]
fn a_default_namespace_server_keeps_a_bare_name() {
    assert_eq!(plan("alpha", "alpha", "beta"), names("beta", "beta"));
}

#[test]
fn a_server_whose_name_is_not_its_registry_entry_is_not_renamed() {
    assert!(plan("team__alpha", "other", "beta").is_err());
}

#[test]
fn the_collision_check_sees_the_namespaced_name() {
    let (_, new_base) = plan("team__qa__alpha", "alpha", "beta").expect("plan");
    assert!(
        picker_session_name_conflicts(["team__qa__alpha", "team__qa__beta"], "team__qa__alpha", &new_base),
        "team__qa__beta already exists in the list the chooser shows"
    );
}

// The server's own name reaches the plan through its session-info reply. A
// session may be called ` alpha` (the picker allows a leading space), and a
// reader that trimmed the whole reply turned it into `alpha`, which no longer
// matched the registry entry, so the rename was refused.

const KEY: &str = "picker-rename-identity-test-key";

/// Plan a rename through a loopback responder that answers the session-info
/// query with `info_line` the way a psmux server writes it.
fn plan_through_query(info_line: &str, old_base: &str, entered: &str) -> Result<(String, String), String> {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("local addr").to_string();
    let reply = format!("OK\n{}\n", info_line);
    let responder = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("read timeout");
        let mut request = BufReader::new(stream.try_clone().expect("clone"));
        let (mut auth, mut command) = (String::new(), String::new());
        request.read_line(&mut auth).expect("auth line");
        request.read_line(&mut command).expect("command line");
        assert_eq!(command, "session-info\n");
        let mut stream = stream;
        stream.write_all(reply.as_bytes()).expect("reply");
    });
    let plan = picker_rename_plan_from_server(&addr, KEY, old_base, entered);
    responder.join().expect("responder thread");
    plan
}

#[test]
fn a_leading_space_in_the_server_name_survives_the_query() {
    assert_eq!(
        plan_through_query(" alpha: 1 windows (created Tue Sep 23 10:00:00 2026)", " alpha", "beta"),
        names("beta", "beta")
    );
}

#[test]
fn a_namespaced_leading_space_name_survives_the_query() {
    assert_eq!(
        plan_through_query(" alpha: 1 windows (created Tue Sep 23 10:00:00 2026)", "team__ alpha", "beta"),
        names("beta", "team__beta")
    );
}

#[test]
fn the_query_plans_a_compound_namespace() {
    assert_eq!(
        plan_through_query("alpha: 2 windows (created Tue Sep 23 10:00:00 2026) (attached)", "team__qa__alpha", "beta"),
        names("beta", "team__qa__beta")
    );
}

#[test]
fn the_server_name_is_the_start_of_its_session_info() {
    assert_eq!(
        picker_server_session_name("alpha: 2 windows (created Tue Sep 23 10:00:00 2026) (attached)"),
        Some("alpha")
    );
    assert_eq!(picker_server_session_name("no separator"), None);
    assert_eq!(picker_server_session_name(": 1 windows"), None);
}
