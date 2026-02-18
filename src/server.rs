use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::env;
use std::net::TcpListener;

use portable_pty::PtySystemSelection;
use ratatui::prelude::*;

use crate::types::*;
use crate::platform::install_console_ctrl_handler;
use crate::cli::parse_target;
use crate::pane::*;
use crate::tree::{self, *};

/// Serialize key_tables into a compact JSON array for syncing to the client.
/// Format: [{"t":"prefix","k":"x","c":"split-window -v","r":false}, ...]
fn serialize_bindings_json(app: &AppState) -> String {
    use crate::commands::format_action;
    use crate::config::format_key_binding;
    let mut out = String::from("[");
    let mut first = true;
    for (table_name, binds) in &app.key_tables {
        for bind in binds {
            if !first { out.push(','); }
            first = false;
            let key_str = json_escape_string(&format_key_binding(&bind.key));
            let cmd_str = json_escape_string(&format_action(&bind.action));
            let tbl_str = json_escape_string(table_name);
            out.push_str(&format!("{{\"t\":\"{}\",\"k\":\"{}\",\"c\":\"{}\",\"r\":{}}}",
                tbl_str, key_str, cmd_str, bind.repeat));
        }
    }
    out.push(']');
    out
}

/// Escape a string for embedding inside a JSON double-quoted value.
/// Handles backslashes, double-quotes, and control characters.
fn json_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}
/// Build windows JSON with pre-expanded tab_text for each window.
/// The tab_text is the fully expanded window-status-format / window-status-current-format.
fn list_windows_json_with_tabs(app: &AppState) -> std::io::Result<String> {
    let mut v: Vec<WinInfo> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() {
        let is_active = i == app.active_idx;
        let fmt = if is_active { &app.window_status_current_format } else { &app.window_status_format };
        let tab = expand_format_for_window(fmt, app, i);
        v.push(WinInfo {
            id: w.id,
            name: w.name.clone(),
            active: is_active,
            activity: w.activity_flag,
            tab_text: tab,
        });
    }
    serde_json::to_string(&v).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("json error: {e}")))
}
use crate::input::*;
use crate::copy_mode::*;
use crate::layout::*;
use crate::window_ops::*;
use crate::config::*;
use crate::commands::*;
use crate::util::*;
use crate::format::*;

/// Complete list of supported tmux-compatible commands (for list-commands).
const TMUX_COMMANDS: &[&str] = &[
    "attach-session (attach)", "bind-key (bind)", "break-pane (breakp)",
    "capture-pane (capturep)", "choose-buffer", "choose-client",
    "choose-session", "choose-tree", "choose-window",
    "clear-history (clearhist)", "clock-mode", "command-prompt",
    "confirm-before (confirm)", "copy-mode", "customize-mode",
    "delete-buffer (deleteb)", "detach-client (detach)",
    "display-menu (menu)", "display-message (display)",
    "display-panes (displayp)", "display-popup (popup)",
    "find-window (findw)", "has-session (has)",
    "if-shell (if)", "join-pane (joinp)",
    "kill-pane (killp)", "kill-server", "kill-session",
    "kill-window (killw)", "last-pane (lastp)", "last-window (last)",
    "link-window (linkw)", "list-buffers (lsb)", "list-clients (lsc)",
    "list-commands (lscm)", "list-keys (lsk)", "list-panes (lsp)",
    "list-sessions (ls)", "list-windows (lsw)",
    "load-buffer (loadb)", "lock-client (lockc)",
    "lock-server (lock)", "lock-session (locks)",
    "move-pane (movep)", "move-window (movew)",
    "new-session (new)", "new-window (neww)",
    "next-layout (nextl)", "next-window (next)",
    "paste-buffer (pasteb)", "pipe-pane (pipep)",
    "previous-layout (prevl)", "previous-window (prev)",
    "refresh-client (refresh)", "rename-session (rename)",
    "rename-window (renamew)", "resize-pane (resizep)",
    "resize-window (resizew)", "respawn-pane (respawnp)",
    "respawn-window (respawnw)", "rotate-window (rotatew)",
    "run-shell (run)", "save-buffer (saveb)",
    "select-layout (selectl)", "select-pane (selectp)",
    "select-window (selectw)", "send-keys (send)",
    "send-prefix", "server-info (info)",
    "set-buffer (setb)", "set-environment (setenv)",
    "set-hook", "set-option (set)",
    "set-window-option (setw)", "show-buffer (showb)",
    "show-environment (showenv)", "show-hooks",
    "show-messages (showmsgs)", "show-options (show)",
    "show-window-options (showw)", "source-file (source)",
    "split-window (splitw)", "start-server (start)",
    "suspend-client (suspendc)", "swap-pane (swapp)",
    "swap-window (swapw)", "switch-client (switchc)",
    "unbind-key (unbind)", "unlink-window (unlinkw)",
    "wait-for (wait)",
];

pub fn run_server(session_name: String, socket_name: Option<String>, initial_command: Option<String>, raw_command: Option<Vec<String>>) -> io::Result<()> {
    // Write crash info to a log file when stderr is unavailable (detached server)
    std::panic::set_hook(Box::new(|info| {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        let path = format!("{}\\.psmux\\crash.log", home);
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(&path, format!("{info}\n\nBacktrace:\n{bt}"));
    }));
    // Install console control handler to prevent termination on client detach
    install_console_ctrl_handler();

    let pty_system = PtySystemSelection::default()
        .get()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;

    let mut app = AppState::new(session_name);
    app.socket_name = socket_name;
    // Server starts detached with a reasonable default window size
    app.attached_clients = 0;
    load_config(&mut app);
    // Create initial window with optional command
    if let Some(ref raw_args) = raw_command {
        create_window_raw(&*pty_system, &mut app, raw_args)?;
    } else {
        create_window(&*pty_system, &mut app, initial_command.as_deref())?;
    }
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    app.control_rx = Some(rx);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    app.control_port = Some(port);
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let dir = format!("{}\\.psmux", home);
    let _ = std::fs::create_dir_all(&dir);
    
    // Generate a random session key for security
    let session_key: String = {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let s = RandomState::new();
        let mut h = s.build_hasher();
        h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64);
        h.write_u64(std::process::id() as u64);
        format!("{:016x}", h.finish())
    };
    
    // Write port and key to files (uses port_file_base for -L namespace support)
    let regpath = format!("{}\\{}.port", dir, app.port_file_base());
    let _ = std::fs::write(&regpath, port.to_string());
    let keypath = format!("{}\\{}.key", dir, app.port_file_base());
    let _ = std::fs::write(&keypath, &session_key);
    
    // Try to set file permissions to user-only (Windows)
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Recreate key file with restricted permissions
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&keypath)
            .map(|mut f| std::io::Write::write_all(&mut f, session_key.as_bytes()));
    }
    
    thread::spawn(move || {
        for conn in listener.incoming() {
            if let Ok(stream) = conn {
                let tx = tx.clone();
                let session_key_clone = session_key.clone();
                thread::spawn(move || {
                // Clone stream for writing, original goes into BufReader for reading
                let mut write_stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                
                // Set initial long timeout for auth
                let _ = stream.set_read_timeout(Some(Duration::from_millis(5000)));
                let mut r = io::BufReader::new(stream);
                
                // Read the authentication line
                let mut auth_line = String::new();
                if r.read_line(&mut auth_line).is_err() {
                    return;
                }
                
                // Verify session key
                let auth_line = auth_line.trim();
                if !auth_line.starts_with("AUTH ") {
                    // Legacy client without auth - reject for security
                    let _ = write_stream.write_all(b"ERROR: Authentication required\n");
                    let _ = write_stream.flush();
                    return;
                }
                let provided_key = auth_line.strip_prefix("AUTH ").unwrap_or("");
                if provided_key != session_key_clone {
                    let _ = write_stream.write_all(b"ERROR: Invalid session key\n");
                    let _ = write_stream.flush();
                    return;
                }
                // Auth successful - send OK and flush immediately
                let _ = write_stream.write_all(b"OK\n");
                let _ = write_stream.flush();
                
                // Set short read timeout for batched command processing
                let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(50)));
                
                // Check for PERSISTENT flag and optional TARGET line
                let mut persistent = false;
                let mut resp_tx_opt: Option<mpsc::Sender<mpsc::Receiver<String>>> = None;
                let mut global_target_win: Option<usize> = None;
                let mut global_target_pane: Option<usize> = None;
                let mut global_pane_is_id = false;
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                
                // Check if client requests persistent connection mode
                if line.trim() == "PERSISTENT" {
                    persistent = true;
                    // Enable TCP_NODELAY for low-latency persistent connections
                    let _ = r.get_ref().set_nodelay(true);
                    let _ = write_stream.set_nodelay(true);
                    // Use longer read timeout for persistent mode - client controls pacing
                    let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(5000)));
                    
                    // Spawn a dedicated writer thread so the read loop never blocks
                    // waiting for dump-state responses.  The read loop sends oneshot
                    // receivers here; the writer thread waits for each response and
                    // writes it to TCP in order.
                    let mut ws_bg = write_stream.try_clone().unwrap();
                    let (resp_tx, resp_rx) = mpsc::channel::<mpsc::Receiver<String>>();
                    std::thread::spawn(move || {
                        while let Ok(rrx) = resp_rx.recv() {
                            if let Ok(text) = rrx.recv() {
                                let _ = write!(ws_bg, "{}\n", text);
                                let _ = ws_bg.flush();
                            }
                        }
                    });
                    resp_tx_opt = Some(resp_tx);
                    line.clear();
                    if r.read_line(&mut line).is_err() {
                        return;
                    }
                }
                
                // Check if this line is a TARGET specification
                if line.trim().starts_with("TARGET ") {
                    let target_spec = line.trim().strip_prefix("TARGET ").unwrap_or("");
                    let parsed = parse_target(target_spec);
                    global_target_win = parsed.window;
                    global_target_pane = parsed.pane;
                    global_pane_is_id = parsed.pane_is_id;
                    // Now read the actual command line
                    line.clear();
                    if r.read_line(&mut line).is_err() {
                        return;
                    }
                }
                
                // Process commands in a loop to handle batching
                loop {
                    if line.trim().is_empty() {
                        // Try to read another command with timeout
                        line.clear();
                        match r.read_line(&mut line) {
                            Ok(0) => break, // EOF - client disconnected
                            Err(e) => {
                                // In persistent mode, timeouts are expected - keep waiting
                                if persistent && (e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut) {
                                    line.clear(); // Clear any partial data from interrupted read
                                    continue;
                                }
                                break; // Real error or non-persistent timeout
                            }
                            Ok(_) => continue, // Process the new line
                        }
                    }
                    
                    // Use quote-aware parser to preserve arguments with spaces
                    let parsed = parse_command_line(&line);
                    let cmd = parsed.get(0).map(|s| s.as_str()).unwrap_or("");
                    let args: Vec<&str> = parsed.iter().skip(1).map(|s| s.as_str()).collect();
                
                // Parse -t argument from command line (takes precedence over global TARGET)
                let mut target_win: Option<usize> = global_target_win;
                let mut target_pane: Option<usize> = global_target_pane;
                let mut pane_is_id = global_pane_is_id;
                let mut i = 0;
                while i < args.len() {
                    if args[i] == "-t" {
                        if let Some(v) = args.get(i+1) {
                            // Parse the -t value using parse_target for consistent handling
                            let pt = parse_target(v);
                            if pt.window.is_some() { target_win = pt.window; }
                            if pt.pane.is_some() { 
                                target_pane = pt.pane;
                                pane_is_id = pt.pane_is_id;
                            }
                        }
                        i += 2; continue;
                    }
                    i += 1;
                }
                // Build args without -t and its value so command handlers get clean positional args
                let args: Vec<&str> = {
                    let mut filtered = Vec::new();
                    let mut i = 0;
                    while i < args.len() {
                        if args[i] == "-t" {
                            i += 2; // skip -t and its value
                            continue;
                        }
                        filtered.push(args[i]);
                        i += 1;
                    }
                    filtered
                };
                if let Some(wid) = target_win { let _ = tx.send(CtrlReq::FocusWindow(wid)); }
                if let Some(pid) = target_pane { 
                    if pane_is_id {
                        let _ = tx.send(CtrlReq::FocusPane(pid));
                    } else {
                        let _ = tx.send(CtrlReq::FocusPaneByIndex(pid));
                    }
                }
                match cmd {
                    "new-window" | "neww" => {
                        let name: Option<String> = args.windows(2).find(|w| w[0] == "-n").map(|w| w[1].trim_matches('"').to_string());
                        let start_dir: Option<String> = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
                        let detached = args.iter().any(|a| *a == "-d");
                        let print_info = args.iter().any(|a| *a == "-P");
                        let format_str: Option<String> = args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].trim_matches('"').to_string());
                        let cmd_str: Option<String> = args.iter()
                            .find(|a| !a.starts_with('-') && args.windows(2).all(|w| !(w[0] == "-n" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-c" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-F" && w[1] == **a)))
                            .map(|s| s.trim_matches('"').to_string());
                        if print_info {
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::NewWindowPrint(cmd_str, name, detached, start_dir, format_str, rtx));
                            if let Ok(text) = rrx.recv_timeout(Duration::from_millis(2000)) {
                                let _ = write!(write_stream, "{}\n", text);
                                let _ = write_stream.flush();
                            }
                            if !persistent { break; }
                        } else {
                            let _ = tx.send(CtrlReq::NewWindow(cmd_str, name, detached, start_dir));
                        }
                    }
                    "split-window" | "splitw" => {
                        let kind = if args.iter().any(|a| *a == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                        let detached = args.iter().any(|a| *a == "-d");
                        let print_info = args.iter().any(|a| *a == "-P");
                        let format_str: Option<String> = args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].trim_matches('"').to_string());
                        let start_dir: Option<String> = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
                        let size_pct: Option<u16> = args.windows(2).find(|w| w[0] == "-p").and_then(|w| w[1].parse().ok())
                            .or_else(|| args.windows(2).find(|w| w[0] == "-l").and_then(|w| {
                                let s = w[1].trim_matches('%');
                                s.parse().ok()
                            }));
                        let cmd_str: Option<String> = args.iter()
                            .find(|a| !a.starts_with('-') && args.windows(2).all(|w| !(w[0] == "-c" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-p" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-l" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-F" && w[1] == **a)))
                            .map(|s| s.trim_matches('"').to_string());
                        if print_info {
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::SplitWindowPrint(kind, cmd_str, detached, start_dir, size_pct, format_str, rtx));
                            if let Ok(text) = rrx.recv_timeout(Duration::from_millis(2000)) {
                                let _ = write!(write_stream, "{}\n", text);
                                let _ = write_stream.flush();
                            }
                            if !persistent { break; }
                        } else {
                            let _ = tx.send(CtrlReq::SplitWindow(kind, cmd_str, detached, start_dir, size_pct));
                        }
                    }
                    "kill-pane" | "killp" => { let _ = tx.send(CtrlReq::KillPane); }
                    "capture-pane" | "capturep" => {
                        let print_stdout = args.iter().any(|a| *a == "-p");
                        let join_lines = args.iter().any(|a| *a == "-J");
                        let escape_seqs = args.iter().any(|a| *a == "-e");
                        // Parse -S start and -E end (negative = scrollback offset, - = entire scrollback)
                        let s_arg = args.windows(2).find(|w| w[0] == "-S").map(|w| w[1]);
                        let e_arg = args.windows(2).find(|w| w[0] == "-E").map(|w| w[1]);
                        let (rtx, rrx) = mpsc::channel::<String>();
                        if escape_seqs {
                            let _ = tx.send(CtrlReq::CapturePaneStyled(rtx));
                        } else if s_arg.is_some() || e_arg.is_some() {
                            let start: Option<u16> = match s_arg {
                                Some("-") => Some(0), // entire scrollback start
                                Some(v) => v.parse::<u16>().ok(),
                                None => None,
                            };
                            let end: Option<u16> = match e_arg {
                                Some("-") => None, // to end of visible
                                Some(v) => v.parse::<u16>().ok(),
                                None => None,
                            };
                            let _ = tx.send(CtrlReq::CapturePaneRange(rtx, start, end));
                        } else {
                            let _ = tx.send(CtrlReq::CapturePane(rtx));
                        }
                        if let Ok(mut text) = rrx.recv() {
                            if join_lines {
                                // Remove trailing whitespace from each line (join wrapped lines)
                                text = text.lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
                            }
                            if print_stdout {
                                let _ = write!(write_stream, "{}\n", text);
                                let _ = write_stream.flush();
                                if !persistent { break; }
                            } else {
                                let _ = tx.send(CtrlReq::SetBuffer(text));
                            }
                        }
                    }
                    "dump-layout" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::DumpLayout(rtx));
                        if let Ok(text) = rrx.recv() { 
                            let _ = write!(write_stream, "{}\n", text); 
                            let _ = write_stream.flush();
                        }
                        if !persistent { break; }
                    }
                    "dump-state" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::DumpState(rtx));
                        if let Some(ref rtx_bg) = resp_tx_opt {
                            // Persistent mode: hand off to writer thread (non-blocking).
                            // This lets the read loop keep processing keys immediately.
                            let _ = rtx_bg.send(rrx);
                        } else {
                            // One-shot mode: block and respond inline
                            if let Ok(text) = rrx.recv() { 
                                let _ = write!(write_stream, "{}\n", text); 
                                let _ = write_stream.flush();
                            }
                            if !persistent { break; }
                        }
                    }
                    "send-text" => {
                        if let Some(payload) = args.get(0) { let _ = tx.send(CtrlReq::SendText(payload.to_string())); }
                    }
                    "send-paste" => {
                        if let Some(encoded) = args.get(0) {
                            if let Some(decoded) = base64_decode(encoded) {
                                let _ = tx.send(CtrlReq::SendPaste(decoded));
                            }
                        }
                    }
                    "send-key" => {
                        if let Some(payload) = args.get(0) { let _ = tx.send(CtrlReq::SendKey(payload.to_string())); }
                    }
                    "zoom-pane" | "resize-pane" | "resizep" if args.iter().any(|a| *a == "-Z") => { let _ = tx.send(CtrlReq::ZoomPane); }
                    "zoom-pane" => { let _ = tx.send(CtrlReq::ZoomPane); }
                    "copy-enter" => { let _ = tx.send(CtrlReq::CopyEnter); }
                    "copy-move" => {
                        if args.len() >= 2 { if let (Ok(dx), Ok(dy)) = (args[0].parse::<i16>(), args[1].parse::<i16>()) { let _ = tx.send(CtrlReq::CopyMove(dx, dy)); } }
                    }
                    "copy-anchor" => { let _ = tx.send(CtrlReq::CopyAnchor); }
                    "copy-yank" => { let _ = tx.send(CtrlReq::CopyYank); }
                    "client-size" => {
                        if args.len() >= 2 { if let (Ok(w), Ok(h)) = (args[0].parse::<u16>(), args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::ClientSize(w, h)); } }
                    }
                    "focus-pane" => {
                        if let Some(pid) = args.get(0).and_then(|s| s.parse::<usize>().ok()) { let _ = tx.send(CtrlReq::FocusPaneCmd(pid)); }
                    }
                    "focus-window" => {
                        if let Some(wid) = args.get(0).and_then(|s| s.parse::<usize>().ok()) { let _ = tx.send(CtrlReq::FocusWindowCmd(wid)); }
                    }
                    "mouse-down" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDown(x,y)); } }
                    }
                    "mouse-down-right" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDownRight(x,y)); } }
                    }
                    "mouse-down-middle" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDownMiddle(x,y)); } }
                    }
                    "mouse-drag" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDrag(x,y)); } }
                    }
                    "mouse-up" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUp(x,y)); } }
                    }
                    "mouse-up-right" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUpRight(x,y)); } }
                    }
                    "mouse-up-middle" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUpMiddle(x,y)); } }
                    }
                    "mouse-move" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseMove(x,y)); } }
                    }
                    "scroll-up" => {
                        let x = args.get(0).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
                        let y = args.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
                        let _ = tx.send(CtrlReq::ScrollUp(x, y));
                    }
                    "scroll-down" => {
                        let x = args.get(0).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
                        let y = args.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
                        let _ = tx.send(CtrlReq::ScrollDown(x, y));
                    }
                    "next-window" | "next" => { let _ = tx.send(CtrlReq::NextWindow); }
                    "previous-window" | "prev" => { let _ = tx.send(CtrlReq::PrevWindow); }
                    "rename-window" | "renamew" => { if let Some(name) = args.get(0) { let _ = tx.send(CtrlReq::RenameWindow((*name).to_string())); } }
                    "list-windows" | "lsw" => {
                        // Extract -F format if provided
                        let fmt = args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].to_string());
                        if let Some(fmt_str) = fmt {
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::ListWindowsFormat(rtx, fmt_str));
                            if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        } else if args.iter().any(|a| *a == "-J") {
                            // JSON output for programmatic use
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::ListWindows(rtx));
                            if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        } else {
                            // tmux-compatible text output (default)
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::ListWindowsTmux(rtx));
                            if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        }
                        if !persistent { break; }
                    }
                    "list-tree" => { let (rtx, rrx) = mpsc::channel::<String>(); let _ = tx.send(CtrlReq::ListTree(rtx)); if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); } if !persistent { break; } }
                    "toggle-sync" => { let _ = tx.send(CtrlReq::ToggleSync); }
                    "set-pane-title" => { let title = args.join(" "); let _ = tx.send(CtrlReq::SetPaneTitle(title)); }
                    "send-keys" => {
                        let literal = args.iter().any(|a| *a == "-l");
                        let has_x = args.iter().any(|a| *a == "-X");
                        // Parse -N <count> for repeat
                        let mut repeat_count: usize = 1;
                        if let Some(n_pos) = args.iter().position(|a| *a == "-N") {
                            if let Some(count_str) = args.get(n_pos + 1) {
                                repeat_count = count_str.parse::<usize>().unwrap_or(1).max(1);
                            }
                        }
                        if has_x {
                            // send-keys -X copy-mode-command
                            let cmd_parts: Vec<&str> = args.iter().filter(|a| **a != "-X" && !a.starts_with('-')).copied().collect();
                            for _ in 0..repeat_count {
                                let _ = tx.send(CtrlReq::SendKeysX(cmd_parts.join(" ")));
                            }
                        } else {
                            let keys: Vec<&str> = args.iter()
                                .enumerate()
                                .filter(|(i, a)| {
                                    !a.starts_with('-') && **a != "-l" && **a != "-t"
                                    // Skip the argument to -N
                                    && !(i > &0 && args.get(i - 1).map_or(false, |prev| *prev == "-N"))
                                })
                                .map(|(_, a)| *a)
                                .collect();
                            for _ in 0..repeat_count {
                                let _ = tx.send(CtrlReq::SendKeys(keys.join(" "), literal));
                            }
                        }
                    }
                    "select-pane" | "selectp" => {
                        let dir = if args.iter().any(|a| *a == "-U") { "U" }
                            else if args.iter().any(|a| *a == "-D") { "D" }
                            else if args.iter().any(|a| *a == "-L") { "L" }
                            else if args.iter().any(|a| *a == "-R") { "R" }
                            else if args.iter().any(|a| *a == "-l") { "last" }
                            else if args.iter().any(|a| *a == "-m") { "mark" }
                            else if args.iter().any(|a| *a == "-M") { "unmark" }
                            else if args.iter().any(|a| *a == "-e") { "enable-input" }
                            else if args.iter().any(|a| *a == "-d") { "disable-input" }
                            else { "" };
                        // Check for -T title
                        let title = args.windows(2).find(|w| w[0] == "-T").map(|w| w[1].to_string());
                        if let Some(t) = title {
                            let _ = tx.send(CtrlReq::SetPaneTitle(t));
                        }
                        let _ = tx.send(CtrlReq::SelectPane(dir.to_string()));
                    }
                    "select-window" | "selectw" => {
                        if let Some(idx) = args.iter().find(|a| !a.starts_with('-')).and_then(|s| s.parse::<usize>().ok()) {
                            let _ = tx.send(CtrlReq::SelectWindow(idx));
                        }
                    }
                    "list-panes" | "lsp" => {
                        let fmt = args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].to_string());
                        let all = args.iter().any(|a| *a == "-a" || *a == "-s");
                        let (rtx, rrx) = mpsc::channel::<String>();
                        if let Some(fmt_str) = fmt {
                            if all {
                                let _ = tx.send(CtrlReq::ListAllPanesFormat(rtx, fmt_str));
                            } else {
                                let _ = tx.send(CtrlReq::ListPanesFormat(rtx, fmt_str));
                            }
                        } else {
                            if all {
                                let _ = tx.send(CtrlReq::ListAllPanes(rtx));
                            } else {
                                let _ = tx.send(CtrlReq::ListPanes(rtx));
                            }
                        }
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "kill-window" | "killw" => { let _ = tx.send(CtrlReq::KillWindow); }
                    "kill-session" => { let _ = tx.send(CtrlReq::KillSession); }
                    "has-session" => {
                        let (rtx, rrx) = mpsc::channel::<bool>();
                        let _ = tx.send(CtrlReq::HasSession(rtx));
                        if let Ok(exists) = rrx.recv() {
                            if !exists { std::process::exit(1); }
                        }
                    }
                    "rename-session" | "rename" => {
                        if let Some(name) = args.iter().find(|a| !a.starts_with('-')) {
                            let _ = tx.send(CtrlReq::RenameSession((*name).to_string()));
                        }
                    }
                    "swap-pane" | "swapp" => {
                        let dir = if args.iter().any(|a| *a == "-U") { "U" }
                            else if args.iter().any(|a| *a == "-D") { "D" }
                            else { "D" };
                        let _ = tx.send(CtrlReq::SwapPane(dir.to_string()));
                    }
                    "resize-pane" | "resizep" => {
                        // Check for zoom toggle first (issue #35)
                        if args.iter().any(|a| *a == "-Z") {
                            let _ = tx.send(CtrlReq::ZoomPane);
                        } else
                        // Check for absolute resize (-x N or -y N)
                        if let Some(xv) = args.windows(2).find(|w| w[0] == "-x").and_then(|w| w[1].parse::<u16>().ok()) {
                            let _ = tx.send(CtrlReq::ResizePaneAbsolute("x".to_string(), xv));
                        } else if let Some(yv) = args.windows(2).find(|w| w[0] == "-y").and_then(|w| w[1].parse::<u16>().ok()) {
                            let _ = tx.send(CtrlReq::ResizePaneAbsolute("y".to_string(), yv));
                        } else {
                            let amount = args.iter().find(|a| a.parse::<u16>().is_ok()).and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
                            let dir = if args.iter().any(|a| *a == "-U") { "U" }
                                else if args.iter().any(|a| *a == "-D") { "D" }
                                else if args.iter().any(|a| *a == "-L") { "L" }
                                else if args.iter().any(|a| *a == "-R") { "R" }
                                else { "D" };
                            let _ = tx.send(CtrlReq::ResizePane(dir.to_string(), amount));
                        }
                    }
                    "set-buffer" => {
                        let content = args.iter().filter(|a| !a.starts_with('-')).cloned().collect::<Vec<&str>>().join(" ");
                        let _ = tx.send(CtrlReq::SetBuffer(content));
                    }
                    "paste-buffer" | "pasteb" => {
                        let buf_idx: Option<usize> = args.windows(2).find(|w| w[0] == "-b").and_then(|w| w[1].parse().ok());
                        let (rtx, rrx) = mpsc::channel::<String>();
                        if let Some(idx) = buf_idx {
                            let _ = tx.send(CtrlReq::ShowBufferAt(rtx, idx));
                        } else {
                            let _ = tx.send(CtrlReq::ShowBuffer(rtx));
                        }
                        if let Ok(text) = rrx.recv() {
                            let _ = tx.send(CtrlReq::SendText(text));
                        }
                    }
                    "list-buffers" | "lsb" => {
                        let fmt = args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].to_string());
                        let (rtx, rrx) = mpsc::channel::<String>();
                        if let Some(fmt_str) = fmt {
                            let _ = tx.send(CtrlReq::ListBuffersFormat(rtx, fmt_str));
                        } else {
                            let _ = tx.send(CtrlReq::ListBuffers(rtx));
                        }
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "show-buffer" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowBuffer(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "delete-buffer" => { let _ = tx.send(CtrlReq::DeleteBuffer); }
                    "choose-buffer" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ChooseBuffer(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "display-message" | "display" => {
                        let fmt = args.iter().filter(|a| !a.starts_with('-')).cloned().collect::<Vec<&str>>().join(" ");
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt));
                        if let Ok(text) = rrx.recv() { let _ = writeln!(write_stream, "{}", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "last-window" | "last" => { let _ = tx.send(CtrlReq::LastWindow); }
                    "last-pane" | "lastp" => { let _ = tx.send(CtrlReq::LastPane); }
                    "rotate-window" | "rotatew" => {
                        let reverse = args.iter().any(|a| *a == "-D");
                        let _ = tx.send(CtrlReq::RotateWindow(reverse));
                    }
                    "display-panes" | "displayp" => { let _ = tx.send(CtrlReq::DisplayPanes); }
                    "break-pane" | "breakp" => { let _ = tx.send(CtrlReq::BreakPane); }
                    "join-pane" | "joinp" => {
                        if let Some(wid) = args.iter().find(|a| !a.starts_with('-')).and_then(|s| s.parse::<usize>().ok()) {
                            let _ = tx.send(CtrlReq::JoinPane(wid));
                        }
                    }
                    "respawn-pane" | "respawnp" => { let _ = tx.send(CtrlReq::RespawnPane); }
                    "session-info" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::SessionInfo(rtx));
                        if let Ok(line) = rrx.recv() { let _ = write!(write_stream, "{}\n", line); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "client-attach" => { let _ = tx.send(CtrlReq::ClientAttach); if !persistent { let _ = write!(write_stream, "ok\n"); } }
                    "client-detach" => { let _ = tx.send(CtrlReq::ClientDetach); if !persistent { let _ = write!(write_stream, "ok\n"); } }
                    "bind-key" | "bind" => {
                        let mut table = "prefix".to_string();
                        let mut repeatable = false;
                        // Parse bind-key's own flags first, then extract key + command.
                        // bind-key flags: -T <table>, -n (root), -r (repeatable)
                        // Everything after the key is the command string verbatim.
                        let mut i = 0;
                        while i < args.len() {
                            match args[i] {
                                "-T" if i + 1 < args.len() => {
                                    table = args[i + 1].to_string();
                                    i += 2; continue;
                                }
                                "-n" => { table = "root".to_string(); i += 1; continue; }
                                "-r" => { repeatable = true; i += 1; continue; }
                                _ => break, // First non-flag arg = the key
                            }
                        }
                        // args[i] = the key, args[i+1..] = the command (preserve all flags)
                        if i < args.len() && i + 1 < args.len() {
                            let key = args[i].to_string();
                            let command = args[i + 1..].join(" ");
                            let _ = tx.send(CtrlReq::BindKey(table, key, command, repeatable));
                        }
                    }
                    "unbind-key" | "unbind" => {
                        let non_flag_args: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if let Some(key) = non_flag_args.first() {
                            let _ = tx.send(CtrlReq::UnbindKey(key.to_string()));
                        }
                    }
                    "list-keys" | "lsk" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ListKeys(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "set-option" | "set" | "set-window-option" | "setw" => {
                        let has_u = args.iter().any(|a| *a == "-u");
                        let has_a = args.iter().any(|a| *a == "-a");
                        let has_q = args.iter().any(|a| *a == "-q");
                        let non_flag_args: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if has_u {
                            if let Some(option) = non_flag_args.first() {
                                let _ = tx.send(CtrlReq::SetOptionUnset(option.to_string()));
                            }
                        } else if non_flag_args.len() >= 2 {
                            let option = non_flag_args[0].to_string();
                            let value = non_flag_args[1..].join(" ");
                            if has_a {
                                let _ = tx.send(CtrlReq::SetOptionAppend(option, value));
                            } else {
                                let _ = tx.send(CtrlReq::SetOptionQuiet(option, value, has_q));
                            }
                        } else if non_flag_args.len() == 1 && has_q {
                            // set -q <option> with no value — silently ignore
                        }
                    }
                    "show-options" | "show" | "show-window-options" | "showw" => {
                        let has_v = args.iter().any(|a| *a == "-v");
                        let has_q = args.iter().any(|a| *a == "-q");
                        let opt_name: Option<&str> = args.iter()
                            .filter(|a| !a.starts_with('-'))
                            .copied()
                            .last();
                        if has_v || (opt_name.is_some() && !has_q) {
                            // Single-option query: show-options -v <name> or show <name>
                            if let Some(name) = opt_name {
                                let (rtx, rrx) = mpsc::channel::<String>();
                                let _ = tx.send(CtrlReq::ShowOptionValue(rtx, name.to_string()));
                                if let Ok(text) = rrx.recv() {
                                    if has_v {
                                        let _ = write!(write_stream, "{}\n", text);
                                    } else {
                                        let _ = write!(write_stream, "{} {}\n", name, text);
                                    }
                                    let _ = write_stream.flush();
                                }
                            }
                        } else {
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::ShowOptions(rtx));
                            if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        }
                        if !persistent { break; }
                    }
                    "source-file" | "source" => {
                        let non_flag_args: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if let Some(path) = non_flag_args.first() {
                            let _ = tx.send(CtrlReq::SourceFile(path.to_string()));
                        }
                    }
                    "move-window" | "movew" => {
                        let target = args.iter().find(|a| a.parse::<usize>().is_ok()).and_then(|s| s.parse().ok());
                        let _ = tx.send(CtrlReq::MoveWindow(target));
                    }
                    "swap-window" | "swapw" => {
                        if let Some(target) = args.iter().find(|a| a.parse::<usize>().is_ok()).and_then(|s| s.parse().ok()) {
                            let _ = tx.send(CtrlReq::SwapWindow(target));
                        }
                    }
                    "link-window" | "linkw" => {
                        let target = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::LinkWindow(target));
                    }
                    "unlink-window" | "unlinkw" => {
                        let _ = tx.send(CtrlReq::UnlinkWindow);
                    }
                    "find-window" | "findw" => {
                        let pattern = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::FindWindow(rtx, pattern));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "move-pane" | "movep" => {
                        if let Some(target) = args.iter().find(|a| a.parse::<usize>().is_ok()).and_then(|s| s.parse().ok()) {
                            let _ = tx.send(CtrlReq::MovePane(target));
                        }
                    }
                    "pipe-pane" | "pipep" => {
                        let stdin_flag = args.iter().any(|a| *a == "-I");
                        let stdout_flag = args.iter().any(|a| *a == "-O");
                        let _toggle = args.iter().any(|a| *a == "-o");
                        let cmd = args.iter().filter(|a| !a.starts_with('-')).cloned().collect::<Vec<&str>>().join(" ");
                        let (stdin, stdout) = if !stdin_flag && !stdout_flag {
                            (false, true)
                        } else {
                            (stdin_flag, stdout_flag)
                        };
                        let _ = tx.send(CtrlReq::PipePane(cmd, stdin, stdout));
                    }
                    "select-layout" | "selectl" => {
                        let layout = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"tiled").to_string();
                        let _ = tx.send(CtrlReq::SelectLayout(layout));
                    }
                    "next-layout" => {
                        let _ = tx.send(CtrlReq::NextLayout);
                    }
                    "list-clients" | "lsc" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ListClients(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "switch-client" | "switchc" => {
                        let target = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::SwitchClient(target));
                    }
                    "lock-client" => {
                        let _ = tx.send(CtrlReq::LockClient);
                    }
                    "refresh-client" => {
                        let _ = tx.send(CtrlReq::RefreshClient);
                    }
                    "suspend-client" => {
                        let _ = tx.send(CtrlReq::SuspendClient);
                    }
                    "copy-mode-page-up" => {
                        let _ = tx.send(CtrlReq::CopyModePageUp);
                    }
                    "clear-history" => {
                        let _ = tx.send(CtrlReq::ClearHistory);
                    }
                    "save-buffer" | "saveb" => {
                        let path = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::SaveBuffer(path));
                    }
                    "load-buffer" | "loadb" => {
                        let path = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::LoadBuffer(path));
                    }
                    "set-environment" | "setenv" => {
                        let non_flag: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if non_flag.len() >= 2 {
                            let _ = tx.send(CtrlReq::SetEnvironment(non_flag[0].to_string(), non_flag[1].to_string()));
                        } else if non_flag.len() == 1 {
                            let _ = tx.send(CtrlReq::SetEnvironment(non_flag[0].to_string(), String::new()));
                        }
                    }
                    "show-environment" | "showenv" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowEnvironment(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "set-hook" => {
                        let non_flag: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if non_flag.len() >= 2 {
                            let _ = tx.send(CtrlReq::SetHook(non_flag[0].to_string(), non_flag[1..].join(" ")));
                        }
                    }
                    "show-hooks" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowHooks(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "wait-for" => {
                        let lock = args.iter().any(|a| *a == "-L");
                        let signal = args.iter().any(|a| *a == "-S");
                        let unlock = args.iter().any(|a| *a == "-U");
                        let channel = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let op = if lock { WaitForOp::Lock }
                            else if signal { WaitForOp::Signal }
                            else if unlock { WaitForOp::Unlock }
                            else { WaitForOp::Wait };
                        let _ = tx.send(CtrlReq::WaitFor(channel, op));
                    }
                    "display-menu" | "menu" => {
                        let mut x_pos: Option<i16> = None;
                        let mut y_pos: Option<i16> = None;
                        let mut i = 0;
                        while i < args.len() {
                            match args[i] {
                                "-x" => { if let Some(v) = args.get(i+1) { x_pos = v.parse().ok(); i += 1; } }
                                "-y" => { if let Some(v) = args.get(i+1) { y_pos = v.parse().ok(); i += 1; } }
                                _ => {}
                            }
                            i += 1;
                        }
                        let menu = args.iter().filter(|a| !a.starts_with('-') && a.parse::<i16>().is_err()).cloned().collect::<Vec<&str>>().join(" ");
                        let _ = tx.send(CtrlReq::DisplayMenu(menu, x_pos, y_pos));
                    }
                    "display-popup" | "popup" => {
                        let close_on_exit = args.iter().any(|a| *a == "-E");
                        let mut width: u16 = 80;
                        let mut height: u16 = 24;
                        let mut i = 0;
                        while i < args.len() {
                            match args[i] {
                                "-w" => { if let Some(v) = args.get(i+1) { width = v.trim_end_matches('%').parse().unwrap_or(80); i += 1; } }
                                "-h" => { if let Some(v) = args.get(i+1) { height = v.trim_end_matches('%').parse().unwrap_or(24); i += 1; } }
                                _ => {}
                            }
                            i += 1;
                        }
                        let content = args.iter().filter(|a| !a.starts_with('-')).cloned().collect::<Vec<&str>>().join(" ");
                        let _ = tx.send(CtrlReq::DisplayPopup(content, width, height, close_on_exit));
                    }
                    "confirm-before" | "confirm" => {
                        let mut prompt: Option<String> = None;
                        let mut i = 0;
                        while i < args.len() {
                            if args[i] == "-p" {
                                if let Some(p) = args.get(i+1) { prompt = Some(p.to_string()); i += 1; }
                            }
                            i += 1;
                        }
                        let non_flag: Vec<&str> = args.iter().filter(|a| !a.starts_with('-') && Some(&a.to_string()) != prompt.as_ref()).copied().collect();
                        let command = non_flag.join(" ");
                        let prompt_str = prompt.unwrap_or_else(|| format!("Run '{}'?", command));
                        let _ = tx.send(CtrlReq::ConfirmBefore(prompt_str, command));
                    }
                    // tmux standard aliases
                    "detach-client" | "detach" => { let _ = tx.send(CtrlReq::ClientDetach); }
                    "attach-session" | "attach" => { let _ = tx.send(CtrlReq::ClientAttach); }
                    "kill-server" => { let _ = tx.send(CtrlReq::KillServer); }
                    "choose-tree" | "choose-window" | "choose-session" => {
                        // These are interactive choosers — send a dump that client handles
                        // For now, map to listing which the client renders as a chooser
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ListTree(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "copy-mode" => {
                        if args.iter().any(|a| *a == "-u") {
                            let _ = tx.send(CtrlReq::CopyEnterPageUp);
                        } else {
                            let _ = tx.send(CtrlReq::CopyEnter);
                        }
                    }
                    "clock-mode" => { let _ = tx.send(CtrlReq::ClockMode); }
                    "show-messages" | "showmsgs" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowMessages(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "command-prompt" => {
                        let initial = args.windows(2).find(|w| w[0] == "-I").map(|w| w[1].to_string()).unwrap_or_default();
                        let _ = tx.send(CtrlReq::CommandPrompt(initial));
                    }
                    "run-shell" | "run" => {
                        let background = args.iter().any(|a| *a == "-b");
                        let cmd_parts: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        let shell_cmd = cmd_parts.join(" ");
                        let shell_cmd = shell_cmd.trim_matches(|c: char| c == '\'' || c == '"').to_string();
                        if !shell_cmd.is_empty() {
                            if background {
                                let _ = std::process::Command::new("cmd").args(["/C", &shell_cmd]).spawn();
                            } else {
                                let output = std::process::Command::new("cmd")
                                    .args(["/C", &shell_cmd]).output();
                                if let Ok(out) = output {
                                    let text = String::from_utf8_lossy(&out.stdout);
                                    if !text.is_empty() {
                                        let _ = write!(write_stream, "{}", text);
                                        let _ = write_stream.flush();
                                    }
                                }
                            }
                        }
                    }
                    "if-shell" | "if" => {
                        // Re-parse from the original line to preserve quoted arguments
                        let if_parsed = parse_command_line(line.trim());
                        let format_mode = if_parsed.iter().any(|a| a == "-F" || a == "-bF" || a == "-Fb");
                        // Collect positional args (skip command name and flags)
                        let positional: Vec<&str> = if_parsed.iter().skip(1)
                            .filter(|a| !a.starts_with('-'))
                            .map(|s| s.as_str())
                            .collect();
                        if positional.len() >= 2 {
                            let condition = positional[0];
                            let true_cmd = positional[1];
                            let false_cmd = positional.get(2).copied();
                            let success = if format_mode {
                                !condition.is_empty() && condition != "0"
                            } else {
                                std::process::Command::new("cmd")
                                    .args(["/C", condition])
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .status()
                                    .map(|s| s.success()).unwrap_or(false)
                            };
                            let cmd_to_run = if success { Some(true_cmd) } else { false_cmd };
                            if let Some(chosen) = cmd_to_run {
                                // Feed the chosen command back into the line buffer so the
                                // main dispatch loop processes it as a regular command.
                                line.clear();
                                line.push_str(chosen);
                                line.push('\n');
                                continue;  // re-enter the dispatch loop with the new command
                            }
                        }
                    }
                    "list-sessions" | "ls" => {
                        let fmt = args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].to_string());
                        if let Some(fmt_str) = fmt {
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt_str));
                            if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        } else {
                            let (rtx, rrx) = mpsc::channel::<String>();
                            let _ = tx.send(CtrlReq::SessionInfo(rtx));
                            if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        }
                        if !persistent { break; }
                    }
                    "new-session" | "new" => {
                        // Accept but ignore in server context (requires a new process)
                    }
                    "list-commands" | "lscm" => {
                        let cmds = TMUX_COMMANDS.join("\n");
                        let _ = write!(write_stream, "{}\n", cmds);
                        let _ = write_stream.flush();
                        if !persistent { break; }
                    }
                    "server-info" | "info" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ServerInfo(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "start-server" => {
                        // Server is already running if we're here, no-op
                        if !persistent { break; }
                    }
                    "send-prefix" => {
                        let _ = tx.send(CtrlReq::SendPrefix);
                    }
                    "previous-layout" | "prevl" => {
                        let _ = tx.send(CtrlReq::PrevLayout);
                    }
                    "resize-window" | "resizew" => {
                        let abs_x = args.windows(2).find(|w| w[0] == "-x").and_then(|w| w[1].parse::<u16>().ok());
                        let abs_y = args.windows(2).find(|w| w[0] == "-y").and_then(|w| w[1].parse::<u16>().ok());
                        if let Some(xv) = abs_x {
                            let _ = tx.send(CtrlReq::ResizeWindow("x".to_string(), xv));
                        } else if let Some(yv) = abs_y {
                            let _ = tx.send(CtrlReq::ResizeWindow("y".to_string(), yv));
                        }
                    }
                    "respawn-window" | "respawnw" => {
                        let _ = tx.send(CtrlReq::RespawnWindow);
                    }
                    "lock-server" | "lock-session" | "lock" => {
                        // Lock is a no-op on Windows (no terminal locking concept)
                        // Stub for compatibility
                    }
                    "focus-in" => { let _ = tx.send(CtrlReq::FocusIn); }
                    "focus-out" => { let _ = tx.send(CtrlReq::FocusOut); }
                    "choose-client" => {
                        // Single-client model — choose-client is a no-op
                    }
                    "customize-mode" => {
                        // tmux 3.2+ customize-mode — stub for compatibility
                    }
                    _ => {}
                }
                    // Try to read next command for batching (with timeout)
                    line.clear();
                    match r.read_line(&mut line) {
                        Ok(0) => break, // EOF - client disconnected
                        Err(e) => {
                            if persistent && (e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut) {
                                line.clear(); // Clear any partial data from interrupted read
                                continue; // Persistent mode - keep waiting
                            }
                            break; // Non-persistent timeout or real error
                        }
                        Ok(_) => {} // Continue processing
                    }
                } // end command loop
                }); // end per-connection thread
            }
        }
    });
    let mut state_dirty = true;
    let mut cached_dump_state = String::new();
    let mut cached_data_version: u64 = 0;
    // Cached metadata JSON — windows/tree/prefix change only on structural
    // mutations, so we rebuild them lazily via `meta_dirty`.
    let mut meta_dirty = true;
    let mut cached_windows_json = String::new();
    let mut cached_tree_json = String::new();
    let mut cached_prefix_str = String::new();
    let mut cached_base_index: usize = 0;
    let mut cached_pred_dim: bool = false;
    let mut cached_status_style = String::new();
    let mut cached_bindings_json = String::from("[]");
    // Reusable buffer for building the combined JSON envelope.
    let mut combined_buf = String::with_capacity(32768);

    /// Sum data_version counters across all panes in the active window.
    fn combined_data_version(app: &AppState) -> u64 {
        let mut v = 0u64;
        fn walk(node: &Node, v: &mut u64) {
            match node {
                Node::Leaf(p) => { *v = v.wrapping_add(p.data_version.load(std::sync::atomic::Ordering::Acquire)); }
                Node::Split { children, .. } => { for c in children { walk(c, v); } }
            }
        }
        if let Some(win) = app.windows.get(app.active_idx) {
            walk(&win.root, &mut v);
        }
        v
    }

    /// Per-window data version for activity detection
    fn window_data_version(win: &Window) -> u64 {
        let mut v = 0u64;
        fn walk(node: &Node, v: &mut u64) {
            match node {
                Node::Leaf(p) => { *v = v.wrapping_add(p.data_version.load(std::sync::atomic::Ordering::Acquire)); }
                Node::Split { children, .. } => { for c in children { walk(c, v); } }
            }
        }
        walk(&win.root, &mut v);
        v
    }

    /// Get a single option's value by name (for `show-options -v name`).
    fn get_option_value(app: &AppState, name: &str) -> String {
        match name {
            "prefix" => format_key_binding(&app.prefix_key),
            "base-index" => app.window_base_index.to_string(),
            "pane-base-index" => app.pane_base_index.to_string(),
            "escape-time" => app.escape_time_ms.to_string(),
            "mouse" => if app.mouse_enabled { "on".into() } else { "off".into() },
            "status" => if app.status_visible { "on".into() } else { "off".into() },
            "status-position" => app.status_position.clone(),
            "status-left" => app.status_left.clone(),
            "status-right" => app.status_right.clone(),
            "history-limit" => app.history_limit.to_string(),
            "display-time" => app.display_time_ms.to_string(),
            "display-panes-time" => app.display_panes_time_ms.to_string(),
            "mode-keys" => app.mode_keys.clone(),
            "focus-events" => if app.focus_events { "on".into() } else { "off".into() },
            "renumber-windows" => if app.renumber_windows { "on".into() } else { "off".into() },
            "automatic-rename" => if app.automatic_rename { "on".into() } else { "off".into() },
            "monitor-activity" => if app.monitor_activity { "on".into() } else { "off".into() },
            "synchronize-panes" => if app.sync_input { "on".into() } else { "off".into() },
            "remain-on-exit" => if app.remain_on_exit { "on".into() } else { "off".into() },
            "set-titles" => if app.set_titles { "on".into() } else { "off".into() },
            "set-titles-string" => app.set_titles_string.clone(),
            "prediction-dimming" => if app.prediction_dimming { "on".into() } else { "off".into() },
            "cursor-style" => std::env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string()),
            "cursor-blink" => if std::env::var("PSMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0" { "on".into() } else { "off".into() },
            "default-shell" | "default-command" => app.default_shell.clone(),
            "word-separators" => app.word_separators.clone(),
            "pane-border-style" => app.pane_border_style.clone(),
            "pane-active-border-style" => app.pane_active_border_style.clone(),
            "status-style" => app.status_style.clone(),
            "window-status-format" => app.window_status_format.clone(),
            "window-status-current-format" => app.window_status_current_format.clone(),
            "window-status-separator" => app.window_status_separator.clone(),
            "window-status-style" => app.window_status_style.clone(),
            "window-status-current-style" => app.window_status_current_style.clone(),
            "window-status-activity-style" => app.window_status_activity_style.clone(),
            "window-status-bell-style" => app.window_status_bell_style.clone(),
            "window-status-last-style" => app.window_status_last_style.clone(),
            "message-style" => app.message_style.clone(),
            "message-command-style" => app.message_command_style.clone(),
            "mode-style" => app.mode_style.clone(),
            "status-left-style" => app.status_left_style.clone(),
            "status-right-style" => app.status_right_style.clone(),
            "status-interval" => app.status_interval.to_string(),
            "status-justify" => app.status_justify.clone(),
            "bell-action" => app.bell_action.clone(),
            "visual-bell" => if app.visual_bell { "on".into() } else { "off".into() },
            "monitor-silence" => app.monitor_silence.to_string(),
            _ => {
                // Support @user-options (stored in environment)
                if name.starts_with('@') {
                    app.environment.get(name).cloned().unwrap_or_default()
                } else {
                    String::new()
                }
            }
        }
    }
    /// Check non-active windows for output activity and set their activity_flag
    fn check_window_activity(app: &mut AppState) {
        if !app.monitor_activity { return; }
        let active = app.active_idx;
        for (i, win) in app.windows.iter_mut().enumerate() {
            if i == active {
                // Active window: clear flag, update version
                win.activity_flag = false;
                win.last_seen_version = window_data_version(win);
                continue;
            }
            let cur = window_data_version(win);
            if cur != win.last_seen_version {
                win.activity_flag = true;
                win.last_seen_version = cur;
            }
        }
    }

    // Track when we recently sent keystrokes to the PTY.  While waiting
    // for the echo to appear we use a much shorter recv_timeout (1ms vs 5ms)
    // so that dump-state requests are served with minimal delay.  This is
    // critical for nested-shell latency (e.g. WSL inside pwsh) where the
    // echo path goes through ConPTY → pwsh → WSL → echo → ConPTY and can
    // take 10-30ms.  Without this, each "no-change" polling cycle costs up
    // to 5ms, adding cumulative latency visible as heavy input lag.
    let mut echo_pending_until: Option<Instant> = None;

    loop {
        // Adaptive timeout: 1ms when echo-pending or fresh PTY data just
        // arrived (so we can serve the waiting dump-state request quickly),
        // 5ms otherwise to stay idle-friendly.
        let data_ready = crate::types::PTY_DATA_READY.swap(false, std::sync::atomic::Ordering::AcqRel);
        if data_ready {
            state_dirty = true;
        }
        let echo_active = echo_pending_until.map_or(false, |t| t.elapsed().as_millis() < 50);
        let timeout_ms: u64 = if echo_active || data_ready { 1 } else { 5 };
        if let Some(rx) = app.control_rx.as_ref() {
            if let Ok(req) = rx.recv_timeout(Duration::from_millis(timeout_ms)) {
                let mut pending = vec![req];
                // Drain any additional queued messages without blocking
                while let Ok(r) = rx.try_recv() {
                    pending.push(r);
                }
                // Also check if fresh PTY output arrived while we were
                // waiting – mark state dirty so DumpState produces a full
                // frame instead of "NC".
                if crate::types::PTY_DATA_READY.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    state_dirty = true;
                }
                // Process key/command inputs BEFORE dump-state requests.
                // This ensures ConPTY receives keystrokes before we serialize
                // the screen, reducing stale-frame responses.
                pending.sort_by_key(|r| match r {
                    CtrlReq::DumpState(_) => 1,
                    CtrlReq::DumpLayout(_) => 1,
                    _ => 0,
                });
                for req in pending {
                    let mutates_state = !matches!(&req, CtrlReq::DumpState(_));
                    let mut hook_event: Option<&str> = None;
                    match req {
                CtrlReq::NewWindow(cmd, name, detached, start_dir) => {
                    let prev_idx = app.active_idx;
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    let _ = create_window(&*pty_system, &mut app, cmd.as_deref());
                    if let Some(n) = name { app.windows.last_mut().map(|w| w.name = n); }
                    if detached { app.active_idx = prev_idx; }
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-new-window");
                }
                CtrlReq::NewWindowPrint(cmd, name, detached, start_dir, format_str, resp) => {
                    let prev_idx = app.active_idx;
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    let _ = create_window(&*pty_system, &mut app, cmd.as_deref());
                    if let Some(n) = name { app.windows.last_mut().map(|w| w.name = n); }
                    // Use full format engine for -P output (tmux compatible)
                    let new_win_idx = app.windows.len() - 1;
                    let fmt = format_str.as_deref().unwrap_or("#{session_name}:#{window_index}");
                    let pane_info = crate::format::expand_format_for_window(fmt, &app, new_win_idx);
                    if detached { app.active_idx = prev_idx; }
                    let _ = resp.send(pane_info);
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-new-window");
                }
                CtrlReq::SplitWindow(k, cmd, detached, start_dir, size_pct) => {
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    let prev_path = app.windows[app.active_idx].active_path.clone();
                    let _ = split_active_with_command(&mut app, k, cmd.as_deref());
                    // Apply size if specified (as percentage)
                    if let Some(_pct) = size_pct {
                        // Size will be applied by resize_all_panes using the layout ratios
                    }
                    if detached {
                        // Revert focus to the previously active pane.
                        // After split, prev_path now points to a Split node;
                        // the original pane is child [0] of that Split.
                        let mut revert_path = prev_path;
                        revert_path.push(0);
                        app.windows[app.active_idx].active_path = revert_path;
                    }
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-split-window");
                }
                CtrlReq::SplitWindowPrint(k, cmd, detached, start_dir, size_pct, format_str, resp) => {
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    let prev_path = app.windows[app.active_idx].active_path.clone();
                    let _ = split_active_with_command(&mut app, k, cmd.as_deref());
                    if let Some(_pct) = size_pct { }
                    // Use full format engine for -P output (tmux compatible)
                    let fmt = format_str.as_deref().unwrap_or("#{session_name}:#{window_index}.#{pane_index}");
                    let pane_info = crate::format::expand_format_for_window(fmt, &app, app.active_idx);
                    if detached {
                        let mut revert_path = prev_path;
                        revert_path.push(0);
                        app.windows[app.active_idx].active_path = revert_path;
                    }
                    let _ = resp.send(pane_info);
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-split-window");
                }
                CtrlReq::KillPane => { let _ = kill_active_pane(&mut app); resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-kill-pane"); }
                CtrlReq::CapturePane(resp) => {
                    if let Some(text) = capture_active_pane_text(&mut app)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneStyled(resp) => {
                    if let Some(text) = capture_active_pane_styled(&mut app)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneRange(resp, s, e) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::FocusWindow(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } meta_dirty = true; }
                CtrlReq::FocusPane(pid) => { focus_pane_by_id(&mut app, pid); meta_dirty = true; }
                CtrlReq::FocusPaneByIndex(idx) => { focus_pane_by_index(&mut app, idx); meta_dirty = true; }
                CtrlReq::SessionInfo(resp) => {
                    let attached = if app.attached_clients > 0 { "(attached)" } else { "(detached)" };
                    let windows = app.windows.len();
                    let (w,h) = {
                        let win = &mut app.windows[app.active_idx];
                        let mut size = (0,0);
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { size = (p.last_cols as i32, p.last_rows as i32); }
                        size
                    };
                    let created = app.created_at.format("%a %b %e %H:%M:%S %Y");
                    let line = format!("{}: {} windows (created {}) [{}x{}] {}\n", app.session_name, windows, created, w, h, attached);
                    let _ = resp.send(line);
                }
                CtrlReq::ClientAttach => { app.attached_clients = app.attached_clients.saturating_add(1); hook_event = Some("client-attached"); }
                CtrlReq::ClientDetach => { app.attached_clients = app.attached_clients.saturating_sub(1); hook_event = Some("client-detached"); }
                CtrlReq::DumpLayout(resp) => {
                    let json = dump_layout_json(&mut app)?;
                    let _ = resp.send(json);
                }
                CtrlReq::DumpState(resp) => {
                    // ── Automatic rename: resolve foreground process ──
                    {
                        let in_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
                        if app.automatic_rename && !in_copy {
                            for win in app.windows.iter_mut() {
                                if win.manual_rename { continue; }
                                if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                                    if p.dead { continue; }
                                    if p.last_title_check.elapsed().as_millis() < 1000 { continue; }
                                    p.last_title_check = std::time::Instant::now();
                                    if p.child_pid.is_none() {
                                        p.child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*p.child) };
                                    }
                                    let new_name = if let Some(pid) = p.child_pid {
                                        crate::platform::process_info::get_foreground_process_name(pid)
                                            .unwrap_or_else(|| "shell".into())
                                    } else if !p.title.is_empty() {
                                        p.title.clone()
                                    } else {
                                        continue;
                                    };
                                    if !new_name.is_empty() && win.name != new_name {
                                        win.name = new_name;
                                        meta_dirty = true;
                                        state_dirty = true;
                                    }
                                }
                            }
                        }
                    }
                    // Fast-path: nothing changed at all → 2-byte "NC" marker
                    // instead of cloning 50-100KB of JSON.
                    if !state_dirty
                        && !cached_dump_state.is_empty()
                        && cached_data_version == combined_data_version(&app)
                    {
                        let _ = resp.send("NC".to_string());
                        continue;
                    }
                    // Rebuild metadata cache if structural changes happened.
                    if meta_dirty {
                        cached_windows_json = list_windows_json_with_tabs(&app)?;
                        cached_tree_json = list_tree_json(&app)?;
                        cached_prefix_str = format_key_binding(&app.prefix_key);
                        cached_base_index = app.window_base_index;
                        cached_pred_dim = app.prediction_dimming;
                        cached_status_style = app.status_style.clone();
                        cached_bindings_json = serialize_bindings_json(&app);
                        meta_dirty = false;
                    }
                    let _t_layout = std::time::Instant::now();
                    let layout_json = dump_layout_json_fast(&mut app)?;
                    let _layout_ms = _t_layout.elapsed().as_micros();
                    combined_buf.clear();
                    let ss_escaped = json_escape_string(&cached_status_style);
                    let sl_expanded = json_escape_string(&expand_format(&app.status_left, &app));
                    let sr_expanded = json_escape_string(&expand_format(&app.status_right, &app));
                    let pbs_escaped = json_escape_string(&app.pane_border_style);
                    let pabs_escaped = json_escape_string(&app.pane_active_border_style);
                    let wsf_escaped = json_escape_string(&app.window_status_format);
                    let wscf_escaped = json_escape_string(&app.window_status_current_format);
                    let wss_escaped = json_escape_string(&app.window_status_separator);
                    let ws_style_escaped = json_escape_string(&app.window_status_style);
                    let wsc_style_escaped = json_escape_string(&app.window_status_current_style);
                    let _ = std::fmt::Write::write_fmt(&mut combined_buf, format_args!(
                        "{{\"layout\":{},\"windows\":{},\"prefix\":\"{}\",\"tree\":{},\"base_index\":{},\"prediction_dimming\":{},\"status_style\":\"{}\",\"status_left\":\"{}\",\"status_right\":\"{}\",\"pane_border_style\":\"{}\",\"pane_active_border_style\":\"{}\",\"wsf\":\"{}\",\"wscf\":\"{}\",\"wss\":\"{}\",\"ws_style\":\"{}\",\"wsc_style\":\"{}\",\"clock_mode\":{},\"bindings\":{}}}",
                        layout_json, cached_windows_json, cached_prefix_str, cached_tree_json, cached_base_index, cached_pred_dim, ss_escaped, sl_expanded, sr_expanded, pbs_escaped, pabs_escaped, wsf_escaped, wscf_escaped, wss_escaped, ws_style_escaped, wsc_style_escaped,
                        matches!(app.mode, Mode::ClockMode), cached_bindings_json,
                    ));
                    cached_dump_state.clear();
                    cached_dump_state.push_str(&combined_buf);
                    cached_data_version = combined_data_version(&app);
                    state_dirty = false;
                    // Timing log: dump-state build time
                    if std::env::var("PSMUX_LATENCY_LOG").unwrap_or_default() == "1" {
                        let total_us = _t_layout.elapsed().as_micros();
                        use std::io::Write as _;
                        static SRV_LOG_INIT: std::sync::Once = std::sync::Once::new();
                        static mut SRV_LOG: Option<std::sync::Mutex<std::fs::File>> = None;
                        SRV_LOG_INIT.call_once(|| {
                            let p = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\gj".into())).join("psmux_server_latency.log");
                            if let Ok(f) = std::fs::File::create(p) {
                                unsafe { SRV_LOG = Some(std::sync::Mutex::new(f)); }
                            }
                        });
                        if let Some(ref mtx) = unsafe { &SRV_LOG } {
                            if let Ok(mut f) = mtx.lock() {
                                let _ = writeln!(f, "[SRV] dump: layout={}us total={}us json_len={}", _layout_ms, total_us, combined_buf.len());
                            }
                        }
                    }
                    let _ = resp.send(combined_buf.clone());
                }
                CtrlReq::SendText(s) => { send_text_to_active(&mut app, &s)?; echo_pending_until = Some(Instant::now()); }
                CtrlReq::SendKey(k) => { send_key_to_active(&mut app, &k)?; echo_pending_until = Some(Instant::now()); }
                CtrlReq::SendPaste(s) => { send_text_to_active(&mut app, &s)?; echo_pending_until = Some(Instant::now()); }
                CtrlReq::ZoomPane => { toggle_zoom(&mut app); hook_event = Some("after-resize-pane"); }
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); }
                CtrlReq::CopyEnterPageUp => {
                    enter_copy_mode(&mut app);
                    let half = app.windows.get(app.active_idx)
                        .and_then(|w| active_pane(&w.root, &w.active_path))
                        .map(|p| p.last_rows as usize).unwrap_or(20);
                    scroll_copy_up(&mut app, half);
                }
                CtrlReq::ClockMode => { app.mode = Mode::ClockMode; }
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                CtrlReq::CopyYank => { let _ = yank_selection(&mut app); app.mode = Mode::Passthrough; }
                CtrlReq::ClientSize(w, h) => { 
                    app.last_window_area = Rect { x: 0, y: 0, width: w, height: h }; 
                    resize_all_panes(&mut app);
                }
                CtrlReq::FocusPaneCmd(pid) => { focus_pane_by_id(&mut app, pid); meta_dirty = true; }
                CtrlReq::FocusWindowCmd(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } meta_dirty = true; }
                CtrlReq::MouseDown(x,y) => { if app.mouse_enabled { remote_mouse_down(&mut app, x, y); state_dirty = true; meta_dirty = true; } }
                CtrlReq::MouseDownRight(x,y) => { if app.mouse_enabled { remote_mouse_button(&mut app, x, y, 2, true); state_dirty = true; } }
                CtrlReq::MouseDownMiddle(x,y) => { if app.mouse_enabled { remote_mouse_button(&mut app, x, y, 1, true); state_dirty = true; } }
                CtrlReq::MouseDrag(x,y) => { if app.mouse_enabled { remote_mouse_drag(&mut app, x, y); state_dirty = true; } }
                CtrlReq::MouseUp(x,y) => { if app.mouse_enabled { remote_mouse_up(&mut app, x, y); state_dirty = true; } }
                CtrlReq::MouseUpRight(x,y) => { if app.mouse_enabled { remote_mouse_button(&mut app, x, y, 2, false); state_dirty = true; } }
                CtrlReq::MouseUpMiddle(x,y) => { if app.mouse_enabled { remote_mouse_button(&mut app, x, y, 1, false); state_dirty = true; } }
                CtrlReq::MouseMove(x,y) => { if app.mouse_enabled { remote_mouse_motion(&mut app, x, y); } }
                CtrlReq::ScrollUp(x, y) => { if app.mouse_enabled { remote_scroll_up(&mut app, x, y); state_dirty = true; } }
                CtrlReq::ScrollDown(x, y) => { if app.mouse_enabled { remote_scroll_down(&mut app, x, y); state_dirty = true; } }
                CtrlReq::NextWindow => { if !app.windows.is_empty() { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + 1) % app.windows.len(); } meta_dirty = true; hook_event = Some("after-select-window"); }
                CtrlReq::PrevWindow => { if !app.windows.is_empty() { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); } meta_dirty = true; hook_event = Some("after-select-window"); }
                CtrlReq::RenameWindow(name) => { let win = &mut app.windows[app.active_idx]; win.name = name; win.manual_rename = true; meta_dirty = true; hook_event = Some("after-rename-window"); }
                CtrlReq::ListWindows(resp) => { let json = list_windows_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ListWindowsTmux(resp) => { let text = list_windows_tmux(&app); let _ = resp.send(text); }
                CtrlReq::ListWindowsFormat(resp, fmt) => { let text = format_list_windows(&app, &fmt); let _ = resp.send(text); }
                CtrlReq::ListTree(resp) => { let json = list_tree_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ToggleSync => { app.sync_input = !app.sync_input; }
                CtrlReq::SetPaneTitle(title) => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { p.title = title; }
                }
                CtrlReq::SendKeys(keys, literal) => {
                    let in_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
                    if in_copy {
                        // In copy/search mode — route through mode-aware handlers
                        if literal {
                            send_text_to_active(&mut app, &keys)?;
                        } else {
                            let parts: Vec<&str> = keys.split_whitespace().collect();
                            for key in parts.iter() {
                                let key_upper = key.to_uppercase();
                                let normalized = match key_upper.as_str() {
                                    "ENTER" => "enter",
                                    "TAB" => "tab",
                                    "ESCAPE" | "ESC" => "esc",
                                    "SPACE" => "space",
                                    "BSPACE" | "BACKSPACE" => "backspace",
                                    "UP" => "up",
                                    "DOWN" => "down",
                                    "RIGHT" => "right",
                                    "LEFT" => "left",
                                    "HOME" => "home",
                                    "END" => "end",
                                    "PAGEUP" | "PPAGE" => "pageup",
                                    "PAGEDOWN" | "NPAGE" => "pagedown",
                                    "DELETE" | "DC" => "delete",
                                    "INSERT" | "IC" => "insert",
                                    _ => "",
                                };
                                if !normalized.is_empty() {
                                    send_key_to_active(&mut app, normalized)?;
                                } else if key_upper.starts_with("C-") || key_upper.starts_with("M-") || (key_upper.starts_with("F") && key_upper.len() >= 2 && key_upper[1..].chars().all(|c| c.is_ascii_digit())) {
                                    send_key_to_active(&mut app, &key.to_lowercase())?;
                                } else {
                                    // Plain text char — route through send_text_to_active (handles copy mode chars)
                                    send_text_to_active(&mut app, key)?;
                                }
                            }
                        }
                    } else if literal {
                        send_text_to_active(&mut app, &keys)?;
                    } else {
                        let parts: Vec<&str> = keys.split_whitespace().collect();
                        for (i, key) in parts.iter().enumerate() {
                            let key_upper = key.to_uppercase();
                            let _is_special = matches!(key_upper.as_str(), 
                                "ENTER" | "TAB" | "ESCAPE" | "ESC" | "SPACE" | "BSPACE" | "BACKSPACE" |
                                "UP" | "DOWN" | "RIGHT" | "LEFT" | "HOME" | "END" |
                                "PAGEUP" | "PPAGE" | "PAGEDOWN" | "NPAGE" | "DELETE" | "DC" | "INSERT" | "IC" |
                                "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
                            ) || key_upper.starts_with("C-") || key_upper.starts_with("M-");
                            
                            match key_upper.as_str() {
                                "ENTER" => send_text_to_active(&mut app, "\r")?,
                                "TAB" => send_text_to_active(&mut app, "\t")?,
                                "ESCAPE" | "ESC" => send_text_to_active(&mut app, "\x1b")?,
                                "SPACE" => send_text_to_active(&mut app, " ")?,
                                "BSPACE" | "BACKSPACE" => send_text_to_active(&mut app, "\x7f")?,
                                "UP" => send_text_to_active(&mut app, "\x1b[A")?,
                                "DOWN" => send_text_to_active(&mut app, "\x1b[B")?,
                                "RIGHT" => send_text_to_active(&mut app, "\x1b[C")?,
                                "LEFT" => send_text_to_active(&mut app, "\x1b[D")?,
                                "HOME" => send_text_to_active(&mut app, "\x1b[H")?,
                                "END" => send_text_to_active(&mut app, "\x1b[F")?,
                                "PAGEUP" | "PPAGE" => send_text_to_active(&mut app, "\x1b[5~")?,
                                "PAGEDOWN" | "NPAGE" => send_text_to_active(&mut app, "\x1b[6~")?,
                                "DELETE" | "DC" => send_text_to_active(&mut app, "\x1b[3~")?,
                                "INSERT" | "IC" => send_text_to_active(&mut app, "\x1b[2~")?,
                                "F1" => send_text_to_active(&mut app, "\x1bOP")?,
                                "F2" => send_text_to_active(&mut app, "\x1bOQ")?,
                                "F3" => send_text_to_active(&mut app, "\x1bOR")?,
                                "F4" => send_text_to_active(&mut app, "\x1bOS")?,
                                "F5" => send_text_to_active(&mut app, "\x1b[15~")?,
                                "F6" => send_text_to_active(&mut app, "\x1b[17~")?,
                                "F7" => send_text_to_active(&mut app, "\x1b[18~")?,
                                "F8" => send_text_to_active(&mut app, "\x1b[19~")?,
                                "F9" => send_text_to_active(&mut app, "\x1b[20~")?,
                                "F10" => send_text_to_active(&mut app, "\x1b[21~")?,
                                "F11" => send_text_to_active(&mut app, "\x1b[23~")?,
                                "F12" => send_text_to_active(&mut app, "\x1b[24~")?,
                                s if s.starts_with("C-M-") || s.starts_with("C-m-") => {
                                    if let Some(c) = key.chars().nth(4) {
                                        let ctrl = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a').wrapping_add(1);
                                        send_text_to_active(&mut app, &format!("\x1b{}", ctrl as char))?;
                                    }
                                }
                                s if s.starts_with("C-") => {
                                    if let Some(c) = s.chars().nth(2) {
                                        let ctrl = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a').wrapping_add(1);
                                        send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                    }
                                }
                                s if s.starts_with("M-") => {
                                    if let Some(c) = key.chars().nth(2) {
                                        send_text_to_active(&mut app, &format!("\x1b{}", c))?;
                                    }
                                }
                                _ => {
                                    send_text_to_active(&mut app, key)?;
                                    if i + 1 < parts.len() {
                                        let next_upper = parts[i + 1].to_uppercase();
                                        let next_is_special = matches!(next_upper.as_str(),
                                            "ENTER" | "TAB" | "ESCAPE" | "ESC" | "SPACE" | "BSPACE" | "BACKSPACE" |
                                            "UP" | "DOWN" | "RIGHT" | "LEFT" | "HOME" | "END" |
                                            "PAGEUP" | "PPAGE" | "PAGEDOWN" | "NPAGE" | "DELETE" | "DC" | "INSERT" | "IC" |
                                            "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
                                        ) || next_upper.starts_with("C-") || next_upper.starts_with("M-");
                                        if !next_is_special {
                                            send_text_to_active(&mut app, " ")?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    echo_pending_until = Some(Instant::now());
                }
                CtrlReq::SendKeysX(cmd) => {
                    // send-keys -X: dispatch copy-mode commands by name
                    // This is the primary mechanism used by tmux-yank and other plugins
                    let in_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
                    if !in_copy {
                        // Auto-enter copy mode for commands that require it
                        enter_copy_mode(&mut app);
                    }
                    match cmd.as_str() {
                        "cancel" => {
                            app.mode = Mode::Passthrough;
                            app.copy_anchor = None;
                            app.copy_pos = None;
                            app.copy_scroll_offset = 0;
                            let win = &mut app.windows[app.active_idx];
                            if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                                if let Ok(mut parser) = p.term.lock() {
                                    parser.screen_mut().set_scrollback(0);
                                }
                            }
                        }
                        "begin-selection" => {
                            if let Some((r,c)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r,c));
                                app.copy_pos = Some((r,c));
                                app.copy_selection_mode = crate::types::SelectionMode::Char;
                            }
                        }
                        "select-line" => {
                            if let Some((r,c)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r,c));
                                app.copy_pos = Some((r,c));
                                app.copy_selection_mode = crate::types::SelectionMode::Line;
                            }
                        }
                        "rectangle-toggle" => {
                            app.copy_selection_mode = match app.copy_selection_mode {
                                crate::types::SelectionMode::Rect => crate::types::SelectionMode::Char,
                                _ => crate::types::SelectionMode::Rect,
                            };
                        }
                        "copy-selection" => {
                            let _ = yank_selection(&mut app);
                        }
                        "copy-selection-and-cancel" => {
                            let _ = yank_selection(&mut app);
                            app.mode = Mode::Passthrough;
                            app.copy_scroll_offset = 0;
                            app.copy_pos = None;
                        }
                        "copy-selection-no-clear" => {
                            let _ = yank_selection(&mut app);
                        }
                        s if s.starts_with("copy-pipe-and-cancel") || s.starts_with("copy-pipe") => {
                            // copy-pipe[-and-cancel] [command] — yank + pipe to command
                            let _ = yank_selection(&mut app);
                            // Extract pipe command from argument if present
                            let cancel = s.contains("cancel");
                            let pipe_cmd = cmd.strip_prefix("copy-pipe-and-cancel")
                                .or_else(|| cmd.strip_prefix("copy-pipe"))
                                .unwrap_or("")
                                .trim();
                            if !pipe_cmd.is_empty() {
                                if let Some(text) = app.paste_buffers.first().cloned() {
                                    // Pipe yanked text to the command's stdin
                                    if let Ok(mut child) = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
                                        .args(if cfg!(windows) { vec!["/C", pipe_cmd] } else { vec!["-c", pipe_cmd] })
                                        .stdin(std::process::Stdio::piped())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::null())
                                        .spawn() {
                                        if let Some(mut stdin) = child.stdin.take() {
                                            use std::io::Write;
                                            let _ = stdin.write_all(text.as_bytes());
                                        }
                                        let _ = child.wait();
                                    }
                                }
                            }
                            if cancel {
                                app.mode = Mode::Passthrough;
                                app.copy_scroll_offset = 0;
                                app.copy_pos = None;
                            }
                        }
                        "cursor-up" => { move_copy_cursor(&mut app, 0, -1); }
                        "cursor-down" => { move_copy_cursor(&mut app, 0, 1); }
                        "cursor-left" => { move_copy_cursor(&mut app, -1, 0); }
                        "cursor-right" => { move_copy_cursor(&mut app, 1, 0); }
                        "start-of-line" => { crate::copy_mode::move_to_line_start(&mut app); }
                        "end-of-line" => { crate::copy_mode::move_to_line_end(&mut app); }
                        "back-to-indentation" => { crate::copy_mode::move_to_first_nonblank(&mut app); }
                        "next-word" => { crate::copy_mode::move_word_forward(&mut app); }
                        "previous-word" => { crate::copy_mode::move_word_backward(&mut app); }
                        "next-word-end" => { crate::copy_mode::move_word_end(&mut app); }
                        "next-space" => { crate::copy_mode::move_word_forward_big(&mut app); }
                        "previous-space" => { crate::copy_mode::move_word_backward_big(&mut app); }
                        "next-space-end" => { crate::copy_mode::move_word_end_big(&mut app); }
                        "top-line" => { crate::copy_mode::move_to_screen_top(&mut app); }
                        "middle-line" => { crate::copy_mode::move_to_screen_middle(&mut app); }
                        "bottom-line" => { crate::copy_mode::move_to_screen_bottom(&mut app); }
                        "history-top" => { crate::copy_mode::scroll_to_top(&mut app); }
                        "history-bottom" => { crate::copy_mode::scroll_to_bottom(&mut app); }
                        "halfpage-up" => {
                            let half = app.windows.get(app.active_idx)
                                .and_then(|w| active_pane(&w.root, &w.active_path))
                                .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                            scroll_copy_up(&mut app, half);
                        }
                        "halfpage-down" => {
                            let half = app.windows.get(app.active_idx)
                                .and_then(|w| active_pane(&w.root, &w.active_path))
                                .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                            scroll_copy_down(&mut app, half);
                        }
                        "page-up" => { scroll_copy_up(&mut app, 20); }
                        "page-down" => { scroll_copy_down(&mut app, 20); }
                        "scroll-up" => { scroll_copy_up(&mut app, 1); }
                        "scroll-down" => { scroll_copy_down(&mut app, 1); }
                        "search-forward" | "search-forward-incremental" => {
                            app.mode = Mode::CopySearch { input: String::new(), forward: true };
                        }
                        "search-backward" | "search-backward-incremental" => {
                            app.mode = Mode::CopySearch { input: String::new(), forward: false };
                        }
                        "search-again" => { crate::copy_mode::search_next(&mut app); }
                        "search-reverse" => { crate::copy_mode::search_prev(&mut app); }
                        "copy-end-of-line" => { let _ = crate::copy_mode::copy_end_of_line(&mut app); app.mode = Mode::Passthrough; app.copy_scroll_offset = 0; app.copy_pos = None; }
                        "select-word" => {
                            // Select the word under cursor
                            crate::copy_mode::move_word_backward(&mut app);
                            if let Some((r,c)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r,c));
                                app.copy_selection_mode = crate::types::SelectionMode::Char;
                            }
                            crate::copy_mode::move_word_end(&mut app);
                        }
                        "other-end" => {
                            if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                                app.copy_anchor = Some(p);
                                app.copy_pos = Some(a);
                            }
                        }
                        "clear-selection" => {
                            app.copy_anchor = None;
                            app.copy_selection_mode = crate::types::SelectionMode::Char;
                        }
                        "append-selection" => {
                            // Append to existing buffer instead of replacing
                            let _ = yank_selection(&mut app);
                            if app.paste_buffers.len() >= 2 {
                                let appended = format!("{}{}", app.paste_buffers[1], app.paste_buffers[0]);
                                app.paste_buffers[0] = appended;
                            }
                        }
                        "append-selection-and-cancel" => {
                            let _ = yank_selection(&mut app);
                            if app.paste_buffers.len() >= 2 {
                                let appended = format!("{}{}", app.paste_buffers[1], app.paste_buffers[0]);
                                app.paste_buffers[0] = appended;
                            }
                            app.mode = Mode::Passthrough;
                            app.copy_scroll_offset = 0;
                            app.copy_pos = None;
                        }
                        "copy-line" => {
                            // Select entire current line and yank
                            if let Some((r, _)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r, 0));
                                app.copy_selection_mode = crate::types::SelectionMode::Line;
                                let cols = app.windows.get(app.active_idx)
                                    .and_then(|w| active_pane(&w.root, &w.active_path))
                                    .map(|p| p.last_cols).unwrap_or(80);
                                app.copy_pos = Some((r, cols.saturating_sub(1)));
                                let _ = yank_selection(&mut app);
                            }
                            app.mode = Mode::Passthrough;
                            app.copy_scroll_offset = 0;
                            app.copy_pos = None;
                        }
                        s if s.starts_with("goto-line") => {
                            // goto-line <N> — jump to line N in scrollback
                            let n = s.strip_prefix("goto-line").unwrap_or("").trim()
                                .parse::<u16>().unwrap_or(0);
                            app.copy_pos = Some((n, 0));
                        }
                        "jump-forward" => { app.copy_find_char_pending = Some(0); }
                        "jump-backward" => { app.copy_find_char_pending = Some(1); }
                        "jump-to-forward" => { app.copy_find_char_pending = Some(2); }
                        "jump-to-backward" => { app.copy_find_char_pending = Some(3); }
                        "jump-again" => {
                            // Repeat last find-char in same direction
                            // We'd need to store last char; for now emit the pending
                        }
                        "jump-reverse" => {
                            // Repeat last find-char in reverse direction
                        }
                        "next-paragraph" => {
                            crate::copy_mode::move_next_paragraph(&mut app);
                        }
                        "previous-paragraph" => {
                            crate::copy_mode::move_prev_paragraph(&mut app);
                        }
                        "next-matching-bracket" => {
                            crate::copy_mode::move_matching_bracket(&mut app);
                        }
                        "stop-selection" => {
                            // Keep cursor position but stop extending selection
                            app.copy_anchor = None;
                        }
                        _ => {} // ignore unknown copy-mode commands
                    }
                }
                CtrlReq::SelectPane(dir) => {
                    match dir.as_str() {
                        "U" => { move_focus(&mut app, FocusDir::Up); }
                        "D" => { move_focus(&mut app, FocusDir::Down); }
                        "L" => { move_focus(&mut app, FocusDir::Left); }
                        "R" => { move_focus(&mut app, FocusDir::Right); }
                        "last" => {
                            // select-pane -l: switch to last active pane
                            let win = &mut app.windows[app.active_idx];
                            if !app.last_pane_path.is_empty() {
                                let tmp = win.active_path.clone();
                                win.active_path = app.last_pane_path.clone();
                                app.last_pane_path = tmp;
                            }
                        }
                        "mark" => {
                            // select-pane -m: mark the current pane
                            let win = &app.windows[app.active_idx];
                            if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                app.marked_pane = Some((app.active_idx, pid));
                            }
                        }
                        "unmark" => {
                            // select-pane -M: clear the marked pane
                            app.marked_pane = None;
                        }
                        _ => {}
                    }
                    hook_event = Some("after-select-pane");
                }
                CtrlReq::SelectWindow(idx) => {
                    if idx >= app.window_base_index {
                        let internal_idx = idx - app.window_base_index;
                        if internal_idx < app.windows.len() {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = internal_idx;
                        }
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                CtrlReq::ListPanes(resp) => {
                    let mut output = String::new();
                    let win = &app.windows[app.active_idx];
                    fn collect_panes(node: &Node, panes: &mut Vec<(usize, u16, u16, vt100::MouseProtocolMode, vt100::MouseProtocolEncoding, bool)>) {
                        match node {
                            Node::Leaf(p) => {
                                let (mode, enc, alt) = {
                                    let term = p.term.lock().unwrap();
                                    let screen = term.screen();
                                    (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding(), screen.alternate_screen())
                                };
                                panes.push((p.id, p.last_cols, p.last_rows, mode, enc, alt));
                            }
                            Node::Split { children, .. } => {
                                for c in children { collect_panes(c, panes); }
                            }
                        }
                    }
                    let mut panes = Vec::new();
                    collect_panes(&win.root, &mut panes);
                    for (_i, (id, cols, rows, mode, enc, alt)) in panes.iter().enumerate() {
                        output.push_str(&format!("%{}: [{}x{}] mouse={:?}/{:?} alt={}\n", id, cols, rows, mode, enc, alt));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListPanesFormat(resp, fmt) => {
                    let text = format_list_panes(&app, &fmt, app.active_idx);
                    let _ = resp.send(text);
                }
                CtrlReq::ListAllPanes(resp) => {
                    let mut output = String::new();
                    fn collect_all_panes(node: &Node, panes: &mut Vec<(usize, u16, u16)>) {
                        match node {
                            Node::Leaf(p) => { panes.push((p.id, p.last_cols, p.last_rows)); }
                            Node::Split { children, .. } => { for c in children { collect_all_panes(c, panes); } }
                        }
                    }
                    for (wi, win) in app.windows.iter().enumerate() {
                        let mut panes = Vec::new();
                        collect_all_panes(&win.root, &mut panes);
                        for (id, cols, rows) in panes {
                            output.push_str(&format!("{}:{}: %{} [{}x{}]\n", app.session_name, wi + app.window_base_index, id, cols, rows));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListAllPanesFormat(resp, fmt) => {
                    let mut lines = Vec::new();
                    for wi in 0..app.windows.len() {
                        lines.push(format_list_panes(&app, &fmt, wi));
                    }
                    let _ = resp.send(lines.join("\n"));
                }
                CtrlReq::KillWindow => {
                    if app.windows.len() > 1 {
                        let mut win = app.windows.remove(app.active_idx);
                        kill_all_children(&mut win.root);
                        if app.active_idx >= app.windows.len() { app.active_idx = app.windows.len() - 1; }
                    } else {
                        // Last window: kill all children; reaper will detect empty session and exit
                        kill_all_children(&mut app.windows[0].root);
                    }
                    hook_event = Some("window-closed");
                }
                CtrlReq::KillSession => {
                    // Kill all child processes in all windows before exiting
                    for win in app.windows.iter_mut() {
                        kill_all_children(&mut win.root);
                    }
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let regpath = format!("{}\\.psmux\\{}.port", home, app.port_file_base());
                    let keypath = format!("{}\\.psmux\\{}.key", home, app.port_file_base());
                    let _ = std::fs::remove_file(&regpath);
                    let _ = std::fs::remove_file(&keypath);
                    std::process::exit(0);
                }
                CtrlReq::HasSession(resp) => {
                    let _ = resp.send(true);
                }
                CtrlReq::RenameSession(name) => {
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let old_path = format!("{}\\.psmux\\{}.port", home, app.port_file_base());
                    let old_keypath = format!("{}\\.psmux\\{}.key", home, app.port_file_base());
                    // Compute new port file base with socket_name prefix
                    let new_base = if let Some(ref sn) = app.socket_name {
                        format!("{}__{}" , sn, name)
                    } else {
                        name.clone()
                    };
                    let new_path = format!("{}\\.psmux\\{}.port", home, new_base);
                    let new_keypath = format!("{}\\.psmux\\{}.key", home, new_base);
                    if let Some(port) = app.control_port {
                        let _ = std::fs::remove_file(&old_path);
                        let _ = std::fs::write(&new_path, port.to_string());
                        if let Ok(key) = std::fs::read_to_string(&old_keypath) {
                            let _ = std::fs::remove_file(&old_keypath);
                            let _ = std::fs::write(&new_keypath, key);
                        }
                    }
                    app.session_name = name;
                    hook_event = Some("after-rename-session");
                }
                CtrlReq::SwapPane(dir) => {
                    match dir.as_str() {
                        "U" => { swap_pane(&mut app, FocusDir::Up); }
                        "D" => { swap_pane(&mut app, FocusDir::Down); }
                        _ => { swap_pane(&mut app, FocusDir::Down); }
                    }
                    hook_event = Some("after-swap-pane");
                }
                CtrlReq::ResizePane(dir, amount) => {
                    match dir.as_str() {
                        "U" | "D" => { resize_pane_vertical(&mut app, if dir == "U" { -(amount as i16) } else { amount as i16 }); }
                        "L" | "R" => { resize_pane_horizontal(&mut app, if dir == "L" { -(amount as i16) } else { amount as i16 }); }
                        _ => {}
                    }
                    hook_event = Some("after-resize-pane");
                }
                CtrlReq::SetBuffer(content) => {
                    app.paste_buffers.insert(0, content);
                    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
                }
                CtrlReq::ListBuffers(resp) => {
                    let mut output = String::new();
                    for (i, buf) in app.paste_buffers.iter().enumerate() {
                        let preview: String = buf.chars().take(50).collect();
                        output.push_str(&format!("buffer{}: {} bytes: \"{}\"\n", i, buf.len(), preview));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListBuffersFormat(resp, fmt) => {
                    let mut output = Vec::new();
                    for (i, _buf) in app.paste_buffers.iter().enumerate() {
                        // Each buffer expands using the same format string
                        // For now, expand with buffer_name/buffer_size from the current app state
                        let _ = i; // buffer index context
                        output.push(expand_format(&fmt, &app));
                    }
                    let _ = resp.send(output.join("\n"));
                }
                CtrlReq::ShowBuffer(resp) => {
                    let content = app.paste_buffers.first().cloned().unwrap_or_default();
                    let _ = resp.send(content);
                }
                CtrlReq::ShowBufferAt(resp, idx) => {
                    let content = app.paste_buffers.get(idx).cloned().unwrap_or_default();
                    let _ = resp.send(content);
                }
                CtrlReq::DeleteBuffer => {
                    if !app.paste_buffers.is_empty() { app.paste_buffers.remove(0); }
                }
                CtrlReq::DisplayMessage(resp, fmt) => {
                    let result = expand_format(&fmt, &app);
                    let _ = resp.send(result);
                }
                CtrlReq::LastWindow => {
                    if app.windows.len() > 1 && app.last_window_idx < app.windows.len() {
                        let tmp = app.active_idx;
                        app.active_idx = app.last_window_idx;
                        app.last_window_idx = tmp;
                    }
                    hook_event = Some("after-select-window");
                }
                CtrlReq::LastPane => {
                    let win = &mut app.windows[app.active_idx];
                    if !app.last_pane_path.is_empty() && path_exists(&win.root, &app.last_pane_path) {
                        let tmp = win.active_path.clone();
                        win.active_path = app.last_pane_path.clone();
                        app.last_pane_path = tmp;
                    } else if !win.active_path.is_empty() {
                        let last = win.active_path.last_mut();
                        if let Some(idx) = last {
                            *idx = (*idx + 1) % 2;
                        }
                    }
                }
                CtrlReq::RotateWindow(reverse) => {
                    rotate_panes(&mut app, reverse);
                    hook_event = Some("after-rotate-window");
                }
                CtrlReq::DisplayPanes => {
                    app.mode = Mode::PaneChooser { opened_at: std::time::Instant::now() };
                    state_dirty = true;
                }
                CtrlReq::BreakPane => {
                    break_pane_to_window(&mut app);
                    hook_event = Some("after-break-pane");
                    meta_dirty = true;
                }
                CtrlReq::JoinPane(target_win) => {
                    // Real join-pane: extract active pane from current window and
                    // graft it as a vertical split into the target window.
                    let src_idx = app.active_idx;
                    if target_win < app.windows.len() && target_win != src_idx {
                        let src_path = app.windows[src_idx].active_path.clone();
                        let src_root = std::mem::replace(&mut app.windows[src_idx].root,
                            Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
                        let (remaining, extracted) = tree::extract_node(src_root, &src_path);
                        if let Some(pane_node) = extracted {
                            let src_empty = remaining.is_none();
                            if let Some(rem) = remaining {
                                app.windows[src_idx].root = rem;
                                app.windows[src_idx].active_path = tree::first_leaf_path(&app.windows[src_idx].root);
                            }
                            // Adjust target index if source window will be removed
                            let tgt = if src_empty && target_win > src_idx { target_win - 1 } else { target_win };
                            if src_empty {
                                app.windows.remove(src_idx);
                                if app.active_idx >= app.windows.len() {
                                    app.active_idx = app.windows.len().saturating_sub(1);
                                }
                            }
                            // Graft pane into target window
                            if tgt < app.windows.len() {
                                let tgt_path = app.windows[tgt].active_path.clone();
                                tree::replace_leaf_with_split(&mut app.windows[tgt].root, &tgt_path, LayoutKind::Vertical, pane_node);
                                app.active_idx = tgt;
                            }
                            resize_all_panes(&mut app);
                            meta_dirty = true;
                            hook_event = Some("after-join-pane");
                        } else {
                            // Extraction failed — restore
                            if let Some(rem) = remaining {
                                app.windows[src_idx].root = rem;
                            }
                        }
                    }
                }
                CtrlReq::RespawnPane => {
                    respawn_active_pane(&mut app)?;
                    hook_event = Some("after-respawn-pane");
                }
                CtrlReq::BindKey(table_name, key, command, repeat) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let kc = normalize_key_for_binding(kc);
                        // Support `\;` chaining in server-side bind-key
                        let sub_cmds = crate::config::split_chained_commands_pub(&command);
                        let action = if sub_cmds.len() > 1 {
                            Some(Action::CommandChain(sub_cmds))
                        } else {
                            parse_command_to_action(&command)
                        };
                        if let Some(act) = action {
                            let table = app.key_tables.entry(table_name).or_default();
                            table.retain(|b| b.key != kc);
                            table.push(Bind { key: kc, action: act, repeat });
                        }
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::UnbindKey(key) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let kc = normalize_key_for_binding(kc);
                        for table in app.key_tables.values_mut() {
                            table.retain(|b| b.key != kc);
                        }
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::ListKeys(resp) => {
                    // Build a list of default bindings as (key, command) pairs
                    let defaults: Vec<(&str, &str)> = vec![
                        ("c", "new-window"),
                        ("n", "next-window"),
                        ("p", "previous-window"),
                        ("%", "split-window -h"),
                        ("\"", "split-window -v"),
                        ("x", "kill-pane"),
                        ("d", "detach-client"),
                        ("w", "choose-window"),
                        (",", "rename-window"),
                        ("$", "rename-session"),
                        ("space", "next-layout"),
                        ("[", "copy-mode"),
                        ("]", "paste-buffer"),
                        (":", "command-prompt"),
                        ("q", "display-panes"),
                        ("z", "resize-pane -Z"),
                        ("o", "select-pane -t +"),
                        (";", "last-pane"),
                        ("l", "last-window"),
                        ("{", "swap-pane -U"),
                        ("}", "swap-pane -D"),
                        ("!", "break-pane"),
                        ("&", "kill-window"),
                        ("Up", "select-pane -U"),
                        ("Down", "select-pane -D"),
                        ("Left", "select-pane -L"),
                        ("Right", "select-pane -R"),
                        ("?", "list-keys"),
                        ("t", "clock-mode"),
                        ("=", "choose-buffer"),
                    ];

                    // Collect user-overridden key strings for the prefix table
                    let mut overridden_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
                    if let Some(prefix_binds) = app.key_tables.get("prefix") {
                        for bind in prefix_binds {
                            overridden_keys.insert(format_key_binding(&bind.key));
                        }
                    }

                    let mut output = String::new();
                    // Print defaults, excluding any that have been overridden by user
                    for (k, cmd) in &defaults {
                        if !overridden_keys.contains(*k) {
                            output.push_str(&format!("bind-key -T prefix {} {}\n", k, cmd));
                        }
                    }
                    // Print all user bindings from key_tables
                    for (table_name, binds) in &app.key_tables {
                        for bind in binds {
                            let key_str = format_key_binding(&bind.key);
                            let action_str = format_action(&bind.action);
                            output.push_str(&format!("bind-key -T {} {} {}\n", table_name, key_str, action_str));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::SetOption(option, value) => {
                    apply_set_option(&mut app, &option, &value, false);
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::SetOptionQuiet(option, value, quiet) => {
                    apply_set_option(&mut app, &option, &value, quiet);
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::SetOptionUnset(option) => {
                    // Reset option to default or remove @user-option
                    if option.starts_with('@') {
                        app.environment.remove(&option);
                    } else {
                        match option.as_str() {
                            "status-left" => { app.status_left = "psmux:#I".to_string(); }
                            "status-right" => { app.status_right = "%H:%M".to_string(); }
                            "mouse" => { app.mouse_enabled = true; }
                            "escape-time" => { app.escape_time_ms = 500; }
                            "history-limit" => { app.history_limit = 2000; }
                            "display-time" => { app.display_time_ms = 750; }
                            "mode-keys" => { app.mode_keys = "emacs".to_string(); }
                            "status" => { app.status_visible = true; }
                            "status-position" => { app.status_position = "bottom".to_string(); }
                            "status-style" => { app.status_style = String::new(); }
                            "renumber-windows" => { app.renumber_windows = false; }
                            "remain-on-exit" => { app.remain_on_exit = false; }
                            "automatic-rename" => { app.automatic_rename = true; }
                            "pane-border-style" => { app.pane_border_style = String::new(); }
                            "pane-active-border-style" => { app.pane_active_border_style = "fg=green".to_string(); }
                            "window-status-format" => { app.window_status_format = "#I:#W#F".to_string(); }
                            "window-status-current-format" => { app.window_status_current_format = "#I:#W#F".to_string(); }
                            "window-status-separator" => { app.window_status_separator = " ".to_string(); }
                            "cursor-style" => { std::env::set_var("PSMUX_CURSOR_STYLE", "bar"); }
                            "cursor-blink" => { std::env::set_var("PSMUX_CURSOR_BLINK", "1"); }
                            _ => {}
                        }
                    }
                }
                CtrlReq::SetOptionAppend(option, value) => {
                    // Append to existing option value
                    if option.starts_with('@') {
                        let existing = app.environment.get(&option).cloned().unwrap_or_default();
                        app.environment.insert(option, format!("{}{}", existing, value));
                    } else {
                        match option.as_str() {
                            "status-left" => { app.status_left.push_str(&value); }
                            "status-right" => { app.status_right.push_str(&value); }
                            "status-style" => { app.status_style.push_str(&value); }
                            "pane-border-style" => { app.pane_border_style.push_str(&value); }
                            "pane-active-border-style" => { app.pane_active_border_style.push_str(&value); }
                            "window-status-format" => { app.window_status_format.push_str(&value); }
                            "window-status-current-format" => { app.window_status_current_format.push_str(&value); }
                            _ => {}
                        }
                    }
                }
                CtrlReq::ShowOptions(resp) => {
                    let mut output = String::new();
                    output.push_str(&format!("prefix {}\n", format_key_binding(&app.prefix_key)));
                    output.push_str(&format!("base-index {}\n", app.window_base_index));
                    output.push_str(&format!("pane-base-index {}\n", app.pane_base_index));
                    output.push_str(&format!("escape-time {}\n", app.escape_time_ms));
                    output.push_str(&format!("mouse {}\n", if app.mouse_enabled { "on" } else { "off" }));
                    output.push_str(&format!("status {}\n", if app.status_visible { "on" } else { "off" }));
                    output.push_str(&format!("status-position {}\n", app.status_position));
                    output.push_str(&format!("status-left \"{}\"\n", app.status_left));
                    output.push_str(&format!("status-right \"{}\"\n", app.status_right));
                    output.push_str(&format!("history-limit {}\n", app.history_limit));
                    output.push_str(&format!("display-time {}\n", app.display_time_ms));
                    output.push_str(&format!("display-panes-time {}\n", app.display_panes_time_ms));
                    output.push_str(&format!("mode-keys {}\n", app.mode_keys));
                    output.push_str(&format!("focus-events {}\n", if app.focus_events { "on" } else { "off" }));
                    output.push_str(&format!("renumber-windows {}\n", if app.renumber_windows { "on" } else { "off" }));
                    output.push_str(&format!("automatic-rename {}\n", if app.automatic_rename { "on" } else { "off" }));
                    output.push_str(&format!("monitor-activity {}\n", if app.monitor_activity { "on" } else { "off" }));
                    output.push_str(&format!("synchronize-panes {}\n", if app.sync_input { "on" } else { "off" }));
                    output.push_str(&format!("remain-on-exit {}\n", if app.remain_on_exit { "on" } else { "off" }));
                    output.push_str(&format!("set-titles {}\n", if app.set_titles { "on" } else { "off" }));
                    if !app.set_titles_string.is_empty() {
                        output.push_str(&format!("set-titles-string \"{}\"\n", app.set_titles_string));
                    }
                    output.push_str(&format!(
                        "prediction-dimming {}\n",
                        if app.prediction_dimming { "on" } else { "off" }
                    ));
                    output.push_str(&format!("cursor-style {}\n", std::env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string())));
                    output.push_str(&format!("cursor-blink {}\n", if std::env::var("PSMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0" { "on" } else { "off" }));
                    if !app.default_shell.is_empty() {
                        output.push_str(&format!("default-shell {}\n", app.default_shell));
                    }
                    output.push_str(&format!("word-separators \"{}\"\n", app.word_separators));
                    if !app.pane_border_style.is_empty() {
                        output.push_str(&format!("pane-border-style \"{}\"\n", app.pane_border_style));
                    }
                    if !app.pane_active_border_style.is_empty() {
                        output.push_str(&format!("pane-active-border-style \"{}\"\n", app.pane_active_border_style));
                    }
                    if !app.status_style.is_empty() {
                        output.push_str(&format!("status-style \"{}\"\n", app.status_style));
                    }
                    if !app.status_left_style.is_empty() {
                        output.push_str(&format!("status-left-style \"{}\"\n", app.status_left_style));
                    }
                    if !app.status_right_style.is_empty() {
                        output.push_str(&format!("status-right-style \"{}\"\n", app.status_right_style));
                    }
                    output.push_str(&format!("status-interval {}\n", app.status_interval));
                    output.push_str(&format!("status-justify {}\n", app.status_justify));
                    output.push_str(&format!("window-status-format \"{}\"\n", app.window_status_format));
                    output.push_str(&format!("window-status-current-format \"{}\"\n", app.window_status_current_format));
                    if !app.window_status_style.is_empty() {
                        output.push_str(&format!("window-status-style \"{}\"\n", app.window_status_style));
                    }
                    if !app.window_status_current_style.is_empty() {
                        output.push_str(&format!("window-status-current-style \"{}\"\n", app.window_status_current_style));
                    }
                    if !app.window_status_activity_style.is_empty() {
                        output.push_str(&format!("window-status-activity-style \"{}\"\n", app.window_status_activity_style));
                    }
                    if !app.message_style.is_empty() {
                        output.push_str(&format!("message-style \"{}\"\n", app.message_style));
                    }
                    if !app.message_command_style.is_empty() {
                        output.push_str(&format!("message-command-style \"{}\"\n", app.message_command_style));
                    }
                    if !app.mode_style.is_empty() {
                        output.push_str(&format!("mode-style \"{}\"\n", app.mode_style));
                    }
                    // Include @user-options (used by plugins)
                    for (key, val) in &app.environment {
                        if key.starts_with('@') {
                            output.push_str(&format!("{} \"{}\"\n", key, val));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::SourceFile(path) => {
                    // Reuse full config parser for source-file (handles all options, binds, etc.)
                    let expanded = if path.starts_with('~') {
                        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                        path.replacen('~', &home, 1)
                    } else {
                        path.clone()
                    };
                    // Support glob patterns (needed by tpm: source ~/.tmux/plugins/*/*.tmux)
                    if expanded.contains('*') || expanded.contains('?') {
                        if let Ok(entries) = glob::glob(&expanded) {
                            for entry in entries.flatten() {
                                if let Ok(contents) = std::fs::read_to_string(&entry) {
                                    parse_config_content(&mut app, &contents);
                                }
                            }
                        }
                    } else if let Ok(contents) = std::fs::read_to_string(&expanded) {
                        parse_config_content(&mut app, &contents);
                    }
                }
                CtrlReq::MoveWindow(target) => {
                    if let Some(t) = target {
                        if t < app.windows.len() && app.active_idx != t {
                            let win = app.windows.remove(app.active_idx);
                            let insert_idx = if t > app.active_idx { t - 1 } else { t };
                            app.windows.insert(insert_idx.min(app.windows.len()), win);
                            app.active_idx = insert_idx.min(app.windows.len() - 1);
                        }
                    }
                }
                CtrlReq::SwapWindow(target) => {
                    if target < app.windows.len() && app.active_idx != target {
                        app.windows.swap(app.active_idx, target);
                    }
                }
                CtrlReq::LinkWindow(_target) => {}
                CtrlReq::UnlinkWindow => {
                    if app.windows.len() > 1 {
                        let mut win = app.windows.remove(app.active_idx);
                        kill_all_children(&mut win.root);
                        if app.active_idx >= app.windows.len() {
                            app.active_idx = app.windows.len() - 1;
                        }
                    }
                }
                CtrlReq::FindWindow(resp, pattern) => {
                    let mut output = String::new();
                    for (i, win) in app.windows.iter().enumerate() {
                        if win.name.contains(&pattern) {
                            output.push_str(&format!("{}: {} []\n", i + app.window_base_index, win.name));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::MovePane(target_win) => {
                    // move-pane is an alias for join-pane
                    let src_idx = app.active_idx;
                    if target_win < app.windows.len() && target_win != src_idx {
                        let src_path = app.windows[src_idx].active_path.clone();
                        let src_root = std::mem::replace(&mut app.windows[src_idx].root,
                            Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
                        let (remaining, extracted) = tree::extract_node(src_root, &src_path);
                        if let Some(pane_node) = extracted {
                            let src_empty = remaining.is_none();
                            if let Some(rem) = remaining {
                                app.windows[src_idx].root = rem;
                                app.windows[src_idx].active_path = tree::first_leaf_path(&app.windows[src_idx].root);
                            }
                            let tgt = if src_empty && target_win > src_idx { target_win - 1 } else { target_win };
                            if src_empty {
                                app.windows.remove(src_idx);
                                if app.active_idx >= app.windows.len() {
                                    app.active_idx = app.windows.len().saturating_sub(1);
                                }
                            }
                            if tgt < app.windows.len() {
                                let tgt_path = app.windows[tgt].active_path.clone();
                                tree::replace_leaf_with_split(&mut app.windows[tgt].root, &tgt_path, LayoutKind::Vertical, pane_node);
                                app.active_idx = tgt;
                            }
                            resize_all_panes(&mut app);
                            meta_dirty = true;
                        } else {
                            if let Some(rem) = remaining {
                                app.windows[src_idx].root = rem;
                            }
                        }
                    }
                }
                CtrlReq::PipePane(cmd, stdin, stdout) => {
                    let win = &app.windows[app.active_idx];
                    let pane_id = get_active_pane_id(&win.root, &win.active_path).unwrap_or(0);
                    
                    if cmd.is_empty() {
                        app.pipe_panes.retain(|p| p.pane_id != pane_id);
                    } else {
                        if let Some(idx) = app.pipe_panes.iter().position(|p| p.pane_id == pane_id) {
                            if let Some(ref mut proc) = app.pipe_panes[idx].process {
                                let _ = proc.kill();
                            }
                            app.pipe_panes.remove(idx);
                        } else {
                            #[cfg(windows)]
                            let process = std::process::Command::new("cmd")
                                .args(["/C", &cmd])
                                .stdin(if stdout { std::process::Stdio::piped() } else { std::process::Stdio::null() })
                                .stdout(if stdin { std::process::Stdio::piped() } else { std::process::Stdio::null() })
                                .stderr(std::process::Stdio::null())
                                .spawn()
                                .ok();
                            
                            app.pipe_panes.push(PipePaneState {
                                pane_id,
                                process,
                                stdin,
                                stdout,
                            });
                        }
                    }
                }
                CtrlReq::SelectLayout(layout) => {
                    apply_layout(&mut app, &layout);
                }
                CtrlReq::NextLayout => {
                    cycle_layout(&mut app);
                }
                CtrlReq::ListClients(resp) => {
                    let mut output = String::new();
                    output.push_str(&format!("/dev/pts/0: {}: {} [{}x{}] (utf8)\n", 
                        app.session_name, 
                        app.windows[app.active_idx].name,
                        app.last_window_area.width,
                        app.last_window_area.height
                    ));
                    let _ = resp.send(output);
                }
                CtrlReq::SwitchClient(_target) => {}
                CtrlReq::LockClient => {}
                CtrlReq::RefreshClient => { state_dirty = true; meta_dirty = true; }
                CtrlReq::SuspendClient => {}
                CtrlReq::CopyModePageUp => {
                    enter_copy_mode(&mut app);
                    move_copy_cursor(&mut app, 0, -20);
                }
                CtrlReq::ClearHistory => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        if let Ok(mut parser) = p.term.lock() {
                            *parser = vt100::Parser::new(p.last_rows, p.last_cols, app.history_limit);
                        }
                    }
                }
                CtrlReq::SaveBuffer(path) => {
                    if let Some(content) = app.paste_buffers.first() {
                        let _ = std::fs::write(&path, content);
                    }
                }
                CtrlReq::LoadBuffer(path) => {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        app.paste_buffers.insert(0, content);
                        if app.paste_buffers.len() > 10 {
                            app.paste_buffers.pop();
                        }
                    }
                }
                CtrlReq::SetEnvironment(key, value) => {
                    app.environment.insert(key.clone(), value.clone());
                    env::set_var(&key, &value);
                }
                CtrlReq::ShowEnvironment(resp) => {
                    let mut output = String::new();
                    // Show psmux/tmux-specific environment vars
                    for (key, value) in &app.environment {
                        output.push_str(&format!("{}={}\n", key, value));
                    }
                    // Also show inherited PSMUX_/TMUX_ vars from process env
                    for (key, value) in env::vars() {
                        if (key.starts_with("PSMUX") || key.starts_with("TMUX")) && !app.environment.contains_key(&key) {
                            output.push_str(&format!("{}={}\n", key, value));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::SetHook(hook, cmd) => {
                    app.hooks.entry(hook).or_insert_with(Vec::new).push(cmd);
                }
                CtrlReq::ShowHooks(resp) => {
                    let mut output = String::new();
                    for (name, commands) in &app.hooks {
                        for cmd in commands {
                            output.push_str(&format!("{} -> {}\n", name, cmd));
                        }
                    }
                    if output.is_empty() {
                        output.push_str("(no hooks)\n");
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::RemoveHook(hook) => {
                    app.hooks.remove(&hook);
                }
                CtrlReq::KillServer => {
                    // Kill all child processes in all windows before exiting
                    for win in app.windows.iter_mut() {
                        kill_all_children(&mut win.root);
                    }
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let regpath = format!("{}\\.psmux\\{}.port", home, app.port_file_base());
                    let keypath = format!("{}\\.psmux\\{}.key", home, app.port_file_base());
                    let _ = std::fs::remove_file(&regpath);
                    let _ = std::fs::remove_file(&keypath);
                    std::process::exit(0);
                }
                CtrlReq::WaitFor(channel, op) => {
                    match op {
                        WaitForOp::Lock => {
                            let entry = app.wait_channels.entry(channel).or_insert_with(|| WaitChannel {
                                locked: false,
                                waiters: Vec::new(),
                            });
                            entry.locked = true;
                        }
                        WaitForOp::Unlock => {
                            if let Some(ch) = app.wait_channels.get_mut(&channel) {
                                ch.locked = false;
                                for waiter in ch.waiters.drain(..) {
                                    let _ = waiter.send(());
                                }
                            }
                        }
                        WaitForOp::Signal => {
                            if let Some(ch) = app.wait_channels.get_mut(&channel) {
                                for waiter in ch.waiters.drain(..) {
                                    let _ = waiter.send(());
                                }
                            }
                        }
                        WaitForOp::Wait => {
                            app.wait_channels.entry(channel).or_insert_with(|| WaitChannel {
                                locked: false,
                                waiters: Vec::new(),
                            });
                        }
                    }
                }
                CtrlReq::DisplayMenu(menu_def, x, y) => {
                    let menu = parse_menu_definition(&menu_def, x, y);
                    if !menu.items.is_empty() {
                        app.mode = Mode::MenuMode { menu };
                    }
                }
                CtrlReq::DisplayPopup(command, width, height, close_on_exit) => {
                    if !command.is_empty() {
                        // Try to spawn with PTY for interactive programs (fzf, etc.)
                        let pty_result = portable_pty::PtySystemSelection::default()
                            .get()
                            .ok()
                            .and_then(|pty_sys| {
                                let pty_size = portable_pty::PtySize { rows: height.saturating_sub(2), cols: width.saturating_sub(2), pixel_width: 0, pixel_height: 0 };
                                let pair = pty_sys.openpty(pty_size).ok()?;
                                let mut cmd_builder = portable_pty::CommandBuilder::new(if cfg!(windows) { "cmd" } else { "sh" });
                                if cfg!(windows) { cmd_builder.args(["/C", &command]); } else { cmd_builder.args(["-c", &command]); }
                                let child = pair.slave.spawn_command(cmd_builder).ok()?;
                                // Close the slave handle immediately – required for ConPTY.
                                drop(pair.slave);
                                let term = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(pty_size.rows, pty_size.cols, 0)));
                                let term_reader = term.clone();
                                if let Ok(mut reader) = pair.master.try_clone_reader() {
                                    std::thread::spawn(move || {
                                        let mut buf = [0u8; 8192];
                                        loop {
                                            match reader.read(&mut buf) {
                                                Ok(n) if n > 0 => { let mut p = term_reader.lock().unwrap(); p.process(&buf[..n]); }
                                                _ => break,
                                            }
                                        }
                                    });
                                }
                                Some(PopupPty { master: pair.master, child, term })
                            });
                        
                        app.mode = Mode::PopupMode {
                            command: command.clone(),
                            output: String::new(),
                            process: None,
                            width,
                            height,
                            close_on_exit,
                            popup_pty: pty_result,
                        };
                    } else {
                        app.mode = Mode::PopupMode {
                            command: String::new(),
                            output: "Press 'q' or Escape to close\n".to_string(),
                            process: None,
                            width,
                            height,
                            close_on_exit: true,
                            popup_pty: None,
                        };
                    }
                }
                CtrlReq::ConfirmBefore(prompt, cmd) => {
                    let prompt_text = if prompt.is_empty() {
                        format!("Confirm: {} (y/n)?", cmd)
                    } else {
                        format!("{} (y/n)?", prompt)
                    };
                    app.mode = Mode::ConfirmMode {
                        prompt: prompt_text,
                        command: cmd,
                        input: String::new(),
                    };
                }
                CtrlReq::ResizePaneAbsolute(axis, size) => {
                    resize_pane_absolute(&mut app, &axis, size);
                }
                CtrlReq::ShowOptionValue(resp, name) => {
                    let val = get_option_value(&app, &name);
                    let _ = resp.send(val);
                }
                CtrlReq::ChooseBuffer(resp) => {
                    let mut output = String::new();
                    for (i, buf) in app.paste_buffers.iter().enumerate() {
                        let preview: String = buf.chars().take(50).collect();
                        let preview = preview.replace('\n', "\\n").replace('\r', "");
                        output.push_str(&format!("buffer{}: {} bytes: \"{}\"\n", i, buf.len(), preview));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ServerInfo(resp) => {
                    let info = format!(
                        "psmux {} (Windows)\npid: {}\nsession: {}\nwindows: {}\nuptime: {}s\nsocket: {}",
                        VERSION,
                        std::process::id(),
                        app.session_name,
                        app.windows.len(),
                        (chrono::Local::now() - app.created_at).num_seconds(),
                        {
                            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                            format!("{}\\.psmux\\{}.port", home, app.port_file_base())
                        }
                    );
                    let _ = resp.send(info);
                }
                CtrlReq::SendPrefix => {
                    // Send the prefix key to the active pane as if typed
                    let prefix = app.prefix_key;
                    let encoded: Vec<u8> = match prefix.0 {
                        crossterm::event::KeyCode::Char(c) if prefix.1.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            vec![(c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1)]
                        }
                        crossterm::event::KeyCode::Char(c) => format!("{}", c).into_bytes(),
                        _ => vec![],
                    };
                    if !encoded.is_empty() {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = p.master.write_all(&encoded);
                            let _ = p.master.flush();
                        }
                    }
                }
                CtrlReq::PrevLayout => {
                    // Cycle layouts in reverse using the same logic as cycle_layout/NextLayout
                    static LAYOUTS: [&str; 5] = ["even-horizontal", "even-vertical", "main-horizontal", "main-vertical", "tiled"];
                    let win = &app.windows[app.active_idx];
                    let current_idx = match &win.root {
                        Node::Leaf(_) => 0,
                        Node::Split { kind, sizes, .. } => {
                            if sizes.is_empty() { 0 }
                            else if sizes.iter().all(|s| *s == sizes[0]) {
                                match kind { LayoutKind::Horizontal => 0, LayoutKind::Vertical => 1 }
                            } else if sizes.len() >= 2 && sizes[0] > sizes[1] {
                                match kind { LayoutKind::Vertical => 2, LayoutKind::Horizontal => 3 }
                            } else { 4 }
                        }
                    };
                    let prev_idx = (current_idx + LAYOUTS.len() - 1) % LAYOUTS.len();
                    apply_layout(&mut app, LAYOUTS[prev_idx]);
                    state_dirty = true;
                }
                CtrlReq::FocusIn => {
                    if app.focus_events {
                        // Forward focus-in escape sequence to all panes in active window
                        let win = &mut app.windows[app.active_idx];
                        fn send_focus_seq(node: &mut Node, seq: &[u8]) {
                            match node {
                                Node::Leaf(p) => { let _ = p.master.write_all(seq); let _ = p.master.flush(); }
                                Node::Split { children, .. } => { for c in children { send_focus_seq(c, seq); } }
                            }
                        }
                        send_focus_seq(&mut win.root, b"\x1b[I");
                    }
                    hook_event = Some("pane-focus-in");
                }
                CtrlReq::FocusOut => {
                    if app.focus_events {
                        let win = &mut app.windows[app.active_idx];
                        fn send_focus_seq(node: &mut Node, seq: &[u8]) {
                            match node {
                                Node::Leaf(p) => { let _ = p.master.write_all(seq); let _ = p.master.flush(); }
                                Node::Split { children, .. } => { for c in children { send_focus_seq(c, seq); } }
                            }
                        }
                        send_focus_seq(&mut win.root, b"\x1b[O");
                    }
                    hook_event = Some("pane-focus-out");
                }
                CtrlReq::CommandPrompt(initial) => {
                    app.mode = Mode::CommandPrompt { input: initial.clone(), cursor: initial.len() };
                    state_dirty = true;
                }
                CtrlReq::ShowMessages(resp) => {
                    // Return message log (tmux stores recent log messages)
                    let _ = resp.send(String::new());
                }
                CtrlReq::ResizeWindow(_dim, _size) => {
                    // On Windows, window size is controlled by the terminal emulator;
                    // resize-window is a no-op since we adapt to the terminal size.
                }
                CtrlReq::RespawnWindow => {
                    // Kill all panes in the active window and respawn	
                    respawn_active_pane(&mut app)?;
                    state_dirty = true;
                }
            }
            // Fire any hooks registered for the event that just occurred
            if let Some(event) = hook_event {
                let cmds: Vec<String> = app.hooks.get(event).cloned().unwrap_or_default();
                for cmd in cmds {
                    parse_config_line(&mut app, &cmd);
                }
            }
            if mutates_state {
                state_dirty = true;
            }
        }
            }
        }
        // Check if all windows/panes have exited
        let (all_empty, any_pruned) = tree::reap_children(&mut app)?;
        if any_pruned {
            // A pane exited naturally - resize remaining panes to fill the space
            resize_all_panes(&mut app);
            state_dirty = true;
            meta_dirty = true;
        }
        if all_empty {
            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
            let regpath = format!("{}\\.psmux\\{}.port", home, app.port_file_base());
            let keypath = format!("{}\\.psmux\\{}.key", home, app.port_file_base());
            let _ = std::fs::remove_file(&regpath);
            let _ = std::fs::remove_file(&keypath);
            break;
        }
        // recv_timeout already handles the wait; no additional sleep needed.
    }
    Ok(())
}

/// Apply a set-option command. If `quiet` is true, unknown options are silently ignored.
fn apply_set_option(app: &mut AppState, option: &str, value: &str, quiet: bool) {
    match option {
        "status-left" => { app.status_left = value.to_string(); }
        "status-right" => { app.status_right = value.to_string(); }
        "status-left-length" | "status-right-length" => {
            // Store for format truncation (tmux-compatible)
            app.environment.insert(format!("@{}", option), value.to_string());
        }
        "base-index" => {
            if let Ok(idx) = value.parse::<usize>() {
                app.window_base_index = idx;
            }
        }
        "pane-base-index" => {
            if let Ok(idx) = value.parse::<usize>() {
                app.pane_base_index = idx;
            }
        }
        "mouse" => { app.mouse_enabled = value == "on" || value == "true" || value == "1"; }
        "prefix" => {
            if let Some(kc) = crate::config::parse_key_string(value) {
                app.prefix_key = kc;
            }
        }
        "escape-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.escape_time_ms = ms;
            }
        }
        "history-limit" => {
            if let Ok(limit) = value.parse::<usize>() {
                app.history_limit = limit;
            }
        }
        "display-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.display_time_ms = ms;
            }
        }
        "display-panes-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.display_panes_time_ms = ms;
            }
        }
        "repeat-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.repeat_time_ms = ms;
            }
        }
        "mode-keys" => { app.mode_keys = value.to_string(); }
        "status" => { app.status_visible = matches!(value, "on" | "true" | "1" | "2"); }
        "status-position" => { app.status_position = value.to_string(); }
        "status-style" => { app.status_style = value.to_string(); }
        // Deprecated but ubiquitous: map status-bg/status-fg to status-style
        "status-bg" => {
            let current = &app.status_style;
            let filtered: String = current.split(',')
                .filter(|s| !s.trim().starts_with("bg="))
                .collect::<Vec<_>>().join(",");
            app.status_style = if filtered.is_empty() {
                format!("bg={}", value)
            } else {
                format!("{},bg={}", filtered, value)
            };
        }
        "status-fg" => {
            let current = &app.status_style;
            let filtered: String = current.split(',')
                .filter(|s| !s.trim().starts_with("fg="))
                .collect::<Vec<_>>().join(",");
            app.status_style = if filtered.is_empty() {
                format!("fg={}", value)
            } else {
                format!("{},fg={}", filtered, value)
            };
        }
        "focus-events" => { app.focus_events = matches!(value, "on" | "true" | "1"); }
        "renumber-windows" => { app.renumber_windows = matches!(value, "on" | "true" | "1"); }
        "remain-on-exit" => { app.remain_on_exit = matches!(value, "on" | "true" | "1"); }
        "set-titles" => { app.set_titles = matches!(value, "on" | "true" | "1"); }
        "set-titles-string" => { app.set_titles_string = value.to_string(); }
        "default-command" | "default-shell" => { app.default_shell = value.to_string(); }
        "word-separators" => { app.word_separators = value.to_string(); }
        "aggressive-resize" => { app.aggressive_resize = matches!(value, "on" | "true" | "1"); }
        "monitor-activity" => { app.monitor_activity = matches!(value, "on" | "true" | "1"); }
        "visual-activity" => { app.visual_activity = matches!(value, "on" | "true" | "1"); }
        "synchronize-panes" => { app.sync_input = matches!(value, "on" | "true" | "1"); }
        "automatic-rename" => {
            app.automatic_rename = matches!(value, "on" | "true" | "1");
            // When user explicitly enables automatic-rename, clear manual_rename
            // on the active window so auto-rename can take effect again.
            if app.automatic_rename {
                if let Some(w) = app.windows.get_mut(app.active_idx) {
                    w.manual_rename = false;
                }
            }
        }
        "prediction-dimming" | "dim-predictions" => {
            app.prediction_dimming = !matches!(value, "off" | "false" | "0");
        }
        "cursor-style" => { std::env::set_var("PSMUX_CURSOR_STYLE", value); }
        "cursor-blink" => { std::env::set_var("PSMUX_CURSOR_BLINK", if matches!(value, "on"|"true"|"1") { "1" } else { "0" }); }
        "pane-border-style" => { app.pane_border_style = value.to_string(); }
        "pane-active-border-style" => { app.pane_active_border_style = value.to_string(); }
        "window-status-format" => { app.window_status_format = value.to_string(); }
        "window-status-current-format" => { app.window_status_current_format = value.to_string(); }
        "window-status-separator" => { app.window_status_separator = value.to_string(); }
        "window-status-style" => { app.window_status_style = value.to_string(); }
        "window-status-current-style" => { app.window_status_current_style = value.to_string(); }
        "window-status-activity-style" => { app.window_status_activity_style = value.to_string(); }
        "window-status-bell-style" => { app.window_status_bell_style = value.to_string(); }
        "window-status-last-style" => { app.window_status_last_style = value.to_string(); }
        "mode-style" => { app.mode_style = value.to_string(); }
        "message-style" => { app.message_style = value.to_string(); }
        "message-command-style" => { app.message_command_style = value.to_string(); }
        "status-left-style" => { app.status_left_style = value.to_string(); }
        "status-right-style" => { app.status_right_style = value.to_string(); }
        "status-justify" => { app.status_justify = value.to_string(); }
        "status-interval" => {
            if let Ok(n) = value.parse::<u64>() { app.status_interval = n; }
        }
        "main-pane-width" | "main-pane-height" => {
            app.environment.insert(format!("@{}", option), value.to_string());
        }
        _ => {
            // Store @user-options (used by plugins like tmux-resurrect, tmux-continuum)
            if option.starts_with('@') {
                app.environment.insert(option.to_string(), value.to_string());
            } else if !quiet {
                // Unknown option — only emit warning if not -q mode
                eprintln!("set-option: unknown option '{}'", option);
            }
        }
    }
}
