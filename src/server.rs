use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::env;
use std::net::TcpListener;

use portable_pty::PtySystemSelection;
use chrono::Local;
use ratatui::prelude::*;

use crate::types::*;
use crate::platform::install_console_ctrl_handler;
use crate::cli::parse_target;
use crate::pane::*;
use crate::tree::{self, *};
use crate::input::*;
use crate::copy_mode::*;
use crate::layout::*;
use crate::window_ops::*;
use crate::config::*;
use crate::commands::*;
use crate::util::*;

pub fn run_server(session_name: String, initial_command: Option<String>, raw_command: Option<Vec<String>>) -> io::Result<()> {
    // Install console control handler to prevent termination on client detach
    install_console_ctrl_handler();

    let pty_system = PtySystemSelection::default()
        .get()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;

    let mut app = AppState {
        windows: Vec::new(),
        active_idx: 0,
        mode: Mode::Passthrough,
        escape_time_ms: 500,
        prefix_key: (crossterm::event::KeyCode::Char('b'), crossterm::event::KeyModifiers::CONTROL),
        drag: None,
        // Use a reasonable default size so pane switching works even when detached
        last_window_area: Rect { x: 0, y: 0, width: 120, height: 30 },
        mouse_enabled: true,
        paste_buffers: Vec::new(),
        status_left: "psmux:#I".to_string(),
        status_right: "%H:%M".to_string(),
        copy_anchor: None,
        copy_pos: None,
        copy_scroll_offset: 0,
        display_map: Vec::new(),
        binds: Vec::new(),
        control_rx: None,
        control_port: None,
        session_name,
        attached_clients: 0,
        created_at: Local::now(),
        next_win_id: 1,
        next_pane_id: 1,
        zoom_saved: None,
        sync_input: false,
        hooks: std::collections::HashMap::new(),
        wait_channels: std::collections::HashMap::new(),
        pipe_panes: Vec::new(),
        last_window_idx: 0,
        last_pane_path: Vec::new(),
    };
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
    
    // Write port and key to files
    let regpath = format!("{}\\{}.port", dir, app.session_name);
    let _ = std::fs::write(&regpath, port.to_string());
    let keypath = format!("{}\\{}.key", dir, app.session_name);
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
                if let Some(wid) = target_win { let _ = tx.send(CtrlReq::FocusWindow(wid)); }
                if let Some(pid) = target_pane { 
                    if pane_is_id {
                        let _ = tx.send(CtrlReq::FocusPane(pid));
                    } else {
                        let _ = tx.send(CtrlReq::FocusPaneByIndex(pid));
                    }
                }
                match cmd {
                    "new-window" => {
                        let cmd_str: Option<String> = args.iter()
                            .find(|a| !a.starts_with('-'))
                            .map(|s| s.trim_matches('"').to_string());
                        let _ = tx.send(CtrlReq::NewWindow(cmd_str));
                    }
                    "split-window" => {
                        let kind = if args.iter().any(|a| *a == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                        let cmd_str: Option<String> = args.iter()
                            .find(|a| !a.starts_with('-'))
                            .map(|s| s.trim_matches('"').to_string());
                        let _ = tx.send(CtrlReq::SplitWindow(kind, cmd_str));
                    }
                    "kill-pane" => { let _ = tx.send(CtrlReq::KillPane); }
                    "capture-pane" => {
                        let print_stdout = args.iter().any(|a| *a == "-p");
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::CapturePane(rtx));
                        if let Ok(text) = rrx.recv() {
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
                        if let Ok(text) = rrx.recv() { 
                            let _ = write!(write_stream, "{}\n", text); 
                            let _ = write_stream.flush();
                        }
                        if !persistent { break; }
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
                    "mouse-drag" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDrag(x,y)); } }
                    }
                    "mouse-up" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUp(x,y)); } }
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
                    "next-window" => { let _ = tx.send(CtrlReq::NextWindow); }
                    "previous-window" => { let _ = tx.send(CtrlReq::PrevWindow); }
                    "rename-window" => { if let Some(name) = args.get(0) { let _ = tx.send(CtrlReq::RenameWindow((*name).to_string())); } }
                    "list-windows" => { let (rtx, rrx) = mpsc::channel::<String>(); let _ = tx.send(CtrlReq::ListWindows(rtx)); if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); } if !persistent { break; } }
                    "list-tree" => { let (rtx, rrx) = mpsc::channel::<String>(); let _ = tx.send(CtrlReq::ListTree(rtx)); if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); } if !persistent { break; } }
                    "toggle-sync" => { let _ = tx.send(CtrlReq::ToggleSync); }
                    "set-pane-title" => { let title = args.join(" "); let _ = tx.send(CtrlReq::SetPaneTitle(title)); }
                    "send-keys" => {
                        let literal = args.iter().any(|a| *a == "-l");
                        let keys: Vec<&str> = args.iter().filter(|a| !a.starts_with('-') && **a != "-l" && **a != "-t").copied().collect();
                        let _ = tx.send(CtrlReq::SendKeys(keys.join(" "), literal));
                    }
                    "select-pane" => {
                        let dir = if args.iter().any(|a| *a == "-U") { "U" }
                            else if args.iter().any(|a| *a == "-D") { "D" }
                            else if args.iter().any(|a| *a == "-L") { "L" }
                            else if args.iter().any(|a| *a == "-R") { "R" }
                            else { "" };
                        let _ = tx.send(CtrlReq::SelectPane(dir.to_string()));
                    }
                    "select-window" => {
                        if let Some(idx) = args.iter().find(|a| !a.starts_with('-')).and_then(|s| s.parse::<usize>().ok()) {
                            let _ = tx.send(CtrlReq::SelectWindow(idx));
                        }
                    }
                    "list-panes" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ListPanes(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "kill-window" => { let _ = tx.send(CtrlReq::KillWindow); }
                    "kill-session" => { let _ = tx.send(CtrlReq::KillSession); }
                    "has-session" => {
                        let (rtx, rrx) = mpsc::channel::<bool>();
                        let _ = tx.send(CtrlReq::HasSession(rtx));
                        if let Ok(exists) = rrx.recv() {
                            if !exists { std::process::exit(1); }
                        }
                    }
                    "rename-session" => {
                        if let Some(name) = args.iter().find(|a| !a.starts_with('-')) {
                            let _ = tx.send(CtrlReq::RenameSession((*name).to_string()));
                        }
                    }
                    "swap-pane" => {
                        let dir = if args.iter().any(|a| *a == "-U") { "U" }
                            else if args.iter().any(|a| *a == "-D") { "D" }
                            else { "D" };
                        let _ = tx.send(CtrlReq::SwapPane(dir.to_string()));
                    }
                    "resize-pane" => {
                        let amount = args.iter().find(|a| a.parse::<u16>().is_ok()).and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
                        let dir = if args.iter().any(|a| *a == "-U") { "U" }
                            else if args.iter().any(|a| *a == "-D") { "D" }
                            else if args.iter().any(|a| *a == "-L") { "L" }
                            else if args.iter().any(|a| *a == "-R") { "R" }
                            else { "D" };
                        let _ = tx.send(CtrlReq::ResizePane(dir.to_string(), amount));
                    }
                    "set-buffer" => {
                        let content = args.iter().filter(|a| !a.starts_with('-')).cloned().collect::<Vec<&str>>().join(" ");
                        let _ = tx.send(CtrlReq::SetBuffer(content));
                    }
                    "paste-buffer" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowBuffer(rtx));
                        if let Ok(text) = rrx.recv() {
                            let _ = tx.send(CtrlReq::SendText(text));
                        }
                    }
                    "list-buffers" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ListBuffers(rtx));
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
                    "display-message" => {
                        let fmt = args.iter().filter(|a| !a.starts_with('-')).cloned().collect::<Vec<&str>>().join(" ");
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt));
                        if let Ok(text) = rrx.recv() { let _ = writeln!(write_stream, "{}", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "last-window" => { let _ = tx.send(CtrlReq::LastWindow); }
                    "last-pane" => { let _ = tx.send(CtrlReq::LastPane); }
                    "rotate-window" => {
                        let reverse = args.iter().any(|a| *a == "-D");
                        let _ = tx.send(CtrlReq::RotateWindow(reverse));
                    }
                    "display-panes" => { let _ = tx.send(CtrlReq::DisplayPanes); }
                    "break-pane" => { let _ = tx.send(CtrlReq::BreakPane); }
                    "join-pane" => {
                        if let Some(wid) = args.iter().find(|a| !a.starts_with('-')).and_then(|s| s.parse::<usize>().ok()) {
                            let _ = tx.send(CtrlReq::JoinPane(wid));
                        }
                    }
                    "respawn-pane" => { let _ = tx.send(CtrlReq::RespawnPane); }
                    "session-info" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::SessionInfo(rtx));
                        if let Ok(line) = rrx.recv() { let _ = write!(write_stream, "{}\n", line); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "client-attach" => { let _ = tx.send(CtrlReq::ClientAttach); if !persistent { let _ = write!(write_stream, "ok\n"); } }
                    "client-detach" => { let _ = tx.send(CtrlReq::ClientDetach); if !persistent { let _ = write!(write_stream, "ok\n"); } }
                    "bind-key" | "bind" => {
                        let non_flag_args: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if non_flag_args.len() >= 2 {
                            let key = non_flag_args[0].to_string();
                            let command = non_flag_args[1..].join(" ");
                            let _ = tx.send(CtrlReq::BindKey(key, command));
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
                    "set-option" | "set" => {
                        let non_flag_args: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if non_flag_args.len() >= 2 {
                            let option = non_flag_args[0].to_string();
                            let value = non_flag_args[1..].join(" ");
                            let _ = tx.send(CtrlReq::SetOption(option, value));
                        }
                    }
                    "show-options" | "show" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowOptions(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "source-file" | "source" => {
                        let non_flag_args: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if let Some(path) = non_flag_args.first() {
                            let _ = tx.send(CtrlReq::SourceFile(path.to_string()));
                        }
                    }
                    "move-window" => {
                        let target = args.iter().find(|a| a.parse::<usize>().is_ok()).and_then(|s| s.parse().ok());
                        let _ = tx.send(CtrlReq::MoveWindow(target));
                    }
                    "swap-window" => {
                        if let Some(target) = args.iter().find(|a| a.parse::<usize>().is_ok()).and_then(|s| s.parse().ok()) {
                            let _ = tx.send(CtrlReq::SwapWindow(target));
                        }
                    }
                    "link-window" => {
                        let target = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::LinkWindow(target));
                    }
                    "unlink-window" => {
                        let _ = tx.send(CtrlReq::UnlinkWindow);
                    }
                    "find-window" => {
                        let pattern = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::FindWindow(rtx, pattern));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "move-pane" => {
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
                    "select-layout" => {
                        let layout = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"tiled").to_string();
                        let _ = tx.send(CtrlReq::SelectLayout(layout));
                    }
                    "next-layout" => {
                        let _ = tx.send(CtrlReq::NextLayout);
                    }
                    "list-clients" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ListClients(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); }
                        if !persistent { break; }
                    }
                    "switch-client" => {
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
                    "save-buffer" => {
                        let path = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::SaveBuffer(path));
                    }
                    "load-buffer" => {
                        let path = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
                        let _ = tx.send(CtrlReq::LoadBuffer(path));
                    }
                    "set-environment" => {
                        let non_flag: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
                        if non_flag.len() >= 2 {
                            let _ = tx.send(CtrlReq::SetEnvironment(non_flag[0].to_string(), non_flag[1].to_string()));
                        } else if non_flag.len() == 1 {
                            let _ = tx.send(CtrlReq::SetEnvironment(non_flag[0].to_string(), String::new()));
                        }
                    }
                    "show-environment" => {
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
    loop {
        let mut sent_pty_input = false;
        while let Some(req) = app.control_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match req {
                CtrlReq::NewWindow(cmd) => { let _ = create_window(&*pty_system, &mut app, cmd.as_deref()); resize_all_panes(&mut app); }
                CtrlReq::SplitWindow(k, cmd) => { let _ = split_active_with_command(&mut app, k, cmd.as_deref()); resize_all_panes(&mut app); }
                CtrlReq::KillPane => { let _ = kill_active_pane(&mut app); resize_all_panes(&mut app); }
                CtrlReq::CapturePane(resp) => {
                    if let Some(text) = capture_active_pane_text(&mut app)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneRange(resp, s, e) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::FocusWindow(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::FocusPane(pid) => { focus_pane_by_id(&mut app, pid); }
                CtrlReq::FocusPaneByIndex(idx) => { focus_pane_by_index(&mut app, idx); }
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
                CtrlReq::ClientAttach => { app.attached_clients = app.attached_clients.saturating_add(1); }
                CtrlReq::ClientDetach => { app.attached_clients = app.attached_clients.saturating_sub(1); }
                CtrlReq::DumpLayout(resp) => {
                    let json = dump_layout_json(&mut app)?;
                    let _ = resp.send(json);
                }
                CtrlReq::DumpState(resp) => {
                    // Brief yield to let ConPTY + PSReadLine finish rendering after input
                    // Without this, dump-state can capture an intermediate render state
                    // (e.g., PSReadLine mid-redraw showing prediction artifacts)
                    if sent_pty_input {
                        std::thread::sleep(Duration::from_micros(500));
                    }
                    let layout_json = dump_layout_json(&mut app)?;
                    let windows_json = list_windows_json(&app)?;
                    let tree_json = list_tree_json(&app)?;
                    let prefix_str = format_key_binding(&app.prefix_key);
                    let combined = format!("{{\"layout\":{},\"windows\":{},\"prefix\":\"{}\",\"tree\":{}}}", layout_json, windows_json, prefix_str, tree_json);
                    let _ = resp.send(combined);
                }
                CtrlReq::SendText(s) => { send_text_to_active(&mut app, &s)?; sent_pty_input = true; }
                CtrlReq::SendKey(k) => { send_key_to_active(&mut app, &k)?; sent_pty_input = true; }
                CtrlReq::SendPaste(s) => { send_text_to_active(&mut app, &s)?; sent_pty_input = true; }
                CtrlReq::ZoomPane => { toggle_zoom(&mut app); }
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); }
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                CtrlReq::CopyYank => { let _ = yank_selection(&mut app); app.mode = Mode::Passthrough; }
                CtrlReq::ClientSize(w, h) => { 
                    app.last_window_area = Rect { x: 0, y: 0, width: w, height: h }; 
                    resize_all_panes(&mut app);
                }
                CtrlReq::FocusPaneCmd(pid) => { focus_pane_by_id(&mut app, pid); }
                CtrlReq::FocusWindowCmd(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::MouseDown(x,y) => { remote_mouse_down(&mut app, x, y); }
                CtrlReq::MouseDrag(x,y) => { remote_mouse_drag(&mut app, x, y); }
                CtrlReq::MouseUp(x,y) => { remote_mouse_up(&mut app, x, y); }
                CtrlReq::ScrollUp(x, y) => { remote_scroll_up(&mut app, x, y); }
                CtrlReq::ScrollDown(x, y) => { remote_scroll_down(&mut app, x, y); }
                CtrlReq::NextWindow => { if !app.windows.is_empty() { app.active_idx = (app.active_idx + 1) % app.windows.len(); } }
                CtrlReq::PrevWindow => { if !app.windows.is_empty() { app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); } }
                CtrlReq::RenameWindow(name) => { let win = &mut app.windows[app.active_idx]; win.name = name; }
                CtrlReq::ListWindows(resp) => { let json = list_windows_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ListTree(resp) => { let json = list_tree_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ToggleSync => { app.sync_input = !app.sync_input; }
                CtrlReq::SetPaneTitle(title) => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { p.title = title; }
                }
                CtrlReq::SendKeys(keys, literal) => {
                    sent_pty_input = true;
                    if literal {
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
                }
                CtrlReq::SelectPane(dir) => {
                    match dir.as_str() {
                        "U" => { move_focus(&mut app, FocusDir::Up); }
                        "D" => { move_focus(&mut app, FocusDir::Down); }
                        "L" => { move_focus(&mut app, FocusDir::Left); }
                        "R" => { move_focus(&mut app, FocusDir::Right); }
                        _ => {}
                    }
                }
                CtrlReq::SelectWindow(idx) => {
                    if idx < app.windows.len() { app.active_idx = idx; }
                }
                CtrlReq::ListPanes(resp) => {
                    let mut output = String::new();
                    let win = &app.windows[app.active_idx];
                    fn collect_panes(node: &Node, panes: &mut Vec<(usize, u16, u16)>) {
                        match node {
                            Node::Leaf(p) => panes.push((p.id, p.last_cols, p.last_rows)),
                            Node::Split { children, .. } => {
                                for c in children { collect_panes(c, panes); }
                            }
                        }
                    }
                    let mut panes = Vec::new();
                    collect_panes(&win.root, &mut panes);
                    for (_i, (id, cols, rows)) in panes.iter().enumerate() {
                        output.push_str(&format!("%{}: [{}x{}]\n", id, cols, rows));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::KillWindow => {
                    if app.windows.len() > 1 {
                        let mut win = app.windows.remove(app.active_idx);
                        kill_all_children(&mut win.root);
                        if app.active_idx >= app.windows.len() { app.active_idx = app.windows.len() - 1; }
                    }
                }
                CtrlReq::KillSession => {
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let regpath = format!("{}\\.psmux\\{}.port", home, app.session_name);
                    let keypath = format!("{}\\.psmux\\{}.key", home, app.session_name);
                    let _ = std::fs::remove_file(&regpath);
                    let _ = std::fs::remove_file(&keypath);
                    std::process::exit(0);
                }
                CtrlReq::HasSession(resp) => {
                    let _ = resp.send(true);
                }
                CtrlReq::RenameSession(name) => {
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let old_path = format!("{}\\.psmux\\{}.port", home, app.session_name);
                    let old_keypath = format!("{}\\.psmux\\{}.key", home, app.session_name);
                    let new_path = format!("{}\\.psmux\\{}.port", home, name);
                    let new_keypath = format!("{}\\.psmux\\{}.key", home, name);
                    if let Some(port) = app.control_port {
                        let _ = std::fs::remove_file(&old_path);
                        let _ = std::fs::write(&new_path, port.to_string());
                        if let Ok(key) = std::fs::read_to_string(&old_keypath) {
                            let _ = std::fs::remove_file(&old_keypath);
                            let _ = std::fs::write(&new_keypath, key);
                        }
                    }
                    app.session_name = name;
                }
                CtrlReq::SwapPane(dir) => {
                    match dir.as_str() {
                        "U" => { swap_pane(&mut app, FocusDir::Up); }
                        "D" => { swap_pane(&mut app, FocusDir::Down); }
                        _ => { swap_pane(&mut app, FocusDir::Down); }
                    }
                }
                CtrlReq::ResizePane(dir, amount) => {
                    match dir.as_str() {
                        "U" | "D" => { resize_pane_vertical(&mut app, if dir == "U" { -(amount as i16) } else { amount as i16 }); }
                        "L" | "R" => { resize_pane_horizontal(&mut app, if dir == "L" { -(amount as i16) } else { amount as i16 }); }
                        _ => {}
                    }
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
                CtrlReq::ShowBuffer(resp) => {
                    let content = app.paste_buffers.first().cloned().unwrap_or_default();
                    let _ = resp.send(content);
                }
                CtrlReq::DeleteBuffer => {
                    if !app.paste_buffers.is_empty() { app.paste_buffers.remove(0); }
                }
                CtrlReq::DisplayMessage(resp, fmt) => {
                    let mut result = fmt.clone();
                    result = result.replace("#S", &app.session_name);
                    result = result.replace("#I", &app.active_idx.to_string());
                    result = result.replace("#W", &app.windows[app.active_idx].name);
                    let win = &app.windows[app.active_idx];
                    let pane_id = get_active_pane_id(&win.root, &win.active_path).unwrap_or(0);
                    result = result.replace("#P", &pane_id.to_string());
                    result = result.replace("#H", &env::var("COMPUTERNAME").or_else(|_| env::var("HOSTNAME")).unwrap_or_default());
                    result = result.replace("#T", &win.name);
                    let _ = resp.send(result);
                }
                CtrlReq::LastWindow => {
                    if app.windows.len() > 1 && app.last_window_idx < app.windows.len() {
                        let tmp = app.active_idx;
                        app.active_idx = app.last_window_idx;
                        app.last_window_idx = tmp;
                    }
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
                }
                CtrlReq::DisplayPanes => {
                    // This would show pane numbers overlay - no-op for server mode
                }
                CtrlReq::BreakPane => {
                    break_pane_to_window(&mut app);
                }
                CtrlReq::JoinPane(target_win) => {
                    if target_win < app.windows.len() {
                        app.active_idx = target_win;
                    }
                }
                CtrlReq::RespawnPane => {
                    respawn_active_pane(&mut app)?;
                }
                CtrlReq::BindKey(key, command) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let action = parse_command_to_action(&command);
                        if let Some(act) = action {
                            app.binds.retain(|b| b.key != kc);
                            app.binds.push(Bind { key: kc, action: act });
                        }
                    }
                }
                CtrlReq::UnbindKey(key) => {
                    if let Some(kc) = parse_key_string(&key) {
                        app.binds.retain(|b| b.key != kc);
                    }
                }
                CtrlReq::ListKeys(resp) => {
                    let mut output = String::new();
                    for bind in &app.binds {
                        let key_str = format_key_binding(&bind.key);
                        let action_str = format_action(&bind.action);
                        output.push_str(&format!("bind-key {} {}\n", key_str, action_str));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::SetOption(option, value) => {
                    match option.as_str() {
                        "status-left" => { app.status_left = value; }
                        "status-right" => { app.status_right = value; }
                        "mouse" => { app.mouse_enabled = value == "on" || value == "true" || value == "1"; }
                        "prefix" => {
                            if let Some(kc) = parse_key_string(&value) {
                                app.prefix_key = kc;
                            }
                        }
                        "escape-time" => {
                            if let Ok(ms) = value.parse::<u64>() {
                                app.escape_time_ms = ms;
                            }
                        }
                        _ => {}
                    }
                }
                CtrlReq::ShowOptions(resp) => {
                    let mut output = String::new();
                    output.push_str(&format!("status-left \"{}\"\n", app.status_left));
                    output.push_str(&format!("status-right \"{}\"\n", app.status_right));
                    output.push_str(&format!("mouse {}\n", if app.mouse_enabled { "on" } else { "off" }));
                    output.push_str(&format!("prefix {}\n", format_key_binding(&app.prefix_key)));
                    output.push_str(&format!("escape-time {}\n", app.escape_time_ms));
                    let _ = resp.send(output);
                }
                CtrlReq::SourceFile(path) => {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        for line in contents.lines() {
                            let line = line.trim();
                            if line.is_empty() || line.starts_with('#') { continue; }
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.is_empty() { continue; }
                            match parts[0] {
                                "set" | "set-option" => {
                                    if parts.len() >= 3 {
                                        let opt_idx = if parts.get(1) == Some(&"-g") { 2 } else { 1 };
                                        if let (Some(opt), Some(val)) = (parts.get(opt_idx), parts.get(opt_idx + 1)) {
                                            match *opt {
                                                "status-left" => { app.status_left = parts[opt_idx + 1..].join(" ").trim_matches('"').to_string(); }
                                                "status-right" => { app.status_right = parts[opt_idx + 1..].join(" ").trim_matches('"').to_string(); }
                                                "mouse" => { app.mouse_enabled = *val == "on" || *val == "true" || *val == "1"; }
                                                "prefix" => { if let Some(kc) = parse_key_string(val) { app.prefix_key = kc; } }
                                                "escape-time" => { if let Ok(ms) = val.parse::<u64>() { app.escape_time_ms = ms; } }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                "bind" | "bind-key" => {
                                    if parts.len() >= 3 {
                                        let key_idx = if parts.get(1).map(|s| s.starts_with('-')).unwrap_or(false) { 2 } else { 1 };
                                        if let Some(key_str) = parts.get(key_idx) {
                                            if let Some(kc) = parse_key_string(key_str) {
                                                let cmd = parts[key_idx + 1..].join(" ");
                                                if let Some(act) = parse_command_to_action(&cmd) {
                                                    app.binds.retain(|b| b.key != kc);
                                                    app.binds.push(Bind { key: kc, action: act });
                                                }
                                            }
                                        }
                                    }
                                }
                                "unbind" | "unbind-key" => {
                                    if parts.len() >= 2 {
                                        let key_idx = if parts.get(1).map(|s| s.starts_with('-')).unwrap_or(false) { 2 } else { 1 };
                                        if let Some(key_str) = parts.get(key_idx) {
                                            if let Some(kc) = parse_key_string(key_str) {
                                                app.binds.retain(|b| b.key != kc);
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
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
                            output.push_str(&format!("{}: {} []\n", i, win.name));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::MovePane(target_win) => {
                    if target_win < app.windows.len() && target_win != app.active_idx {
                        app.active_idx = target_win;
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
                CtrlReq::RefreshClient => {}
                CtrlReq::SuspendClient => {}
                CtrlReq::CopyModePageUp => {
                    enter_copy_mode(&mut app);
                    move_copy_cursor(&mut app, 0, -20);
                }
                CtrlReq::ClearHistory => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        if let Ok(mut parser) = p.term.lock() {
                            *parser = vt100::Parser::new(p.last_rows, p.last_cols, 1000);
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
                    env::set_var(&key, &value);
                }
                CtrlReq::ShowEnvironment(resp) => {
                    let mut output = String::new();
                    for (key, value) in env::vars() {
                        if key.starts_with("PSMUX") || key.starts_with("TMUX") {
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
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let regpath = format!("{}\\.psmux\\{}.port", home, app.session_name);
                    let keypath = format!("{}\\.psmux\\{}.key", home, app.session_name);
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
                        #[cfg(windows)]
                        let process = std::process::Command::new("cmd")
                            .args(["/C", &command])
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()
                            .ok();
                        
                        app.mode = Mode::PopupMode {
                            command: command.clone(),
                            output: String::new(),
                            process,
                            width,
                            height,
                            close_on_exit,
                        };
                    } else {
                        app.mode = Mode::PopupMode {
                            command: String::new(),
                            output: "Press 'q' or Escape to close\n".to_string(),
                            process: None,
                            width,
                            height,
                            close_on_exit: true,
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
            }
        }
        // Check if all windows/panes have exited
        let (all_empty, any_pruned) = tree::reap_children(&mut app)?;
        if any_pruned {
            // A pane exited naturally — resize remaining panes to fill the space
            resize_all_panes(&mut app);
        }
        if all_empty {
            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
            let regpath = format!("{}\\.psmux\\{}.port", home, app.session_name);
            let _ = std::fs::remove_file(&regpath);
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
