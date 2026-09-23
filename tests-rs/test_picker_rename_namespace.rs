// A picker rename is planned in the namespace of the server being renamed.
//
// The session chooser lists the sessions of the namespace the client is
// attached in, which comes from the attached session's registry name: a client
// can attach to `team__alpha` by that full name without having been started
// with `-L team`. The `$` rename built the name to wait for, and the name to
// check for collisions, from the client's own startup `-L` instead. Renaming
// `team__alpha` to `beta` from such a client asked the server to become
// "beta", which it did as `team__beta`, while the client waited for a registry
// entry called "beta", reported the successful rename as a failure after the
// confirmation timeout, and never ran its rename-success update. The collision
// check missed an existing `team__beta` the same way.
//
// These tests pin picker_rename_plan; they start no server (see AGENTS.md).

use super::*;

fn plan(old_base: &str, entered: &str) -> (String, String) {
    picker_rename_plan(old_base, entered)
}

#[test]
fn a_namespaced_server_is_expected_under_its_own_namespace() {
    assert_eq!(plan("team__alpha", "beta"), ("beta".to_string(), "team__beta".to_string()));
}

#[test]
fn a_typed_namespace_prefix_is_not_doubled() {
    assert_eq!(plan("team__alpha", "team__beta"), ("beta".to_string(), "team__beta".to_string()));
}

#[test]
fn the_collision_check_sees_the_namespaced_name() {
    let (_, new_base) = plan("team__alpha", "beta");
    assert!(
        picker_session_name_conflicts(["team__alpha", "team__beta"], "team__alpha", &new_base),
        "team__beta already exists in the list the chooser shows"
    );
}

#[test]
fn a_default_namespace_server_keeps_a_bare_name() {
    assert_eq!(plan("alpha", "beta"), ("beta".to_string(), "beta".to_string()));
}
