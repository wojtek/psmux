use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, PtySystemSelection};

use crate::types::*;
use crate::tree::*;

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
        // Default shell — find which shell we'll launch
        which::which("pwsh").ok()
            .or_else(|| which::which("cmd").ok())
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "shell".into())
    }
}

pub fn create_window(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, command: Option<&str>) -> io::Result<()> {
    let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
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
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    thread::spawn(move || {
        let mut local = [0u8; 65536];
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => {
                    let mut parser = term_reader.lock().unwrap();
                    parser.process(&local[..n]);
                    drop(parser);
                    dv_writer.fetch_add(1, std::sync::atomic::Ordering::Release);
                    crate::types::PTY_DATA_READY.store(true, std::sync::atomic::Ordering::Release);
                }
                Ok(_) => thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    });

    let configured_shell = if app.default_shell.is_empty() { None } else { Some(app.default_shell.as_str()) };
    let child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*child) };
    let pane = Pane { master: pair.master, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id), child_pid, data_version, last_title_check: std::time::Instant::now(), last_infer_title: std::time::Instant::now(), dead: false, vt_bridge_cache: None };
    app.next_pane_id += 1;
    let win_name = command.map(|c| default_shell_name(Some(c), None)).unwrap_or_else(|| default_shell_name(None, configured_shell));
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0 });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    Ok(())
}

pub fn split_active(app: &mut AppState, kind: LayoutKind) -> io::Result<()> {
    split_active_with_command(app, kind, None)
}

/// Create a new window with a raw command (program + args, no shell wrapping)
pub fn create_window_raw(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, raw_args: &[String]) -> io::Result<()> {
    let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
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

    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 1000)));
    let term_reader = term.clone();
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    thread::spawn(move || {
        let mut local = [0u8; 65536];
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => {
                    let mut parser = term_reader.lock().unwrap();
                    parser.process(&local[..n]);
                    drop(parser);
                    dv_writer.fetch_add(1, std::sync::atomic::Ordering::Release);
                    crate::types::PTY_DATA_READY.store(true, std::sync::atomic::Ordering::Release);
                }
                Ok(_) => thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    });

    let child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*child) };
    let pane = Pane { master: pair.master, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id), child_pid, data_version, last_title_check: std::time::Instant::now(), last_infer_title: std::time::Instant::now(), dead: false, vt_bridge_cache: None };
    app.next_pane_id += 1;
    let win_name = std::path::Path::new(&raw_args[0]).file_stem().and_then(|s| s.to_str()).unwrap_or(&raw_args[0]).to_string();
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0 });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    Ok(())
}

pub fn split_active_with_command(app: &mut AppState, kind: LayoutKind, command: Option<&str>) -> io::Result<()> {
    let pty_system = PtySystemSelection::default().get().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
    let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
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
    let mut reader = pair.master.try_clone_reader().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    thread::spawn(move || {
        let mut local = [0u8; 65536];
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => { let mut parser = term_reader.lock().unwrap(); parser.process(&local[..n]); drop(parser); dv_writer.fetch_add(1, std::sync::atomic::Ordering::Release); crate::types::PTY_DATA_READY.store(true, std::sync::atomic::Ordering::Release); }
                Ok(_) => thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    });
    let child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*child) };
    let new_leaf = Node::Leaf(Pane { master: pair.master, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id), child_pid, data_version, last_title_check: std::time::Instant::now(), last_infer_title: std::time::Instant::now(), dead: false, vt_bridge_cache: None });
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
        let pwsh = which::which("pwsh").ok().map(|p| p.to_string_lossy().into_owned());
        let cmd_exe = which::which("cmd").ok().map(|p| p.to_string_lossy().into_owned());
        
        match pwsh.or(cmd_exe) {
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
        let pwsh = which::which("pwsh").ok().map(|p| p.to_string_lossy().into_owned());
        let cmd_exe = which::which("cmd").ok().map(|p| p.to_string_lossy().into_owned());
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
        match pwsh.or(cmd_exe) {
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

/// Build a CommandBuilder that launches the given shell path interactively.
/// Used when `default-shell` / `default-command` is configured.
/// Supports pwsh, powershell, cmd, and any arbitrary executable.
pub fn build_default_shell(shell_path: &str) -> CommandBuilder {
    // Extract the program (first token) and optional extra arguments.
    let parts: Vec<&str> = shell_path.split_whitespace().collect();
    let program = parts.first().copied().unwrap_or(shell_path);
    let extra_args: Vec<&str> = if parts.len() > 1 { parts[1..].to_vec() } else { vec![] };

    // Resolve bare names via `which`.
    let resolved = which::which(program).ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string());

    let lower = resolved.to_lowercase();
    let mut builder = CommandBuilder::new(&resolved);
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("PSMUX_SESSION", "1");

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

    // Append any extra arguments from the default-shell string.
    if !extra_args.is_empty() {
        builder.args(extra_args);
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

// reap_children is in tree.rs
