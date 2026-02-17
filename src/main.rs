mod types;
mod platform;
mod cli;
mod session;
mod tree;
mod rendering;
mod config;
mod commands;
mod pane;
mod copy_mode;
mod input;
mod layout;
mod window_ops;
mod util;
mod format;
mod server;
mod client;
mod app;

use std::io::{self, Write, Read as _, BufRead as _};
use std::time::Duration;
use std::env;

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use crossterm::terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute};
use crossterm::cursor::{EnableBlinking, DisableBlinking};
use crossterm::event::{EnableMouseCapture, DisableMouseCapture, EnableBracketedPaste, DisableBracketedPaste};

use crate::types::*;
use crate::platform::enable_virtual_terminal_processing;
use crate::cli::*;
use crate::session::*;
use crate::rendering::apply_cursor_style;
use crate::server::run_server;
use crate::client::run_remote;
use crate::util::*;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    
    // Clean up any stale port files at startup
    cleanup_stale_port_files();
    
    // Parse -L flag early (tmux-compatible: names the server socket for namespace isolation)
    // In psmux, -L <name> creates a namespace prefix for session port/key files.
    // Sessions under -L "foo" are stored as "foo__sessionname.port".
    // IMPORTANT: Only recognize -L as a global flag when it appears BEFORE the subcommand.
    // This avoids conflict with subcommand flags (e.g. select-pane -L, resize-pane -L).
    let mut l_socket_name: Option<String> = None;
    {
        let mut i = 1; // skip binary name
        while i < args.len() {
            let arg = &args[i];
            if arg == "-L" && i + 1 < args.len() {
                l_socket_name = Some(args[i + 1].clone());
                i += 2;
            } else if (arg == "-S" || arg == "-f" || arg == "-t") && i + 1 < args.len() {
                i += 2; // skip other global flag-value pairs
            } else if arg.starts_with('-') {
                i += 1; // skip single global flags (e.g. -v, -V)
            } else {
                break; // hit the subcommand name — stop scanning for global flags
            }
        }
    }

    // Parse -t flag early to set target session for all commands
    // Supports session:window.pane format (e.g., "dev:0.1")
    // PSMUX_TARGET_SESSION stores the port file base name (for port file lookup)
    // PSMUX_TARGET_FULL stores the full target (session:window.pane) for the server
    if let Some(pos) = args.iter().position(|a| a == "-t") {
        if let Some(target) = args.get(pos + 1) {
            // Store the full target for the server to parse
            env::set_var("PSMUX_TARGET_FULL", target);
            // Extract just the session name for port file lookup
            let session = extract_session_from_target(target);
            // Apply -L namespace prefix for port file lookup
            let port_file_base = if let Some(ref l) = l_socket_name {
                format!("{}__{}", l, session)
            } else {
                session.clone()
            };
            env::set_var("PSMUX_TARGET_SESSION", &port_file_base);
        }
    } else if env::var("PSMUX_TARGET_SESSION").is_err() {
        // No -t flag: try to resolve session from TMUX env var (set inside psmux panes)
        // TMUX format: /tmp/psmux-<pid>/<socket_name>,<port>,<session_idx>
        if let Ok(tmux_val) = env::var("TMUX") {
            // Extract the port from the TMUX value
            let parts: Vec<&str> = tmux_val.split(',').collect();
            if parts.len() >= 2 {
                if let Ok(port) = parts[1].trim().parse::<u16>() {
                    // Look up which session owns this port (port file base
                    // already includes -L namespace prefix if applicable)
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let psmux_dir = format!("{}\\.psmux", home);
                    if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().map(|e| e == "port").unwrap_or(false) {
                                if let Ok(port_str) = std::fs::read_to_string(&path) {
                                    if let Ok(file_port) = port_str.trim().parse::<u16>() {
                                        if file_port == port {
                                            if let Some(port_file_base) = path.file_stem().and_then(|s| s.to_str()) {
                                                env::set_var("PSMUX_TARGET_SESSION", port_file_base);
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Find the actual command by skipping -t/-L and their arguments
    let cmd_args: Vec<&String> = args.iter().skip(1).filter(|a| {
        if *a == "-t" || *a == "-L" { return false; }
        // Check if previous arg was -t or -L
        if let Some(pos) = args.iter().position(|x| x == *a) {
            if pos > 0 && (args[pos - 1] == "-t" || args[pos - 1] == "-L") { return false; }
        }
        true
    }).collect();
    
    let cmd = cmd_args.first().map(|s| s.as_str()).unwrap_or("");
    
    // Handle help and version flags first
    match cmd {
        "-h" | "--help" | "help" => {
            print_help();
            return Ok(());
        }
        "-V" | "--version" | "version" => {
            print_version();
            return Ok(());
        }
        "list-commands" | "lscm" => {
            print_commands();
            return Ok(());
        }
        _ => {}
    }
    
    match cmd {
        // kill-server MUST be handled early before any potential fall-through
        "kill-server" => {
            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
            let psmux_dir = format!("{}\\.psmux", home);
            // Compute namespace prefix for -L filtering (matches list-sessions behavior)
            let ns_prefix = l_socket_name.as_ref().map(|l| format!("{l}__"));
            let mut sessions_killed = 0;
            if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "port").unwrap_or(false) {
                        if let Some(session_name) = path.file_stem().and_then(|s| s.to_str()) {
                            // Apply -L namespace filtering:
                            // With -L: only kill sessions under that namespace
                            // Without -L: kill ALL sessions (tmux behavior)
                            if let Some(ref pfx) = ns_prefix {
                                if !session_name.starts_with(pfx.as_str()) { continue; }
                            }
                            if let Ok(port_str) = std::fs::read_to_string(&path) {
                                if let Ok(port) = port_str.trim().parse::<u16>() {
                                    let addr = format!("127.0.0.1:{}", port);
                                    let sess_key = read_session_key(session_name).unwrap_or_default();
                                    if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
                                        let _ = write!(stream, "AUTH {}\n", sess_key);
                                        let _ = std::io::Write::write_all(&mut stream, b"kill-server\n");
                                        sessions_killed += 1;
                                    } else {
                                        let _ = std::fs::remove_file(&path);
                                    }
                                }
                            } else {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
            if sessions_killed > 0 {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            return Ok(());
        }
        "ls" | "list-sessions" => {
                let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                let dir = format!("{}\\.psmux", home);
                // Compute namespace prefix for -L filtering
                let ns_prefix = l_socket_name.as_ref().map(|l| format!("{l}__"));
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        if let Some(name) = e.file_name().to_str() {
                            if let Some((base, ext)) = name.rsplit_once('.') {
                                if ext == "port" {
                                    // Filter by -L namespace: when -L is given, only show
                                    // sessions with that prefix; when no -L, only show
                                    // sessions without any namespace prefix
                                    if let Some(ref pfx) = ns_prefix {
                                        if !base.starts_with(pfx.as_str()) { continue; }
                                    } else {
                                        if base.contains("__") { continue; }
                                    }
                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                        if let Ok(_p) = port_str.trim().parse::<u16>() {
                                            let addr = format!("127.0.0.1:{}", port_str.trim());
                                            if let Ok(mut s) = std::net::TcpStream::connect_timeout(
                                                &addr.parse().unwrap(),
                                                Duration::from_millis(50)
                                            ) {
                                                let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
                                                // Read session key and authenticate
                                                let key_path = format!("{}\\.psmux\\{}.key", home, base);
                                                if let Ok(key) = std::fs::read_to_string(&key_path) {
                                                    let _ = std::io::Write::write_all(&mut s, format!("AUTH {}\n", key.trim()).as_bytes());
                                                }
                                                let _ = std::io::Write::write_all(&mut s, b"session-info\n");
                                                let mut br = std::io::BufReader::new(s);
                                                let mut line = String::new();
                                                // Skip "OK" response from AUTH
                                                let _ = br.read_line(&mut line);
                                                if line.trim() == "OK" {
                                                    line.clear();
                                                    let _ = br.read_line(&mut line);
                                                }
                                                if !line.trim().is_empty() && line.trim() != "ERROR: Authentication required" { 
                                                    println!("{}", line.trim_end()); 
                                                } else { 
                                                    println!("{}", base); 
                                                }
                                            } else {
                                                // stale port file - remove it
                                                let _ = std::fs::remove_file(e.path());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(());
            }
            "a" | "at" | "attach" | "attach-session" => {
                let name = args
                    .iter()
                    .position(|a| a == "-t")
                    .and_then(|i| args.get(i + 1))
                    .map(|s| s.clone())
                    .or_else(resolve_default_session_name)
                    .or_else(resolve_last_session_name)
                    .unwrap_or_else(|| "default".to_string());
                env::set_var("PSMUX_SESSION_NAME", name);
                env::set_var("PSMUX_REMOTE_ATTACH", "1");
            }
            "server" => {
                // Internal command - run headless server (used when spawning background server)
                let name = args.iter().position(|a| a == "-s").and_then(|i| args.get(i+1)).map(|s| s.clone()).unwrap_or_else(|| "default".to_string());
                // Parse -L socket name for namespace isolation
                let server_socket_name = args.iter().position(|a| a == "-L").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Check for initial command via -c flag (shell-wrapped)
                let initial_cmd = args.iter().position(|a| a == "-c").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Check for raw command after -- (direct execution)
                let raw_cmd: Option<Vec<String>> = args.iter().position(|a| a == "--").map(|pos| {
                    args.iter().skip(pos + 1).cloned().collect()
                }).filter(|v: &Vec<String>| !v.is_empty());
                return run_server(name, server_socket_name, initial_cmd, raw_cmd);
            }
            "new-session" | "new" => {
                let name = cmd_args.iter().position(|a| *a == "-s").and_then(|i| cmd_args.get(i+1)).map(|s| s.to_string())
                    .unwrap_or_else(|| "default".to_string());
                // Compute port file base name: with -L namespace prefix if specified
                let port_file_base = if let Some(ref l) = l_socket_name {
                    format!("{}__{}", l, name)
                } else {
                    name.clone()
                };
                let detached = cmd_args.iter().any(|a| *a == "-d");
                let print_info = cmd_args.iter().any(|a| *a == "-P");
                let format_str: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].trim_matches('"').to_string());
                // Check for -- separator: everything after it is a raw command (direct execution)
                let dash_dash_pos = cmd_args.iter().position(|a| *a == "--");
                let raw_cmd_args: Option<Vec<String>> = dash_dash_pos.map(|pos| {
                    cmd_args.iter().skip(pos + 1).map(|s| s.to_string()).collect()
                }).filter(|v: &Vec<String>| !v.is_empty());
                // Parse initial command - look for trailing arguments after all flags (legacy mode, no --)
                // cmd_args[0] is the command name, so we skip it
                let initial_cmd: Option<String> = if raw_cmd_args.is_some() { None } else {
                    let mut skip_next = false;
                    let mut cmd_parts: Vec<&str> = Vec::new();
                    for (i, arg) in cmd_args.iter().enumerate().skip(1) { // Skip command name
                        if skip_next { skip_next = false; continue; }
                        if *arg == "-s" || *arg == "-t" || *arg == "-F" { skip_next = true; continue; }
                        if arg.starts_with('-') { continue; }
                        // This arg and all following are the command
                        cmd_parts.extend(cmd_args.iter().skip(i).map(|s| s.as_str()));
                        break;
                    }
                    if cmd_parts.is_empty() { None } else { Some(cmd_parts.join(" ")) }
                };
                
                // Check if session already exists AND is actually running
                let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                let port_path = format!("{}\\.psmux\\{}.port", home, port_file_base);
                if std::path::Path::new(&port_path).exists() {
                    // Verify server is actually running
                    let server_alive = if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                        if let Ok(port) = port_str.trim().parse::<u16>() {
                            let addr = format!("127.0.0.1:{}", port);
                            std::net::TcpStream::connect_timeout(
                                &addr.parse().unwrap(),
                                Duration::from_millis(100)
                            ).is_ok()
                        } else { false }
                    } else { false };
                    
                    if server_alive {
                        eprintln!("psmux: session '{}' already exists", name);
                        return Ok(());
                    } else {
                        // Stale port file - remove it and continue
                        let _ = std::fs::remove_file(&port_path);
                    }
                }
                
                // Always spawn a background server first
                let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
                let mut server_args: Vec<String> = vec!["server".into(), "-s".into(), name.clone()];
                // Pass -L socket name to server for namespace isolation
                if let Some(ref l) = l_socket_name {
                    server_args.push("-L".into());
                    server_args.push(l.clone());
                }
                // Pass initial command if provided
                if let Some(ref init_cmd) = initial_cmd {
                    server_args.push("-c".into());
                    server_args.push(init_cmd.clone());
                }
                // Pass raw command args (direct execution) if -- was used
                if let Some(ref raw_args) = raw_cmd_args {
                    server_args.push("--".into());
                    for a in raw_args {
                        server_args.push(a.clone());
                    }
                }
                // On Windows, mark parent's stdout/stderr as non-inheritable before
                // spawning the server. This prevents the server from inheriting
                // PowerShell's redirect pipes (which would cause the parent to hang
                // waiting for the pipe to close). The server creates its own ConPTY
                // handles so it doesn't need the parent's stdio.
                #[cfg(windows)]
                {
                    #[link(name = "kernel32")]
                    extern "system" {
                        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
                        fn SetHandleInformation(hObject: *mut std::ffi::c_void, dwMask: u32, dwFlags: u32) -> i32;
                    }
                    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5u32; // -11i32 as u32
                    const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;  // -12i32 as u32
                    const HANDLE_FLAG_INHERIT: u32 = 0x00000001;
                    unsafe {
                        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
                        let stderr = GetStdHandle(STD_ERROR_HANDLE);
                        SetHandleInformation(stdout, HANDLE_FLAG_INHERIT, 0);
                        SetHandleInformation(stderr, HANDLE_FLAG_INHERIT, 0);
                    }
                }
                // Spawn server with a hidden console window via CreateProcessW.
                // This gives ConPTY a real console while keeping the window invisible.
                #[cfg(windows)]
                crate::platform::spawn_server_hidden(&exe, &server_args)?;
                #[cfg(not(windows))]
                {
                    let mut cmd = std::process::Command::new(&exe);
                    for a in &server_args { cmd.arg(a); }
                    cmd.stdin(std::process::Stdio::null());
                    cmd.stdout(std::process::Stdio::null());
                    cmd.stderr(std::process::Stdio::null());
                    let _child = cmd.spawn().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to spawn server: {e}")))?;
                }
                
                // Wait for server to create port file (up to 2 seconds)
                for _ in 0..20 {
                    if std::path::Path::new(&port_path).exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                
                if detached {
                    // If -P flag, print pane info before returning
                    if print_info {
                        // Set target session so send_control_with_response connects to the right server
                        env::set_var("PSMUX_TARGET_SESSION", &port_file_base);
                        // Give server a moment to initialize
                        std::thread::sleep(Duration::from_millis(200));
                        // Query the server for pane info using display-message
                        let fmt = if let Some(ref f) = format_str {
                            f.clone()
                        } else {
                            // tmux default: new-session -P prints "session_name:"
                            "#{session_name}:".to_string()
                        };
                        match send_control_with_response(format!("display-message -p {}\n", fmt)) {
                            Ok(resp) => { let trimmed = resp.trim(); if !trimmed.is_empty() { println!("{}", trimmed); } }
                            Err(_) => {}
                        }
                    }
                    return Ok(());
                } else {
                    // User wants attached session - set env vars to attach
                    env::set_var("PSMUX_SESSION_NAME", &port_file_base);
                    env::set_var("PSMUX_REMOTE_ATTACH", "1");
                    // Continue to attach below...
                }
            }
            "new-window" | "neww" => {
                // Parse -n name flag
                let name_arg: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-n").map(|w| w[1].trim_matches('"').to_string());
                let detached = cmd_args.iter().any(|a| *a == "-d");
                let print_info = cmd_args.iter().any(|a| *a == "-P");
                // Parse -F format string
                let format_str: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].trim_matches('"').to_string());
                // Parse -c start_dir flag
                let start_dir: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
                // Parse command — first non-flag argument, excluding -n/-t/-c/-F values
                let cmd_arg = cmd_args.iter().skip(1)
                    .filter(|a| !a.starts_with('-'))
                    .find(|a| {
                        // Exclude values of -n, -t, -c, -F flags
                        !cmd_args.windows(2).any(|w| (w[0] == "-n" || w[0] == "-t" || w[0] == "-c" || w[0] == "-F") && w[1] == **a)
                    })
                    .map(|s| s.as_str()).unwrap_or("");
                let mut cmd_line = "new-window".to_string();
                if detached { cmd_line.push_str(" -d"); }
                if print_info { cmd_line.push_str(" -P"); }
                if let Some(ref fmt) = format_str {
                    cmd_line.push_str(&format!(" -F \"{}\"", fmt.replace("\"", "\\\"")));
                }
                if let Some(name) = &name_arg {
                    cmd_line.push_str(&format!(" -n \"{}\"", name.replace("\"", "\\\"")));
                }
                if let Some(dir) = &start_dir {
                    cmd_line.push_str(&format!(" -c \"{}\"", dir.replace("\"", "\\\"")));
                }
                if !cmd_arg.is_empty() {
                    cmd_line.push_str(&format!(" \"{}\"", cmd_arg.replace("\"", "\\\"")));
                }
                cmd_line.push('\n');
                if print_info {
                    let resp = send_control_with_response(cmd_line)?;
                    print!("{}", resp);
                } else {
                    send_control(cmd_line)?;
                }
                return Ok(());
            }
            "split-window" | "splitw" => {
                let flag = if cmd_args.iter().any(|a| *a == "-h") { "-h" } else { "-v" };
                let detached = cmd_args.iter().any(|a| *a == "-d");
                let print_info = cmd_args.iter().any(|a| *a == "-P");
                // Parse -F format string
                let format_str: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-F").map(|w| w[1].trim_matches('"').to_string());
                // Parse -c start_dir, -p percentage, -l percentage flags
                let start_dir: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
                let size_pct: Option<String> = cmd_args.windows(2).find(|w| w[0] == "-p" || w[0] == "-l").map(|w| w[1].to_string());
                // Parse command after flags (first non-flag argument, skipping command name at cmd_args[0])
                let cmd_arg = cmd_args.iter().skip(1)
                    .find(|a| !a.starts_with('-') && !cmd_args.windows(2).any(|w| (w[0] == "-c" || w[0] == "-p" || w[0] == "-l" || w[0] == "-F") && w[1] == **a))
                    .map(|s| s.as_str()).unwrap_or("");
                let mut cmd_line = format!("split-window {}", flag);
                if detached { cmd_line.push_str(" -d"); }
                if print_info { cmd_line.push_str(" -P"); }
                if let Some(ref fmt) = format_str {
                    cmd_line.push_str(&format!(" -F \"{}\"", fmt.replace("\"", "\\\"")));
                }
                if let Some(dir) = &start_dir {
                    cmd_line.push_str(&format!(" -c \"{}\"", dir.replace("\"", "\\\"")));
                }
                if let Some(pct) = &size_pct {
                    cmd_line.push_str(&format!(" -p {}", pct));
                }
                if !cmd_arg.is_empty() {
                    cmd_line.push_str(&format!(" \"{}\"", cmd_arg.replace("\"", "\\\"")));
                }
                cmd_line.push('\n');
                if print_info {
                    let resp = send_control_with_response(cmd_line)?;
                    print!("{}", resp);
                } else {
                    send_control(cmd_line)?;
                }
                return Ok(());
            }
            "kill-pane" | "killp" => { send_control("kill-pane\n".to_string())?; return Ok(()); }
            "capture-pane" | "capturep" => {
                // Parse optional flags - cmd_args[0] is command, start from 1
                let mut cmd = "capture-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(target) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", target));
                                i += 1;
                            }
                        }
                        "-S" => {
                            if let Some(start) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -S {}", start));
                                i += 1;
                            }
                        }
                        "-E" => {
                            if let Some(end) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -E {}", end));
                                i += 1;
                            }
                        }
                        "-p" => { cmd.push_str(" -p"); }
                        "-e" => { cmd.push_str(" -e"); }
                        "-J" => { cmd.push_str(" -J"); }
                        "-b" => {
                            if let Some(buf) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -b {}", buf));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // send-keys - Send keys to a pane (critical for scripting)
            "send-keys" | "send" | "send-key" => {
                let mut literal = false;
                let mut keys: Vec<String> = Vec::new();
                // Skip the command itself (index 0 in cmd_args), start at index 1
                for i in 1..cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-l" => { literal = true; }
                        "-R" => { keys.push("__RESET__".to_string()); }
                        "-t" => { } // already handled
                        _ => { keys.push(cmd_args[i].to_string()); }
                    }
                }
                let mut cmd = "send-keys".to_string();
                if literal { cmd.push_str(" -l"); }
                // Quote arguments that contain spaces to preserve them
                for k in keys { 
                    if k.contains(' ') || k.contains('\t') {
                        // Escape any existing quotes and wrap in quotes
                        let escaped = k.replace('\\', "\\\\").replace('"', "\\\"");
                        cmd.push_str(&format!(" \"{}\"", escaped));
                    } else {
                        cmd.push_str(&format!(" {}", k)); 
                    }
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // select-pane - Select the active pane
            "select-pane" | "selectp" => {
                let mut cmd = "select-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-L" => { cmd.push_str(" -L"); }
                        "-R" => { cmd.push_str(" -R"); }
                        "-l" => { cmd.push_str(" -l"); }
                        "-Z" => { cmd.push_str(" -Z"); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // select-window - Select a window
            "select-window" | "selectw" => {
                let mut cmd = "select-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-l" => { cmd.push_str(" -l"); }
                        "-n" => { cmd.push_str(" -n"); }
                        "-p" => { cmd.push_str(" -p"); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // list-panes - List all panes
            "list-panes" | "lsp" => {
                let mut cmd = "list-panes".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { cmd.push_str(" -a"); }
                        "-s" => { cmd.push_str(" -s"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-F" => {
                            if let Some(f) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -F \"{}\"", f.trim_matches('"').replace("\"", "\\\"")));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // list-windows - List all windows
            "list-windows" | "lsw" => {
                let mut cmd = "list-windows".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { cmd.push_str(" -a"); }
                        "-J" => { cmd.push_str(" -J"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // kill-window - Kill a window
            "kill-window" | "killw" => {
                let mut cmd = "kill-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-a" => { cmd.push_str(" -a"); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // kill-session - Kill a session
            "kill-session" | "kill-ses" => {
                let mut target: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                // Apply -L namespace prefix for port file lookup
                                let namespaced = if let Some(ref l) = l_socket_name {
                                    format!("{}__{}", l, t)
                                } else {
                                    t.to_string()
                                };
                                target = Some(namespaced);
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let session_name = target.clone().unwrap_or_else(|| {
                    env::var("PSMUX_TARGET_SESSION").unwrap_or_else(|_| {
                        // Apply -L namespace prefix to default
                        if let Some(ref l) = l_socket_name {
                            format!("{}__{}", l, "default")
                        } else {
                            "default".to_string()
                        }
                    })
                });
                if let Some(ref t) = target {
                    env::set_var("PSMUX_TARGET_SESSION", t);
                }
                // Try to send kill command to server
                if send_control("kill-session\n".to_string()).is_err() {
                    // Server not responding - clean up stale port file
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let port_path = format!("{}\\.psmux\\{}.port", home, session_name);
                    let _ = std::fs::remove_file(&port_path);
                }
                return Ok(());
            }
            // has-session - Check if session exists (for scripting)
            "has-session" | "has" => {
                // Get target from env (set from -t flag) or from remaining args
                let target = env::var("PSMUX_TARGET_SESSION").unwrap_or_else(|_| {
                    // Try to get session name from cmd_args
                    let mut t = "default".to_string();
                    let mut i = 1;
                    while i < cmd_args.len() {
                        if cmd_args[i].as_str() == "-t" {
                            if let Some(v) = cmd_args.get(i + 1) { t = v.to_string(); }
                            i += 1;
                        } else if !cmd_args[i].starts_with('-') {
                            t = cmd_args[i].to_string();
                            break;
                        }
                        i += 1;
                    }
                    // Apply -L namespace prefix for port file lookup
                    if let Some(ref l) = l_socket_name {
                        format!("{}__{}", l, t)
                    } else {
                        t
                    }
                });
                let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                let path = format!("{}\\.psmux\\{}.port", home, target);
                if let Ok(port_str) = std::fs::read_to_string(&path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let addr = format!("127.0.0.1:{}", port);
                        // Actually authenticate and query the server to ensure it's healthy
                        let session_key = read_session_key(&target).unwrap_or_default();
                        if let Ok(mut s) = std::net::TcpStream::connect_timeout(
                            &addr.parse().unwrap(),
                            Duration::from_millis(500)
                        ) {
                            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                            let _ = write!(s, "AUTH {}\n", session_key);
                            let _ = write!(s, "session-info\n");
                            let _ = s.flush();
                            let mut buf = [0u8; 256];
                            if let Ok(n) = std::io::Read::read(&mut s, &mut buf) {
                                if n > 0 {
                                    let resp = String::from_utf8_lossy(&buf[..n]);
                                    if resp.contains("OK") {
                                        std::process::exit(0);
                                    }
                                }
                            }
                            // Fallback: connection succeeded so session likely exists
                            std::process::exit(0);
                        } else {
                            // Stale port file - clean it up
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                std::process::exit(1);
            }
            // rename-session - Rename a session
            "rename-session" | "rename" => {
                let mut new_name: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    if !cmd_args[i].starts_with('-') {
                        new_name = Some(cmd_args[i].to_string());
                        break;
                    }
                    i += 1;
                }
                if let Some(name) = new_name {
                    send_control(format!("rename-session {}\n", name))?;
                }
                return Ok(());
            }
            // swap-pane - Swap panes
            "swap-pane" | "swapp" => {
                let mut cmd = "swap-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // resize-pane - Resize a pane
            "resize-pane" | "resizep" => {
                let mut cmd = "resize-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-L" => { cmd.push_str(" -L"); }
                        "-R" => { cmd.push_str(" -R"); }
                        "-Z" => { cmd.push_str(" -Z"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-x" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -x {}", v));
                                i += 1;
                            }
                        }
                        "-y" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -y {}", v));
                                i += 1;
                            }
                        }
                        s if s.parse::<i32>().is_ok() => {
                            cmd.push_str(&format!(" {}", s));
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // paste-buffer - Paste buffer into pane
            "paste-buffer" | "pasteb" => {
                let mut cmd = "paste-buffer".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -b {}", b));
                                i += 1;
                            }
                        }
                        "-d" => { cmd.push_str(" -d"); }
                        "-p" => { cmd.push_str(" -p"); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // set-buffer - Set buffer contents
            "set-buffer" | "setb" => {
                let mut buffer_name: Option<String> = None;
                let mut data: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => {
                            data = Some(s.to_string());
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let mut cmd = "set-buffer".to_string();
                if let Some(b) = buffer_name { cmd.push_str(&format!(" -b {}", b)); }
                if let Some(d) = data { cmd.push_str(&format!(" {}", d)); }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // list-buffers - List paste buffers
            "list-buffers" | "lsb" => {
                let resp = send_control_with_response("list-buffers\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // show-buffer - Show buffer contents
            "show-buffer" | "showb" => {
                let mut buffer_name: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let mut cmd = "show-buffer".to_string();
                if let Some(b) = buffer_name { cmd.push_str(&format!(" -b {}", b)); }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // delete-buffer - Delete a paste buffer
            "delete-buffer" | "deleteb" => {
                let mut buffer_name: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let mut cmd = "delete-buffer".to_string();
                if let Some(b) = buffer_name { cmd.push_str(&format!(" -b {}", b)); }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // display-message - Display a message
            "display-message" | "display" => {
                let mut message: Vec<String> = Vec::new();
                let mut target: Option<String> = None;
                let mut print_to_stdout = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                target = Some(t.to_string());
                                i += 1;
                            }
                        }
                        "-p" => { print_to_stdout = true; }
                        s => { message.push(s.to_string()); }
                    }
                    i += 1;
                }
                let msg = message.join(" ");
                let mut cmd = "display-message".to_string();
                if let Some(t) = target { cmd.push_str(&format!(" -t {}", t)); }
                if print_to_stdout { cmd.push_str(" -p"); }
                cmd.push_str(&format!(" {}", msg));
                cmd.push('\n');
                if print_to_stdout {
                    let resp = send_control_with_response(cmd)?;
                    print!("{}", resp);
                } else {
                    send_control(cmd)?;
                }
                return Ok(());
            }
            // run-shell - Run a shell command
            "run-shell" | "run" => {
                let mut cmd_to_run: Vec<String> = Vec::new();
                let mut background = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => { background = true; }
                        s => { cmd_to_run.push(s.to_string()); }
                    }
                    i += 1;
                }
                let shell_cmd = cmd_to_run.join(" ");
                // Run the command using the system shell
                if background {
                    #[cfg(windows)]
                    {
                        let _ = std::process::Command::new("cmd")
                            .args(["/C", &shell_cmd])
                            .spawn();
                    }
                } else {
                    #[cfg(windows)]
                    {
                        let output = std::process::Command::new("cmd")
                            .args(["/C", &shell_cmd])
                            .output()?;
                        io::stdout().write_all(&output.stdout)?;
                        io::stderr().write_all(&output.stderr)?;
                        std::process::exit(output.status.code().unwrap_or(0));
                    }
                }
                return Ok(());
            }
            // respawn-pane - Restart the pane's process
            "respawn-pane" | "respawnp" | "resp" => {
                let mut cmd = "respawn-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-k" => { cmd.push_str(" -k"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // last-window - Select last used window
            "last-window" | "last" => {
                send_control("last-window\n".to_string())?;
                return Ok(());
            }
            // last-pane - Select last used pane
            "last-pane" | "lastp" => {
                send_control("last-pane\n".to_string())?;
                return Ok(());
            }
            // next-window - Move to next window
            "next-window" | "next" => {
                send_control("next-window\n".to_string())?;
                return Ok(());
            }
            // previous-window - Move to previous window
            "previous-window" | "prev" => {
                send_control("previous-window\n".to_string())?;
                return Ok(());
            }
            // rotate-window - Rotate panes in window
            "rotate-window" | "rotatew" => {
                let mut cmd = "rotate-window".to_string();
                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // display-panes - Show pane numbers
            "display-panes" | "displayp" => {
                send_control("display-panes\n".to_string())?;
                return Ok(());
            }
            // break-pane - Break pane out to a new window
            "break-pane" | "breakp" => {
                let mut cmd = "break-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-d" => { cmd.push_str(" -d"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // join-pane - Join a pane to another window
            "join-pane" | "joinp" => {
                let mut cmd = "join-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-h" => { cmd.push_str(" -h"); }
                        "-v" => { cmd.push_str(" -v"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // rename-window - Rename current window
            "rename-window" | "renamew" => {
                // cmd_args[0] is the command, cmd_args[1] should be the new name
                if let Some(name) = cmd_args.get(1) {
                    if !name.starts_with('-') {
                        send_control(format!("rename-window {}\n", name))?;
                    }
                }
                return Ok(());
            }
            // zoom-pane - Toggle pane zoom
            "zoom-pane" | "resizep -Z" => {
                send_control("zoom-pane\n".to_string())?;
                return Ok(());
            }
            // source-file - Load a configuration file
            "source-file" | "source" => {
                let mut quiet = false;
                let mut file_path: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-q" => { quiet = true; }
                        "-n" => { /* parse only, don't execute */ }
                        "-v" => { /* verbose */ }
                        s if !s.starts_with('-') => { file_path = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(path) = file_path {
                    // Expand ~ to home directory
                    let expanded = if path.starts_with('~') {
                        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                        path.replacen('~', &home, 1)
                    } else {
                        path
                    };
                    if let Err(e) = std::fs::read_to_string(&expanded) {
                        if !quiet {
                            eprintln!("psmux: {}: {}", expanded, e);
                            std::process::exit(1);
                        }
                    } else {
                        // Send source-file command to server if attached
                        send_control(format!("source-file {}\n", expanded))?;
                    }
                }
                return Ok(());
            }
            // list-keys - List all key bindings
            "list-keys" | "lsk" => {
                let resp = send_control_with_response("list-keys\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // bind-key - Bind a key to a command
            "bind-key" | "bind" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                match send_control(format!("{}\n", cmd_str)) {
                    Ok(()) => {},
                    Err(e) if e.to_string().contains("no session") => {
                        eprintln!("warning: no active session; bind-key will take effect when set inside a session or via config file");
                    },
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // unbind-key - Unbind a key
            "unbind-key" | "unbind" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                match send_control(format!("{}\n", cmd_str)) {
                    Ok(()) => {},
                    Err(e) if e.to_string().contains("no session") => {
                        eprintln!("warning: no active session; unbind-key will take effect when set inside a session or via config file");
                    },
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // set-option / set - Set an option
            "set-option" | "set" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                match send_control(format!("{}\n", cmd_str)) {
                    Ok(()) => {},
                    Err(e) if e.to_string().contains("no session") => {
                        eprintln!("warning: no active session; option will take effect when set inside a session or via config file");
                    },
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // show-options / show / show-window-options / showw - Show options
            "show-options" | "show" | "show-window-options" | "showw" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                let resp = send_control_with_response(format!("{}\n", cmd_str))?;
                print!("{}", resp);
                return Ok(());
            }
            // if-shell - Conditional execution
            "if-shell" | "if" => {
                let mut background = false;
                let mut condition: Option<String> = None;
                let mut cmd_true: Option<String> = None;
                let mut cmd_false: Option<String> = None;
                let mut format_mode = false;
                let mut i = 1;
                
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => { background = true; }
                        "-F" => { format_mode = true; }
                        "-t" => { i += 1; } // Skip target
                        s if !s.starts_with('-') => {
                            if condition.is_none() {
                                condition = Some(s.to_string());
                            } else if cmd_true.is_none() {
                                cmd_true = Some(s.to_string());
                            } else if cmd_false.is_none() {
                                cmd_false = Some(s.to_string());
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                
                if let (Some(cond), Some(true_cmd)) = (condition, cmd_true) {
                    let success = if format_mode {
                        // Treat condition as format string - non-empty and non-zero is true
                        !cond.is_empty() && cond != "0"
                    } else {
                        // Run shell command - suppress stdout/stderr so it doesn't leak to terminal
                        #[cfg(windows)]
                        {
                            std::process::Command::new("cmd")
                                .args(["/C", &cond])
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status()
                                .map(|s| s.success())
                                .unwrap_or(false)
                        }
                        #[cfg(not(windows))]
                        {
                            std::process::Command::new("sh")
                                .args(["-c", &cond])
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status()
                                .map(|s| s.success())
                                .unwrap_or(false)
                        }
                    };
                    
                    let cmd_to_run = if success { Some(true_cmd) } else { cmd_false };
                    
                    if let Some(cmd) = cmd_to_run {
                        // Re-quote multi-word arguments for TCP transport
                        let needs_quoting = cmd.contains(' ');
                        let tcp_cmd = if needs_quoting {
                            // The command string may contain spaces (e.g. "display-message -p hello")
                            // Send it as-is since it's already a full command line
                            format!("{}\n", cmd)
                        } else {
                            format!("{}\n", cmd)
                        };
                        // Use send_control_with_response to capture any output from the chosen command
                        let resp = send_control_with_response(tcp_cmd)?;
                        if !resp.is_empty() {
                            print!("{}", resp);
                        }
                    }
                }
                return Ok(());
            }
            // wait-for - Wait for a signal
            "wait-for" | "wait" => {
                let mut lock = false;
                let mut signal = false;
                let mut unlock = false;
                let mut channel: Option<String> = None;
                let mut i = 1;
                
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-L" => { lock = true; }
                        "-S" => { signal = true; }
                        "-U" => { unlock = true; }
                        s if !s.starts_with('-') => { channel = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                
                if let Some(ch) = channel {
                    if signal {
                        send_control(format!("wait-for -S {}\n", ch))?;
                    } else if lock {
                        send_control(format!("wait-for -L {}\n", ch))?;
                    } else if unlock {
                        send_control(format!("wait-for -U {}\n", ch))?;
                    } else {
                        // Wait for channel - this blocks
                        let resp = send_control_with_response(format!("wait-for {}\n", ch))?;
                        if !resp.is_empty() {
                            print!("{}", resp);
                        }
                    }
                }
                return Ok(());
            }
            // select-layout - Select a layout for the window
            "select-layout" | "selectl" => {
                let mut layout: Option<String> = None;
                let mut next = false;
                let mut prev = false;
                let mut i = 1;
                
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-n" => { next = true; }
                        "-p" => { prev = true; }
                        "-o" => { /* last layout */ }
                        "-E" => { /* spread evenly */ }
                        "-t" => { i += 1; } // Skip target
                        s if !s.starts_with('-') => { layout = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                
                if next {
                    send_control("next-layout\n".to_string())?;
                } else if prev {
                    send_control("previous-layout\n".to_string())?;
                } else if let Some(l) = layout {
                    send_control(format!("select-layout {}\n", l))?;
                } else {
                    send_control("select-layout\n".to_string())?;
                }
                return Ok(());
            }
            // move-window - Move a window
            "move-window" | "movew" => {
                let mut cmd = "move-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { cmd.push_str(" -a"); }
                        "-b" => { cmd.push_str(" -b"); }
                        "-r" => { cmd.push_str(" -r"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-k" => { cmd.push_str(" -k"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // swap-window - Swap windows
            "swap-window" | "swapw" => {
                let mut cmd = "swap-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-d" => { cmd.push_str(" -d"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // list-clients - List all clients
            "list-clients" | "lsc" => {
                let resp = send_control_with_response("list-clients\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // switch-client - Switch the current client to another session
            "switch-client" | "switchc" => {
                let mut cmd = "switch-client".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-l" => { cmd.push_str(" -l"); }
                        "-n" => { cmd.push_str(" -n"); }
                        "-p" => { cmd.push_str(" -p"); }
                        "-r" => { cmd.push_str(" -r"); }
                        "-c" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -c {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // copy-mode - Enter copy mode
            "copy-mode" => {
                let mut cmd = "copy-mode".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-u" => { cmd.push_str(" -u"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-e" => { cmd.push_str(" -e"); }
                        "-H" => { cmd.push_str(" -H"); }
                        "-q" => { cmd.push_str(" -q"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // clock-mode - Display a clock
            "clock-mode" => {
                send_control("clock-mode\n".to_string())?;
                return Ok(());
            }
            // choose-buffer - List paste buffers interactively
            "choose-buffer" | "chooseb" => {
                let resp = send_control_with_response("choose-buffer\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // set-environment / setenv - Set environment variable
            "set-environment" | "setenv" => {
                let mut cmd = "set-environment".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-g" => { cmd.push_str(" -g"); }
                        "-r" => { cmd.push_str(" -r"); }
                        "-u" => { cmd.push_str(" -u"); }
                        "-h" => { cmd.push_str(" -h"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s => { cmd.push_str(&format!(" {}", s)); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // show-environment / showenv - Show environment variables
            "show-environment" | "showenv" => {
                let mut cmd = "show-environment".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-g" => { cmd.push_str(" -g"); }
                        "-s" => { cmd.push_str(" -s"); }
                        "-h" => { cmd.push_str(" -h"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => { cmd.push_str(&format!(" {}", s)); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // load-buffer - Load a paste buffer from a file
            "load-buffer" | "loadb" => {
                let mut buffer_name: Option<String> = None;
                let mut file_path: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => { file_path = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(path) = file_path {
                    let content = if path == "-" {
                        let mut input = String::new();
                        io::stdin().read_to_string(&mut input)?;
                        input
                    } else {
                        std::fs::read_to_string(&path)?
                    };
                    let mut cmd = "set-buffer".to_string();
                    if let Some(b) = buffer_name {
                        cmd.push_str(&format!(" -b {}", b));
                    }
                    // Escape the content for transmission
                    let escaped = content.replace('\n', "\\n").replace('\r', "\\r");
                    cmd.push_str(&format!(" {}", escaped));
                    cmd.push('\n');
                    send_control(cmd)?;
                }
                return Ok(());
            }
            // save-buffer - Save a paste buffer to a file
            "save-buffer" | "saveb" => {
                let mut buffer_name: Option<String> = None;
                let mut file_path: Option<String> = None;
                let mut append = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { append = true; }
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => { file_path = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(path) = file_path {
                    let mut cmd = "show-buffer".to_string();
                    if let Some(b) = buffer_name {
                        cmd.push_str(&format!(" -b {}", b));
                    }
                    cmd.push('\n');
                    let content = send_control_with_response(cmd)?;
                    if path == "-" {
                        print!("{}", content);
                    } else if append {
                        use std::fs::OpenOptions;
                        let mut file = OpenOptions::new().append(true).create(true).open(&path)?;
                        file.write_all(content.as_bytes())?;
                    } else {
                        std::fs::write(&path, &content)?;
                    }
                }
                return Ok(());
            }
            // clear-history - Clear pane history
            "clear-history" | "clearhist" => {
                let mut cmd = "clear-history".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-H" => { cmd.push_str(" -H"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // pipe-pane - Pipe pane output to a command
            "pipe-pane" | "pipep" => {
                let mut cmd = "pipe-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-I" => { cmd.push_str(" -I"); }
                        "-O" => { cmd.push_str(" -O"); }
                        "-o" => { cmd.push_str(" -o"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s => { cmd.push_str(&format!(" {}", s)); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // find-window - Search for a window
            "find-window" | "findw" => {
                let mut pattern: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-C" | "-N" | "-T" | "-i" | "-r" | "-Z" => {}
                        "-t" => { i += 1; }
                        s if !s.starts_with('-') => { pattern = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(p) = pattern {
                    let resp = send_control_with_response(format!("find-window {}\n", p))?;
                    print!("{}", resp);
                }
                return Ok(());
            }
            // list-commands - List all commands (duplicate handled above but kept for match completeness)
            "list-commands" | "lscm" => {
                print_commands();
                return Ok(());
            }
            // set-hook - Set a hook
            "set-hook" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                send_control(format!("{}\n", cmd_str))?;
                return Ok(());
            }
            // show-hooks - Show hooks
            "show-hooks" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                let resp = send_control_with_response(format!("{}\n", cmd_str))?;
                print!("{}", resp);
                return Ok(());
            }
            // next-layout - Cycle to next layout
            "next-layout" => {
                send_control("next-layout\n".to_string())?;
                return Ok(());
            }
            // previous-layout - Cycle to previous layout
            "previous-layout" => {
                send_control("previous-layout\n".to_string())?;
                return Ok(());
            }
            _ => {
                // Unknown command - print error and exit
                if !cmd.is_empty() {
                    eprintln!("psmux: unknown command: {}", cmd);
                    eprintln!("Run 'psmux --help' for usage information.");
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unknown command: {}", cmd)));
                }
            }
        }
    
    // Default behavior: If no PSMUX_REMOTE_ATTACH is set and no specific command matched,
    // we need to either attach to an existing session or create a new one.
    // This ensures sessions persist after detach.
    if env::var("PSMUX_REMOTE_ATTACH").ok().as_deref() != Some("1") {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
        let session_name = env::var("PSMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string());
        let port_path = format!("{}\\.psmux\\{}.port", home, session_name);
        
        // Check if port file exists AND server is actually alive
        let server_alive = if std::path::Path::new(&port_path).exists() {
            if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                if let Ok(port) = port_str.trim().parse::<u16>() {
                    let addr = format!("127.0.0.1:{}", port);
                    std::net::TcpStream::connect_timeout(
                        &addr.parse().unwrap(),
                        Duration::from_millis(50)
                    ).is_ok()
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        
        if !server_alive {
            // Clean up stale port file if it exists
            let _ = std::fs::remove_file(&port_path);
            // No existing session - create one in background
            let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
            let server_args: Vec<String> = vec!["server".into(), "-s".into(), session_name.clone()];
            #[cfg(windows)]
            crate::platform::spawn_server_hidden(&exe, &server_args)?;
            #[cfg(not(windows))]
            {
                let mut cmd = std::process::Command::new(&exe);
                for a in &server_args { cmd.arg(a); }
                cmd.stdin(std::process::Stdio::null());
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                let _child = cmd.spawn().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to spawn server: {e}")))?;
            }
            
            // Wait for server to start
            for _ in 0..20 {
                if std::path::Path::new(&port_path).exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        
        // Now attach to the session
        env::set_var("PSMUX_SESSION_NAME", &session_name);
        env::set_var("PSMUX_REMOTE_ATTACH", "1");
    }
    
    if env::var("PSMUX_ACTIVE").ok().as_deref() == Some("1") {
        eprintln!("psmux: nested sessions are not allowed");
        return Ok(());
    }
    env::set_var("PSMUX_ACTIVE", "1");
    let mut stdout = io::stdout();
    enable_virtual_terminal_processing();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableBlinking, EnableMouseCapture, EnableBracketedPaste)?;
    apply_cursor_style(&mut stdout)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    
    // Loop to handle session switching without spawning new processes
    loop {
        let result = run_remote(&mut terminal);
        
        // Check if we should switch to another session
        if let Ok(switch_to) = env::var("PSMUX_SWITCH_TO") {
            env::remove_var("PSMUX_SWITCH_TO");
            env::set_var("PSMUX_SESSION_NAME", &switch_to);
            // Update last_session file
            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
            let last_path = format!("{}\\.psmux\\last_session", home);
            let _ = std::fs::write(&last_path, &switch_to);
            // Continue loop to attach to new session
            continue;
        }
        
        // Normal exit
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), DisableBlinking, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        return result;
    }
}
