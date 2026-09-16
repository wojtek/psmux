//! Issue #650: a desktop psmux command must not reap the registry of a live
//! server it is merely not allowed to open.
//!
//! `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` is not the universal
//! same-user privilege the old comment in `pid_anchor_verdict` assumed. A
//! server started from an OpenSSH logon lives in Terminal Services session 0
//! under the elevated token sshd mints for an administrator; a desktop shell
//! lives in session 1 under the UAC-filtered Medium token. Measured on the
//! reporter's machine and reproduced here, that desktop caller is refused with
//! `ERROR_ACCESS_DENIED` while the session 0 side can open the desktop freely,
//! which is why only the desktop side reaped. The refusal needs both halves,
//! another Terminal Services session and an unelevated caller; within one
//! session the handle is granted across integrity levels.
//!
//! `get_process_name` answered `None` for that refusal, `pid_anchor_verdict`
//! read `None` as "the server has exited", and the startup registry sweep runs
//! on every invocation — so `psmux -V` deleted a live session's
//! `.port`/`.key`/`.pid`/`.sid`.
//!
//! These tests pin the two halves of the fix: the snapshot fallback names a
//! process the handle path cannot open but still says nothing about a PID that
//! is genuinely gone, and the anchor verdict keeps a live server when only the
//! handle path fails.

use super::*;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A private directory per case, matching the idiom the other registry tests
/// in this crate use rather than pulling in a temp-dir dependency.
fn temp_dir() -> PathBuf {
    let mut p = std::env::temp_dir();
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    p.push(format!("psmux_issue650_{}_{}", std::process::id(), n));
    let _ = std::fs::create_dir_all(&p);
    p
}

/// The snapshot fallback finds THIS process by PID.
///
/// Self-lookup goes through the handle path, so this is really asserting that
/// the two paths agree on shape: both answer with the `.exe`-stripped image
/// name for a process that is plainly alive.
#[test]
#[cfg(windows)]
fn snapshot_fallback_names_the_current_process() {
    let me = std::process::id();
    let name = crate::platform::process_info::get_process_name_or_snapshot(me)
        .expect("the running process must be nameable");
    assert!(
        !name.is_empty(),
        "expected a non-empty image name for our own pid {me}, got {name:?}"
    );
    assert!(
        !name.to_ascii_lowercase().ends_with(".exe"),
        "the liveness lookup must return the file stem like the handle path \
         does, not the raw snapshot image; got {name:?}"
    );
}

/// A PID that exists nowhere still answers `None`, so the #448 fast reap keeps
/// firing for a server that really has exited.
#[test]
#[cfg(windows)]
fn snapshot_fallback_still_says_none_for_a_pid_that_does_not_exist() {
    // Odd PIDs are never allocated by Windows (PIDs are multiples of 4), so
    // this one cannot collide with a live process no matter how loaded the
    // machine is.
    let absent = 0xFFFF_FFFDu32;
    assert_eq!(
        crate::platform::process_info::get_process_name_or_snapshot(absent),
        None,
        "a PID that is neither openable nor in the process table must stay None"
    );
}

/// PID 0 is the Toolhelp32 entry for "[System Process]" and must never be
/// mistaken for a nameable process.
#[test]
#[cfg(windows)]
fn snapshot_fallback_refuses_pid_zero() {
    assert_eq!(
        crate::platform::process_info::get_process_name_or_snapshot(0),
        None,
        "pid 0 is the snapshot's System Process placeholder, not a real process"
    );
}

/// The anchor verdict keeps a live server when only the handle path fails.
///
/// This is the issue in one assertion: same `.pid` file, same PID, and the
/// only difference is whether the name lookup can see a process the caller is
/// not allowed to open.
#[test]
#[cfg(windows)]
fn anchor_keeps_a_live_server_when_only_the_handle_path_fails() {
    let dir = temp_dir();
    let port_path = dir.join("ns650__remote.port");
    std::fs::write(&port_path, b"65000").expect("write port");
    // A PID psmux would be refused a handle for. Its numeric value is
    // irrelevant; the lookup is injected.
    std::fs::write(dir.join("ns650__remote.pid"), b"4242").expect("write pid");

    // The handle path is refused: this is what `get_process_name` returned
    // before the fix, and what it still returns across an integrity boundary.
    let handle_only = |_pid: u32| -> Option<String> { None };
    assert_eq!(
        pid_anchor_verdict_with(&port_path, handle_only),
        Some(false),
        "regression fixture: an unnameable PID is the verdict that reaped a \
         live server"
    );

    // With the snapshot second opinion the same PID names a psmux server.
    let with_snapshot = |_pid: u32| -> Option<String> { Some("psmux".to_string()) };
    assert_eq!(
        pid_anchor_verdict_with(&port_path, with_snapshot),
        Some(true),
        "a server the snapshot can still see must be kept, not reaped"
    );
}

/// A PID nothing can name is still a dead server: the fast reap survives.
#[test]
#[cfg(windows)]
fn anchor_still_reaps_a_pid_that_is_gone_everywhere() {
    let dir = temp_dir();
    let port_path = dir.join("ns650__gone.port");
    std::fs::write(&port_path, b"65001").expect("write port");
    std::fs::write(dir.join("ns650__gone.pid"), b"4243").expect("write pid");

    let nowhere = |_pid: u32| -> Option<String> { None };
    assert_eq!(
        pid_anchor_verdict_with(&port_path, nowhere),
        Some(false),
        "a PID that is neither openable nor in the table is a dead server"
    );
}

/// A PID that now names another image is no longer reaped on the name alone.
///
/// This entry is UNSIGNED: a bare `4244` body predates #448 and carries no
/// creation time, so nothing here identifies a process instance. The image name
/// is the only remaining hint and it is not trustworthy on its own, because
/// Windows resolves a live process's name from the executable file: renaming
/// that file made eight healthy servers report a foreign name on 2026-09-10 and
/// every caller that believed it deleted their registry entries. The anchor now
/// declines to answer and the network probe decides.
///
/// Reuse is still caught where it can be proved: a SIGNED entry whose recorded
/// creation time does not match is dead outright (see
/// `pid_anchor_reports_dead_when_the_recorded_signature_does_not_match` in
/// `test_session.rs`), and an unsigned entry with a recognised name keeps the
/// coarse created-after-the-file guard.
#[test]
#[cfg(windows)]
fn anchor_defers_on_an_unsigned_pid_naming_another_image() {
    let dir = temp_dir();
    let port_path = dir.join("ns650__recycled.port");
    std::fs::write(&port_path, b"65002").expect("write port");
    std::fs::write(dir.join("ns650__recycled.pid"), b"4244").expect("write pid");

    let other_image = |_pid: u32| -> Option<String> { Some("notepad".to_string()) };
    assert_eq!(
        pid_anchor_verdict_with(&port_path, other_image),
        None,
        "an unsigned entry with a foreign image name is inconclusive, not dead"
    );
}

/// The snapshot spelling of an image name reaches the same verdict as the
/// handle spelling.
///
/// The handle path returns the file stem with the image's real casing
/// (`psmux`); the Toolhelp32 entry is lowercased and carries the extension
/// (`psmux.exe`), which the fallback strips. Both must be accepted, because
/// the same registry file can be read by a caller that gets either one.
#[test]
#[cfg(windows)]
fn anchor_accepts_every_server_image_spelling() {
    let dir = temp_dir();
    let port_path = dir.join("ns650__spelling.port");
    std::fs::write(&port_path, b"65003").expect("write port");
    std::fs::write(dir.join("ns650__spelling.pid"), b"4245").expect("write pid");

    for spelling in ["psmux", "PSMUX", "tmux", "pmux"] {
        let name_of = move |_pid: u32| -> Option<String> { Some(spelling.to_string()) };
        assert_eq!(
            pid_anchor_verdict_with(&port_path, name_of),
            Some(true),
            "{spelling} names a psmux server image and must be kept"
        );
    }
}
