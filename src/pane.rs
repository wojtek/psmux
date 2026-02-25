use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::types::{AppState, Pane, Node, LayoutKind, Window};
use crate::tree::{replace_leaf_with_split, active_pane_mut, kill_leaf};

/// Send a preemptive cursor-position report (\x1b[1;1R) to the ConPTY input pipe.
///
/// Windows ConPTY sends a Device Status Report (\x1b[6n]) during initialization
/// and **blocks** until the host responds with a cursor-position report.  In
/// portable-pty ≤0.2 this was handled internally, but 0.9+ exposes raw handles
/// and the host must respond.  Writing the response preemptively (before the
/// reader thread even starts) is safe because the data sits in the pipe buffer
/// and ConPTY reads it when ready.
pub fn conpty_preemptive_dsr_response(writer: &mut dyn std::io::Write) {
    let _ = writer.write_all(b"\x1b[1;1R");
    let _ = writer.flush();
}

/// Cached resolved shell path to avoid repeated `which::which()` PATH scans.
/// Resolved once on first use, reused for all subsequent pane spawns.
static CACHED_SHELL_PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Get the cached shell path, resolving via `which` only on first call.
fn cached_shell() -> Option<&'static str> {
    CACHED_SHELL_PATH.get_or_init(|| {
        which::which("pwsh").ok()
            .or_else(|| which::which("cmd").ok())
            .map(|p| p.to_string_lossy().into_owned())
    }).as_deref()
}

/// Determine the default shell name for window naming (like tmux shows "bash", "zsh").
fn default_shell_name(command: Option<&str>, configured_shell: Option<&str>) -> String {
    if let Some(cmd) = command {
        // Extract the program name from the command string
        let first = cmd.split_whitespace().next().unwrap_or(cmd);
        std::path::Path::new(first)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(first)
            .to_string()
    } else if let Some(shell) = configured_shell {
        // Use configured default-shell name
        let first = shell.split_whitespace().next().unwrap_or(shell);
        std::path::Path::new(first)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(first)
            .to_string()
    } else {
        // Default shell — use cached resolved path
        cached_shell()
            .and_then(|p| std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "shell".into())
    }
}

pub fn create_window(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, command: Option<&str>) -> io::Result<()> {
    // Use actual terminal size if known, otherwise fall back to defaults
    let area = app.last_window_area;
    let rows = if area.height > 1 { area.height } else { 30 }.max(MIN_PANE_DIM);
    let cols = if area.width > 1 { area.width } else { 120 }.max(MIN_PANE_DIM);
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;

    // When no explicit command is given, use the configured default-shell
    // (from `set -g default-shell` / `default-command`).
    let mut shell_cmd = if command.is_some() {
        build_command(command)
    } else if !app.default_shell.is_empty() {
        build_default_shell(&app.default_shell)
    } else {
        build_command(None)
    };
    set_tmux_env(&mut shell_cmd, app.next_pane_id, app.control_port, app.socket_name.as_deref());
    let child = pair
        .slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // On Windows ConPTY the slave handle MUST be closed after spawning so the
    // child owns the sole reference to the console input pipe.  Leaving it open
    // causes "The handle is invalid" IOExceptions inside the child process.
    drop(pair.slave);

    let scrollback = app.history_limit as u32;
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, scrollback as usize)));
    let term_reader = term.clone();
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    spawn_reader_thread(reader, term_reader, dv_writer);

    let configured_shell = if app.default_shell.is_empty() { None } else { Some(app.default_shell.as_str()) };
    let child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*child) };
    let mut pty_writer = pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?;
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let pane = Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id), child_pid, data_version, last_title_check: std::time::Instant::now(), last_infer_title: std::time::Instant::now(), dead: false, vt_bridge_cache: None, copy_state: None };
    app.next_pane_id += 1;
    let win_name = command.map(|c| default_shell_name(Some(c), None)).unwrap_or_else(|| default_shell_name(None, configured_shell));
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0 });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    Ok(())
}

pub fn split_active(app: &mut AppState, kind: LayoutKind) -> io::Result<()> {
    split_active_with_command(app, kind, None, None)
}

/// Create a new window with a raw command (program + args, no shell wrapping)
pub fn create_window_raw(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, raw_args: &[String]) -> io::Result<()> {
    let area = app.last_window_area;
    let rows = if area.height > 1 { area.height } else { 30 };
    let cols = if area.width > 1 { area.width } else { 120 };
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;

    let mut shell_cmd = build_raw_command(raw_args);
    set_tmux_env(&mut shell_cmd, app.next_pane_id, app.control_port, app.socket_name.as_deref());
    let child = pair
        .slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // Close the slave handle immediately – see create_window() comment.
    drop(pair.slave);

    let scrollback = app.history_limit;
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, scrollback)));
    let term_reader = term.clone();
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    spawn_reader_thread(reader, term_reader, dv_writer);

    let child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*child) };
    let mut pty_writer = pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?;
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let pane = Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id), child_pid, data_version, last_title_check: std::time::Instant::now(), last_infer_title: std::time::Instant::now(), dead: false, vt_bridge_cache: None, copy_state: None };
    app.next_pane_id += 1;
    let win_name = std::path::Path::new(&raw_args[0]).file_stem().and_then(|s| s.to_str()).unwrap_or(&raw_args[0]).to_string();
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0 });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    Ok(())
}

/// Minimum pane dimension (rows or cols) — ConPTY on Windows crashes
/// the child process if either dimension is less than 2.
pub const MIN_PANE_DIM: u16 = 2;

/// Minimum rows for a split to be allowed — each resulting pane needs at
/// least this many rows to run a shell prompt.
const MIN_SPLIT_ROWS: u16 = 4;
/// Minimum cols for a split to be allowed.
const MIN_SPLIT_COLS: u16 = 10;

pub fn split_active_with_command(app: &mut AppState, kind: LayoutKind, command: Option<&str>, pty_system_ref: Option<&dyn portable_pty::PtySystem>) -> io::Result<()> {
    // ── Guard: refuse split if the active pane is too small ──────────
    // After splitting, each half gets roughly (dim / 2) - 1 (for the divider).
    // If that would be below MIN_PANE_DIM, deny the split to avoid crashing
    // the child process (ConPTY cannot function below ~2 rows or cols).
    {
        let win = &app.windows[app.active_idx];
        if let Some(p) = crate::tree::active_pane(&win.root, &win.active_path) {
            let (cur_rows, cur_cols) = (p.last_rows, p.last_cols);
            match kind {
                LayoutKind::Vertical => {
                    // Splitting vertically divides height; need room for 2 panes + 1 divider
                    if cur_rows < MIN_SPLIT_ROWS * 2 + 1 {
                        return Err(io::Error::new(io::ErrorKind::Other,
                            format!("pane too small to split vertically ({cur_rows} rows, need {})", MIN_SPLIT_ROWS * 2 + 1)));
                    }
                }
                LayoutKind::Horizontal => {
                    // Splitting horizontally divides width; need room for 2 panes + 1 divider
                    if cur_cols < MIN_SPLIT_COLS * 2 + 1 {
                        return Err(io::Error::new(io::ErrorKind::Other,
                            format!("pane too small to split horizontally ({cur_cols} cols, need {})", MIN_SPLIT_COLS * 2 + 1)));
                    }
                }
            }
        }
    }

    // Reuse provided PTY system or create one as fallback
    let owned_pty;
    let pty_system: &dyn portable_pty::PtySystem = if let Some(ps) = pty_system_ref {
        ps
    } else {
        owned_pty = native_pty_system();
        &*owned_pty
    };
    // Compute target pane size from the *active pane's* actual dimensions,
    // not the full window area — ensures we don't over-estimate and then
    // immediately resize to a tiny rect.
    let (pane_rows, pane_cols) = {
        let win = &app.windows[app.active_idx];
        if let Some(p) = crate::tree::active_pane(&win.root, &win.active_path) {
            (p.last_rows, p.last_cols)
        } else {
            let area = app.last_window_area;
            (if area.height > 1 { area.height } else { 30 }, if area.width > 1 { area.width } else { 120 })
        }
    };
    let (rows, cols) = match kind {
        LayoutKind::Vertical => {
            let half = (pane_rows.saturating_sub(1)) / 2; // subtract 1 for divider
            (half.max(MIN_PANE_DIM), pane_cols.max(MIN_PANE_DIM))
        }
        LayoutKind::Horizontal => {
            let half = (pane_cols.saturating_sub(1)) / 2;
            (pane_rows.max(MIN_PANE_DIM), half.max(MIN_PANE_DIM))
        }
    };
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    // When no explicit command is given, use the configured default-shell.
    let mut shell_cmd = if command.is_some() {
        build_command(command)
    } else if !app.default_shell.is_empty() {
        build_default_shell(&app.default_shell)
    } else {
        build_command(None)
    };
    set_tmux_env(&mut shell_cmd, app.next_pane_id, app.control_port, app.socket_name.as_deref());
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // Close the slave handle immediately – see create_window() comment.
    drop(pair.slave);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, app.history_limit)));
    let term_reader = term.clone();
    let reader = pair.master.try_clone_reader().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    spawn_reader_thread(reader, term_reader, dv_writer);
    let child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*child) };
    let mut pty_writer = pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?;
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let new_leaf = Node::Leaf(Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id), child_pid, data_version, last_title_check: std::time::Instant::now(), last_infer_title: std::time::Instant::now(), dead: false, vt_bridge_cache: None, copy_state: None });
    app.next_pane_id += 1;
    let win = &mut app.windows[app.active_idx];
    replace_leaf_with_split(&mut win.root, &win.active_path, kind, new_leaf);
    let mut new_path = win.active_path.clone();
    new_path.push(1);
    win.active_path = new_path;
    Ok(())
}

pub fn kill_active_pane(app: &mut AppState) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    // Explicitly kill the active pane's process tree FIRST.
    // remove_node() doesn't call kill_node() when the root is a single Leaf,
    // so we must do it here to ensure no orphaned processes.
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        crate::platform::process_kill::kill_process_tree(&mut p.child);
    }
    kill_leaf(&mut win.root, &win.active_path);
    Ok(())
}

pub fn detect_shell() -> CommandBuilder {
    build_command(None)
}

/// Set TMUX and TMUX_PANE environment variables on a CommandBuilder.
/// TMUX format: /tmp/psmux-{server_pid}/{socket_name},{port},0
/// TMUX_PANE format: %{pane_id}
/// The socket_name component encodes the -L namespace for child process resolution.
pub fn set_tmux_env(builder: &mut CommandBuilder, pane_id: usize, control_port: Option<u16>, socket_name: Option<&str>) {
    let server_pid = std::process::id();
    let port = control_port.unwrap_or(0);
    let sn = socket_name.unwrap_or("default");
    // Format compatible with tmux: <socket_path>,<pid>,<session_idx>
    // We encode the socket name in the path component for -L namespace resolution
    builder.env("TMUX", format!("/tmp/psmux-{}/{},{},0", server_pid, sn, port));
    builder.env("TMUX_PANE", format!("%{}", pane_id));
}

pub fn build_command(command: Option<&str>) -> CommandBuilder {
    if let Some(cmd) = command {
        let shell = cached_shell().map(|s| s.to_string());
        
        match shell {
            Some(path) => {
                let mut builder = CommandBuilder::new(&path);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                
                if path.to_lowercase().contains("pwsh") {
                    builder.args(["-NoLogo", "-Command", cmd]);
                } else {
                    builder.args(["/C", cmd]);
                }
                builder
            }
            None => {
                let mut builder = CommandBuilder::new("pwsh.exe");
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                builder.args(["-NoLogo", "-Command", cmd]);
                builder
            }
        }
    } else {
        let shell = cached_shell().map(|s| s.to_string());
        // PSReadLine v2.2.6+ enables PredictionSource HistoryAndPlugin by default.
        // Predictions cause display corruption in terminal multiplexers because
        // PSReadLine's VT rendering races with ConPTY output capture.
        // We aggressively disable ALL prediction features with multiple fallback layers.
        let psrl_init = concat!(
            "$PSStyle.OutputRendering = 'Ansi'; ",
            "try { Set-PSReadLineOption -PredictionSource None -ErrorAction Stop } catch {}; ",
            "try { Set-PSReadLineOption -PredictionViewStyle InlineView -ErrorAction Stop } catch {}; ",
            "try { Remove-PSReadLineKeyHandler -Chord 'F2' -ErrorAction Stop } catch {}",
        );
        match shell {
            Some(path) => {
                let mut builder = CommandBuilder::new(&path);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                if path.to_lowercase().contains("pwsh") {
                    builder.args(["-NoLogo", "-NoExit", "-Command", psrl_init]);
                }
                builder
            }
            None => {
                let mut builder = CommandBuilder::new("pwsh.exe");
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                builder
            }
        }
    }
}

/// Cached resolved default-shell path to avoid repeated `which::which()` scans.
static CACHED_DEFAULT_SHELL: std::sync::OnceLock<std::collections::HashMap<String, String>> = std::sync::OnceLock::new();
static CACHED_DEFAULT_SHELL_MAP: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> = std::sync::Mutex::new(None);

/// Resolve a program name via `which`, caching the result.
fn cached_which(program: &str) -> String {
    // Fast path: check if already cached in the global OnceLock for the default
    // (most common case is always the same shell)
    let mut map = CACHED_DEFAULT_SHELL_MAP.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(std::collections::HashMap::new);
    if let Some(cached) = map.get(program) {
        return cached.clone();
    }
    let resolved = which::which(program).ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string());
    map.insert(program.to_string(), resolved.clone());
    resolved
}

/// Build a CommandBuilder that launches the given shell path interactively.
/// Used when `default-shell` / `default-command` is configured.
/// Supports pwsh, powershell, cmd, and any arbitrary executable.
pub fn build_default_shell(shell_path: &str) -> CommandBuilder {
    // Extract the program (first token) and optional extra arguments.
    let parts: Vec<&str> = shell_path.split_whitespace().collect();
    let program = parts.first().copied().unwrap_or(shell_path);
    let extra_args: Vec<&str> = if parts.len() > 1 { parts[1..].to_vec() } else { vec![] };

    // Resolve bare names via cached `which` — avoids repeated PATH scans.
    let resolved = cached_which(program);

    let lower = resolved.to_lowercase();
    let mut builder = CommandBuilder::new(&resolved);
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("PSMUX_SESSION", "1");

    // Prepend extra arguments (e.g. -NoProfile) BEFORE our -NoExit/-Command block
    // so they're interpreted as flags rather than as -Command arguments.
    if !extra_args.is_empty() {
        builder.args(extra_args.clone());
    }

    if lower.contains("pwsh") || lower.contains("powershell") {
        // PSReadLine prediction workaround for PowerShell-based shells.
        let psrl_init = concat!(
            "$PSStyle.OutputRendering = 'Ansi'; ",
            "try { Set-PSReadLineOption -PredictionSource None -ErrorAction Stop } catch {}; ",
            "try { Set-PSReadLineOption -PredictionViewStyle InlineView -ErrorAction Stop } catch {}; ",
            "try { Remove-PSReadLineKeyHandler -Chord 'F2' -ErrorAction Stop } catch {}",
        );
        builder.args(["-NoLogo", "-NoExit", "-Command", psrl_init]);
    }

    builder
}

/// Build a CommandBuilder for direct execution (no shell wrapping).
/// raw_args[0] is the program, rest are its arguments.
/// Used when -- separator is specified in new-session.
pub fn build_raw_command(raw_args: &[String]) -> CommandBuilder {
    if raw_args.is_empty() {
        return build_command(None);
    }
    let program = &raw_args[0];
    let mut builder = CommandBuilder::new(program);
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("PSMUX_SESSION", "1");
    if raw_args.len() > 1 {
        let args: Vec<&str> = raw_args[1..].iter().map(|s| s.as_str()).collect();
        builder.args(args);
    }
    builder
}

/// Spawn a dedicated PTY reader thread that processes output and updates the
/// data_version counter. Exits cleanly after 200 consecutive zero-byte reads
/// (indicating the PTY pipe is closed) or on any I/O error.
///
/// Uses an 8KB read buffer (down from 64KB) to reduce mutex hold time during
/// `parser.process()`, which improves DumpState latency under heavy output.
pub fn spawn_reader_thread(
    mut reader: Box<dyn std::io::Read + Send>,
    term_reader: Arc<Mutex<vt100::Parser>>,
    dv_writer: Arc<std::sync::atomic::AtomicU64>,
) {
    thread::spawn(move || {
        let mut local = [0u8; 8192];
        let mut zero_reads: u32 = 0;
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => {
                    zero_reads = 0;
                    if let Ok(mut parser) = term_reader.lock() {
                        parser.process(&local[..n]);
                    }
                    dv_writer.fetch_add(1, std::sync::atomic::Ordering::Release);
                    crate::types::PTY_DATA_READY.store(true, std::sync::atomic::Ordering::Release);
                }
                Ok(_) => {
                    zero_reads += 1;
                    if zero_reads > 200 { break; }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
}

// reap_children is in tree.rs
