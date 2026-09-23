// Regression tests for the stale-port startup tax.
//
// Root cause proven by measurement: every CLI invocation ran
// cleanup_stale_port_files(), which TCP-probed each .port file serially
// (3 attempts x 100ms connect timeout). On Windows hosts where a dead
// loopback port never sends RST (stealth firewall), each probe attempt
// burned its full connect timeout AND classified as Inconclusive, so the
// stale files were never reaped: ~350-400ms per stale file on EVERY psmux
// command, forever. Six stale files made `psmux new-session` take ~2.4s
// (and cold start ~4.9s, since the spawned server pays the tax again).
//
// The fix consults the .pid sentinel (issue #448) first: a dead PID is a
// definitive, microsecond-cheap stale verdict with no network round-trip.

use super::*;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_psmux_dir(test_name: &str) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("psmux_{test_name}_{}_{}", std::process::id(), n))
        .join(".psmux");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_registry(dir: &std::path::Path, session: &str, port: &str) -> (PathBuf, PathBuf, PathBuf) {
    let port_path = dir.join(format!("{session}.port"));
    let key_path = dir.join(format!("{session}.key"));
    let sid_path = dir.join(format!("{session}.sid"));
    fs::write(&port_path, port).unwrap();
    fs::write(&key_path, "test-key").unwrap();
    fs::write(&sid_path, "1").unwrap();
    (port_path, key_path, sid_path)
}

/// Spawn a short-lived real process and return its PID after it has exited.
/// This produces a PID that is genuinely dead (not fabricated), matching what
/// a hard-killed session server leaves behind in its .pid file.
#[cfg(windows)]
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("cmd")
        .args(["/c", "exit"])
        .spawn()
        .expect("spawn cmd");
    let pid = child.id();
    let _ = child.wait();
    pid
}

#[cfg(windows)]
#[test]
fn dead_pid_anchor_reaps_registry_without_any_network_probe() {
    let dir = temp_psmux_dir("pid_anchor_dead");
    let (port_path, key_path, sid_path) = write_registry(&dir, "crashed", "54329");
    let pid_path = dir.join("crashed.pid");
    fs::write(&pid_path, dead_pid().to_string()).unwrap();

    // The probe must never run: a dead .pid anchor is definitive on its own.
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        panic!("network probe must not run when the .pid anchor says dead");
    });

    assert!(!port_path.exists(), ".port of a dead-PID session must be reaped");
    assert!(!key_path.exists(), ".key of a dead-PID session must be reaped");
    assert!(!sid_path.exists(), ".sid of a dead-PID session must be reaped");
    assert!(!pid_path.exists(), ".pid sentinel itself must be reaped");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[cfg(windows)]
#[test]
fn recycled_pid_is_reaped_without_probe_when_its_signature_does_not_match() {
    // Fork identity rule (FORK.md; `pid_anchor_verdict`): the recorded pid:creation pair decides, never the image name.
    // The current test process is alive, as an unrelated application holding
    // a dead server's recycled PID would be. The `.pid` body records that PID
    // with a creation time this process does not have, so the server that
    // wrote it is provably gone.
    let dir = temp_psmux_dir("pid_anchor_recycled");
    let (port_path, _, _) = write_registry(&dir, "recycled", "54330");
    let pid = std::process::id();
    let live_creation = crate::platform::process_kill::process_creation_time(pid)
        .expect("the running test process has a creation time");
    fs::write(dir.join("recycled.pid"), format_pid_file_contents(pid, live_creation - 1)).unwrap();

    cleanup_stale_port_files_in_with(&dir, |_, _| {
        panic!("network probe must not run when the recorded signature does not match");
    });

    assert!(!port_path.exists(), "recycled-PID registry must be reaped");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn missing_pid_anchor_falls_back_to_network_probe() {
    // Pre-#448 registries have no .pid file; behavior must be unchanged:
    // the probe runs, and Inconclusive keeps the files.
    let dir = temp_psmux_dir("pid_anchor_missing");
    let (port_path, key_path, sid_path) = write_registry(&dir, "legacy", "54331");

    let mut probe_ran = false;
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        probe_ran = true;
        PortProbeResult::Inconclusive
    });

    assert!(probe_ran, "without a .pid anchor the network probe must still run");
    assert!(port_path.exists(), "inconclusive probe must keep .port");
    assert!(key_path.exists(), "inconclusive probe must keep .key");
    assert!(sid_path.exists(), "inconclusive probe must keep .sid");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn unparseable_pid_anchor_falls_back_to_network_probe() {
    let dir = temp_psmux_dir("pid_anchor_garbage");
    let (port_path, _, _) = write_registry(&dir, "garbled", "54332");
    fs::write(dir.join("garbled.pid"), "not-a-pid").unwrap();

    let mut probe_ran = false;
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        probe_ran = true;
        PortProbeResult::Inconclusive
    });

    assert!(probe_ran, "garbage .pid must fall back to the network probe");
    assert!(port_path.exists(), "inconclusive fallback must keep files");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[cfg(windows)]
#[test]
fn cleanup_with_many_dead_pid_registries_is_fast() {
    // The measured defect: 6 stale files cost ~2.4s (serial network probes).
    // With the PID anchor, even 6 dead registries must clean up in well under
    // the cost of a single 100ms probe attempt.
    let dir = temp_psmux_dir("pid_anchor_speed");
    let dead = dead_pid();
    for i in 0..6 {
        write_registry(&dir, &format!("dead{i}"), &format!("5430{i}"));
        fs::write(dir.join(format!("dead{i}.pid")), dead.to_string()).unwrap();
    }

    let start = Instant::now();
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        panic!("no network probes expected for dead-PID registries");
    });
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(250),
        "cleanup of 6 dead registries took {:?}; the stale-port tax is back",
        elapsed
    );
    for i in 0..6 {
        assert!(!dir.join(format!("dead{i}.port")).exists(), "dead{i}.port must be reaped");
    }
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn filetime_conversion_is_monotonic_and_anchored() {
    // 1601->1970 offset must be present and ordering preserved.
    let now = std::time::SystemTime::now();
    let later = now + Duration::from_secs(10);
    let a = system_time_to_filetime_ticks(now).unwrap();
    let b = system_time_to_filetime_ticks(later).unwrap();
    assert!(b > a, "later SystemTime must map to larger FILETIME ticks");
    assert_eq!(b - a, 10 * 10_000_000, "10s must be exactly 10^8 ticks");
    assert!(a > 116_444_736_000_000_000, "ticks must include the 1601 epoch offset");
}
