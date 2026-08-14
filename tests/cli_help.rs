use std::process::{Command, Output};

fn run_psmux(args: &[&str]) -> Output {
    let namespace = format!("cli-help-test-{}", std::process::id());
    Command::new(env!("CARGO_BIN_EXE_psmux"))
        .args(["-L", &namespace])
        .args(args)
        .output()
        .expect("psmux command should start")
}

#[test]
fn send_help_succeeds_without_a_session_for_every_alias() {
    for command in ["send-keys", "send", "send-key"] {
        let output = run_psmux(&[command, "--help"]);
        assert!(
            output.status.success(),
            "{command} --help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("USAGE:"),
            "{command} --help did not print usage"
        );
        assert!(
            stdout.contains("send-keys"),
            "{command} --help did not identify the command"
        );
        assert!(
            output.stderr.is_empty(),
            "{command} --help wrote stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = run_psmux(&["help", "send"]);
    assert!(
        output.status.success(),
        "help send failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("USAGE:"));
}

#[test]
fn misspelled_long_option_fails_before_session_access() {
    let output = run_psmux(&["send", "--hepl"]);
    assert!(
        !output.status.success(),
        "misspelled option unexpectedly succeeded"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown send-keys option '--hepl'"),
        "unexpected error: {stderr}"
    );
    assert!(
        stderr.contains("psmux send -- --hepl"),
        "error did not explain literal input: {stderr}"
    );
    assert!(
        !stderr.contains("no server running"),
        "command reached session lookup: {stderr}"
    );
}
