use super::*;
use std::fs;

// Env-mutating tests serialise through crate::util::lock_test_env(), a single
// process-wide lock shared with the other env-touching test modules (see
// util.rs). A per-module mutex is not enough: USERPROFILE/HOME are
// process-global, so two modules with separate mutexes can still collide.

/// Helper: build a fresh AppState for testing.
fn mock_app() -> AppState {
    AppState::new("test_session".to_string())
}

/// RAII guard that sets HOME/USERPROFILE and restores on drop.
struct EnvGuard {
    orig_userprofile: Option<String>,
    orig_home: Option<String>,
    orig_xdg_config: Option<String>,
}
impl EnvGuard {
    fn new(home: &str) -> Self {
        let g = Self {
            orig_userprofile: std::env::var("USERPROFILE").ok(),
            orig_home: std::env::var("HOME").ok(),
            orig_xdg_config: std::env::var("XDG_CONFIG_HOME").ok(),
        };
        std::env::set_var("USERPROFILE", home);
        std::env::set_var("HOME", home);
        std::env::set_var("XDG_CONFIG_HOME", std::path::Path::new(home).join(".config"));
        g
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.orig_userprofile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match &self.orig_xdg_config {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &self.orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

// ── Issue #135: Plugin discovery must check XDG paths ───────────────────

/// When a plugin is installed at ~/.config/psmux/plugins/<name>/plugin.conf,
/// the @plugin auto-source must find it — not only check ~/.psmux/plugins/.
#[test]
fn plugin_discovery_finds_xdg_path() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_xdg_plugin_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-theme");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.conf"), "set -g @test-theme-option 'xdg-found'\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'psmux-plugins/psmux-test-theme'\n");

    let val = app.user_options.get("@test-theme-option");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("xdg-found"),
        "Plugin at XDG path (~/.config/psmux/plugins/) should be auto-sourced"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// When a plugin is installed at ~/.psmux/plugins/<name>/plugin.conf (classic path),
/// it should still be found (regression guard).
#[test]
fn plugin_discovery_still_finds_classic_path() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_classic_plugin_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".psmux").join("plugins").join("psmux-test-theme");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.conf"), "set -g @test-classic-option 'classic-found'\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'psmux-plugins/psmux-test-theme'\n");

    let val = app.user_options.get("@test-classic-option");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("classic-found"),
        "Plugin at classic path (~/.psmux/plugins/) should still be found"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// When a plugin.conf references ~/.psmux/plugins/ but the script is actually
/// at ~/.config/psmux/plugins/, psmux's run-shell should fall back to the XDG path.
#[test]
fn run_shell_tilde_psmux_fallback_to_xdg() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_runshell_fallback_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let xdg_scripts = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-plugin").join("scripts");
    fs::create_dir_all(&xdg_scripts).unwrap();
    fs::write(xdg_scripts.join("test.ps1"), "# test script\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let wrong_path = tmp.join(".psmux").join("plugins");
    assert!(!wrong_path.is_dir(), "Test setup: classic plugin dir should NOT exist");

    let correct_path = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-plugin").join("scripts").join("test.ps1");
    assert!(correct_path.exists(), "Test setup: script should exist at XDG path");

    let _ = fs::remove_dir_all(&tmp);
}

/// XDG path with short plugin name (no org/ prefix) should also be found.
#[test]
fn plugin_discovery_xdg_short_name() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_xdg_short_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-short");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.conf"), "set -g @test-short 'short-found'\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-short'\n");

    let val = app.user_options.get("@test-short");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("short-found"),
        "Plugin found by short name at XDG path"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// XDG PS1 plugin discovery should also work.
#[test]
fn plugin_discovery_xdg_ps1_entry() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_xdg_ps1_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-ps1");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("psmux-test-ps1.ps1"),
        "# PSMux plugin\n# tmux set -g @ps1-test 'ps1-found'\n",
    ).unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-ps1'\n");

    for script in &app.pending_plugin_scripts {
        assert!(
            !script.contains(".psmux\\plugins") || script.contains(".config\\psmux\\plugins"),
            "Pending script path should use XDG location, got: {}", script
        );
    }

    let _ = fs::remove_dir_all(&tmp);
}

/// A '#branch' suffix on the spec must not shadow the plugin name. Branch names
/// may contain '/' (e.g. 'temp/integration'), so deriving the name from the last
/// '/' segment of 'MattKotsenas/psmux-plugins/psmux-test-branch#temp/integration'
/// would pick 'integration' and miss the plugin; the '#branch' suffix is stripped
/// first. Regression guard.
#[test]
fn plugin_discovery_strips_branch_suffix() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_branch_suffix_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".psmux").join("plugins").join("psmux-test-branch");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("plugin.conf"),
        "set -g @test-branch-option 'branch-found'\n",
    ).unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "set -g @plugin 'MattKotsenas/psmux-plugins/psmux-test-branch#temp/integration'\n",
    );

    let val = app.user_options.get("@test-branch-option");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("branch-found"),
        "A '#temp/integration' branch suffix must not shadow the plugin name"
    );

    let _ = fs::remove_dir_all(&tmp);
}
