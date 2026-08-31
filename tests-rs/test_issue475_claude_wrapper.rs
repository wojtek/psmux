// Issue #475: the PSMUX_CLAUDE_TEAMMATE_MODE claude wrapper hardcoded
// `& claude.exe`, which breaks npm/nvm4w installs of Claude Code on Windows
// that ship only claude.cmd + claude.ps1 (no exe).  Every `claude` invocation
// inside a psmux pane then failed with "The term 'claude.exe' is not
// recognized".  The wrapper must resolve the real command at call time via
// Get-Command with a CommandType filter (Application covers .exe/.cmd,
// ExternalScript covers .ps1) which also excludes the wrapper function itself.

use super::ENV_SHIM_PS;

// Since psmux#399 the wrapper skips --teammate-mode injection when
// teammateMode is configured in any settings.json Claude Code reads (user
// scope, project scope walking up from the CWD, managed).  These tests assert
// injection DOES happen, so they must run isolated from the developer's real
// ~/.claude/settings.json: CLAUDE_CONFIG_DIR points at an empty dir and the
// CWD sits outside the user profile so the ancestor walk finds nothing.
fn run_pwsh(script: &str, path: &std::ffi::OsStr) -> std::process::Output {
    let cwd = neutral_cwd("475");
    let cfg = cwd.join("empty_claude_cfg");
    let _ = std::fs::create_dir_all(&cfg);
    let attempt = |exe: &str| {
        std::process::Command::new(exe)
            .args(["-NoProfile", "-Command", script])
            .env("PATH", path)
            .env("PSMUX_CLAUDE_TEAMMATE_MODE", "tmux")
            .env("CLAUDE_CONFIG_DIR", &cfg)
            .current_dir(&cwd)
            .output()
    };
    attempt("pwsh").or_else(|_| attempt("powershell")).expect("no PowerShell available")
}

/// A scratch directory outside the real user profile, so the wrapper's
/// ancestor .claude/settings.json walk (psmux#399) cannot pick up the
/// developer's own user-scope settings file.
fn neutral_cwd(tag: &str) -> std::path::PathBuf {
    let base = std::env::var_os("PUBLIC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("psmux_wrapper_test_{tag}"));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A user's PowerShell profile may define `claude` to supply owner-selected
/// defaults.  The psmux teammate wrapper must compose with that function rather
/// than replacing it and invoking the underlying application directly.  The
/// shim is applied twice to also prove that initialization cannot capture its
/// own wrapper and recurse.
#[test]
fn wrapper_preserves_existing_claude_function_defaults() {
    let dir = std::env::temp_dir().join("psmux_test475_user_claude_function");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("claude.ps1"),
        "Write-Output \"FAKE_CLAUDE_ARGS=$args\"\r\n",
    )
    .unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = vec![dir.clone()];
    dirs.extend(std::env::split_paths(&path).filter(|d| !d.join("claude.exe").exists()));
    let new_path = std::env::join_paths(dirs).unwrap();

    let script = format!(
        "$f='owner-profile-value'; function Global:claude {{ & claude.ps1 --dangerously-skip-permissions --effort max @args }}; {0}; {0}; claude --print hi; Write-Output \"PROFILE_F=$f\"",
        ENV_SHIM_PS
    );
    let out = run_pwsh(&script, &new_path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "composed wrapper failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains(
            "FAKE_CLAUDE_ARGS=--dangerously-skip-permissions --effort max --teammate-mode tmux --print hi"
        ),
        "psmux replaced the user's claude defaults or lost teammate arguments.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(
        stdout.matches("--dangerously-skip-permissions").count(),
        1,
        "repeated shim initialization duplicated the user's defaults.\nstdout: {stdout}"
    );
    assert_eq!(
        stdout.matches("--teammate-mode").count(),
        1,
        "repeated shim initialization duplicated teammate mode.\nstdout: {stdout}"
    );
    assert!(
        stdout.contains("PROFILE_F=owner-profile-value"),
        "shim initialization overwrote a profile-owned variable.\nstdout: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An explicit caller choice must still outrank automatic teammate mode while
/// flowing through the user's profile wrapper.
#[test]
fn existing_claude_function_receives_explicit_teammate_mode_once() {
    let dir = std::env::temp_dir().join("psmux_test475_user_claude_explicit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("claude.ps1"),
        "Write-Output \"FAKE_CLAUDE_ARGS=$args\"\r\n",
    )
    .unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = vec![dir.clone()];
    dirs.extend(std::env::split_paths(&path).filter(|d| !d.join("claude.exe").exists()));
    let new_path = std::env::join_paths(dirs).unwrap();

    let script = format!(
        "function Global:claude {{ & claude.ps1 --dangerously-skip-permissions --effort max @args }}; {}; claude --teammate-mode off --print hi",
        ENV_SHIM_PS
    );
    let out = run_pwsh(&script, &new_path);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains(
            "FAKE_CLAUDE_ARGS=--dangerously-skip-permissions --effort max --teammate-mode off --print hi"
        ),
        "explicit teammate mode did not pass through the user's wrapper.\nstdout: {stdout}"
    );
    assert_eq!(
        stdout.matches("--teammate-mode").count(),
        1,
        "explicit teammate mode was duplicated.\nstdout: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Functional proof: with a PATH that contains only npm-style claude.cmd /
/// claude.ps1 (no claude.exe anywhere), invoking `claude` through the real
/// shipped shim must run the npm shim and auto-inject --teammate-mode.
#[test]
fn wrapper_resolves_npm_only_install_without_claude_exe() {
    let dir = std::env::temp_dir().join("psmux_test475_fake_npm_claude");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("claude.cmd"),
        "@echo off\r\necho FAKE_NPM_CLAUDE_RAN args=%*\r\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("claude.ps1"),
        "Write-Output \"FAKE_NPM_CLAUDE_RAN args=$args\"\r\n",
    )
    .unwrap();

    // Reproduce the reporter's environment: strip every PATH dir that holds a
    // claude.exe, put the npm-style shim dir first.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = vec![dir.clone()];
    dirs.extend(std::env::split_paths(&path).filter(|d| !d.join("claude.exe").exists()));
    let new_path = std::env::join_paths(dirs).unwrap();

    let script = format!("{}; claude --version", ENV_SHIM_PS);
    let out = run_pwsh(&script, &new_path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout.contains("not recognized") && !stderr.contains("not recognized"),
        "BUG #475 REGRESSED: wrapper failed to resolve npm claude.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("FAKE_NPM_CLAUDE_RAN"),
        "npm-style claude was not executed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("--teammate-mode tmux"),
        "wrapper must auto-inject --teammate-mode tmux.\nstdout: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An explicit --teammate-mode passed by the caller must be forwarded as-is
/// (no duplicate injection).
#[test]
fn wrapper_passes_explicit_teammate_mode_through() {
    let dir = std::env::temp_dir().join("psmux_test475_fake_npm_claude_explicit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("claude.ps1"),
        "Write-Output \"FAKE_NPM_CLAUDE_RAN args=$args\"\r\n",
    )
    .unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = vec![dir.clone()];
    dirs.extend(std::env::split_paths(&path).filter(|d| !d.join("claude.exe").exists()));
    let new_path = std::env::join_paths(dirs).unwrap();

    let script = format!("{}; claude --teammate-mode off --print hi", ENV_SHIM_PS);
    let out = run_pwsh(&script, &new_path);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("args=--teammate-mode off --print hi"),
        "explicit --teammate-mode must pass through untouched.\nstdout: {stdout}"
    );
    let count = stdout.matches("--teammate-mode").count();
    assert_eq!(count, 1, "flag must not be injected twice.\nstdout: {stdout}");

    let _ = std::fs::remove_dir_all(&dir);
}
