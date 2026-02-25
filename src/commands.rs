use std::io;
use std::time::Instant;

use crate::types::{AppState, Mode, Action, FocusDir, LayoutKind, MenuItem, Menu, PopupPty};
use crate::tree::{compute_rects, kill_all_children};
use crate::pane::{create_window, split_active, kill_active_pane};
use crate::copy_mode::{enter_copy_mode, switch_with_copy_save, paste_latest,
    capture_active_pane, save_latest_buffer};
use crate::session::{send_control_to_port, list_all_sessions_tree};
use crate::window_ops::toggle_zoom;

/// Build the choose-tree data for the WindowChooser mode.
pub fn build_choose_tree(app: &AppState) -> Vec<crate::session::TreeEntry> {
    let current_windows: Vec<(String, usize, String, bool)> = app.windows.iter().enumerate().map(|(i, w)| {
        let panes = crate::tree::count_panes(&w.root);
        let size = format!("{}x{}", app.last_window_area.width, app.last_window_area.height);
        (w.name.clone(), panes, size, i == app.active_idx)
    }).collect();
    list_all_sessions_tree(&app.session_name, &current_windows)
}

/// Extract a window index from a tmux-style target string.
/// Handles formats like "0", ":0", ":=0", "=0", stripping leading ':'/'=' chars.
fn parse_window_target(target: &str) -> Option<usize> {
    let s = target.trim_start_matches(':').trim_start_matches('=');
    s.parse::<usize>().ok()
}

/// Parse a command string to an Action
pub fn parse_command_to_action(cmd: &str) -> Option<Action> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() { return None; }
    
    match parts[0] {
        "display-panes" | "displayp" => Some(Action::DisplayPanes),
        "new-window" | "neww" => Some(Action::NewWindow),
        "split-window" | "splitw" => {
            if parts.iter().any(|p| *p == "-h") {
                Some(Action::SplitHorizontal)
            } else {
                Some(Action::SplitVertical)
            }
        }
        "kill-pane" | "killp" => Some(Action::KillPane),
        "next-window" | "next" => Some(Action::NextWindow),
        "previous-window" | "prev" => Some(Action::PrevWindow),
        "copy-mode" => Some(Action::CopyMode),
        "paste-buffer" | "pasteb" => Some(Action::Paste),
        "detach-client" | "detach" => Some(Action::Detach),
        "rename-window" | "renamew" => Some(Action::RenameWindow),
        "choose-window" | "choose-tree" | "choose-session" => Some(Action::WindowChooser),
        "resize-pane" | "resizep" if parts.iter().any(|p| *p == "-Z") => Some(Action::ZoomPane),
        "zoom-pane" => Some(Action::ZoomPane),
        "select-pane" | "selectp" => {
            if parts.iter().any(|p| *p == "-U") {
                Some(Action::MoveFocus(FocusDir::Up))
            } else if parts.iter().any(|p| *p == "-D") {
                Some(Action::MoveFocus(FocusDir::Down))
            } else if parts.iter().any(|p| *p == "-L") {
                Some(Action::MoveFocus(FocusDir::Left))
            } else if parts.iter().any(|p| *p == "-R") {
                Some(Action::MoveFocus(FocusDir::Right))
            } else {
                Some(Action::Command(cmd.to_string()))
            }
        }
        "last-window" | "last" => Some(Action::Command("last-window".to_string())),
        "last-pane" | "lastp" => Some(Action::Command("last-pane".to_string())),
        "swap-pane" | "swapp" => Some(Action::Command(cmd.to_string())),
        "resize-pane" | "resizep" => Some(Action::Command(cmd.to_string())),
        "rotate-window" | "rotatew" => Some(Action::Command(cmd.to_string())),
        "break-pane" | "breakp" => Some(Action::Command(cmd.to_string())),
        "respawn-pane" | "respawnp" => Some(Action::Command(cmd.to_string())),
        "kill-window" | "killw" => Some(Action::Command(cmd.to_string())),
        "kill-session" => Some(Action::Command(cmd.to_string())),
        "select-window" | "selectw" => Some(Action::Command(cmd.to_string())),
        "toggle-sync" => Some(Action::Command("toggle-sync".to_string())),
        "send-keys" => Some(Action::Command(cmd.to_string())),
        "set-option" | "set" | "setw" | "set-window-option" => Some(Action::Command(cmd.to_string())),
        "source-file" | "source" => Some(Action::Command(cmd.to_string())),
        "select-layout" | "selectl" => Some(Action::Command(cmd.to_string())),
        "next-layout" => Some(Action::Command("next-layout".to_string())),
        "confirm-before" | "confirm" => Some(Action::Command(cmd.to_string())),
        "display-menu" | "menu" => Some(Action::Command(cmd.to_string())),
        "display-popup" | "popup" => Some(Action::Command(cmd.to_string())),
        "pipe-pane" | "pipep" => Some(Action::Command(cmd.to_string())),
        "rename-session" | "rename" => Some(Action::Command(cmd.to_string())),
        "clear-history" => Some(Action::Command("clear-history".to_string())),
        "set-buffer" | "setb" => Some(Action::Command(cmd.to_string())),
        "delete-buffer" | "deleteb" => Some(Action::Command("delete-buffer".to_string())),
        "display-message" | "display" => Some(Action::Command(cmd.to_string())),
        "switch-client" | "switchc" => {
            // Check for -T flag to switch key table
            if let Some(pos) = parts.iter().position(|p| *p == "-T") {
                if let Some(table) = parts.get(pos + 1) {
                    Some(Action::SwitchTable(table.to_string()))
                } else {
                    Some(Action::Command(cmd.to_string()))
                }
            } else {
                Some(Action::Command(cmd.to_string()))
            }
        }
        _ => Some(Action::Command(cmd.to_string()))
    }
}

/// Format an Action back to a command string
pub fn format_action(action: &Action) -> String {
    match action {
        Action::DisplayPanes => "display-panes".to_string(),
        Action::NewWindow => "new-window".to_string(),
        Action::SplitHorizontal => "split-window -h".to_string(),
        Action::SplitVertical => "split-window -v".to_string(),
        Action::KillPane => "kill-pane".to_string(),
        Action::NextWindow => "next-window".to_string(),
        Action::PrevWindow => "previous-window".to_string(),
        Action::CopyMode => "copy-mode".to_string(),
        Action::Paste => "paste-buffer".to_string(),
        Action::Detach => "detach-client".to_string(),
        Action::RenameWindow => "rename-window".to_string(),
        Action::WindowChooser => "choose-window".to_string(),
        Action::ZoomPane => "resize-pane -Z".to_string(),
        Action::MoveFocus(dir) => {
            let flag = match dir {
                FocusDir::Up => "-U",
                FocusDir::Down => "-D",
                FocusDir::Left => "-L",
                FocusDir::Right => "-R",
            };
            format!("select-pane {}", flag)
        }
        Action::Command(cmd) => cmd.clone(),
        Action::CommandChain(cmds) => cmds.join(" \\; "),
        Action::SwitchTable(table) => format!("switch-client -T {}", table),
    }
}

/// Parse a command line string, respecting quoted arguments
pub fn parse_command_line(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escape_next = false;
    
    for c in line.chars() {
        if escape_next {
            current.push(c);
            escape_next = false;
        } else if c == '\\' && in_quotes {
            escape_next = true;
        } else if c == '"' {
            in_quotes = !in_quotes;
        } else if c.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
        } else {
            current.push(c);
        }
    }
    
    if !current.is_empty() {
        args.push(current);
    }
    
    args
}

/// Parse a menu definition string into a Menu structure
pub fn parse_menu_definition(def: &str, x: Option<i16>, y: Option<i16>) -> Menu {
    let mut menu = Menu {
        title: String::new(),
        items: Vec::new(),
        selected: 0,
        x,
        y,
    };
    
    let parts: Vec<&str> = def.split_whitespace().collect();
    if parts.is_empty() {
        return menu;
    }
    
    let mut i = 0;
    while i < parts.len() {
        if parts[i] == "-T" {
            if let Some(title) = parts.get(i + 1) {
                menu.title = title.trim_matches('"').to_string();
                i += 2;
                continue;
            }
        }
        
        if let Some(name) = parts.get(i) {
            let name = name.trim_matches('"').to_string();
            if name.is_empty() || name == "-" {
                menu.items.push(MenuItem {
                    name: String::new(),
                    key: None,
                    command: String::new(),
                    is_separator: true,
                });
                i += 1;
            } else {
                let key = parts.get(i + 1).map(|k| k.trim_matches('"').chars().next()).flatten();
                let command = parts.get(i + 2).map(|c| c.trim_matches('"').to_string()).unwrap_or_default();
                menu.items.push(MenuItem {
                    name,
                    key,
                    command,
                    is_separator: false,
                });
                i += 3;
            }
        } else {
            break;
        }
    }
    
    if menu.items.is_empty() && !def.is_empty() {
        menu.title = "Menu".to_string();
        menu.items.push(MenuItem {
            name: def.to_string(),
            key: Some('1'),
            command: def.to_string(),
            is_separator: false,
        });
    }
    
    menu
}

/// Fire hooks for a given event
pub fn fire_hooks(app: &mut AppState, event: &str) {
    if let Some(commands) = app.hooks.get(event).cloned() {
        for cmd in commands {
            let _ = execute_command_string(app, &cmd);
        }
    }
}

/// Execute an Action (from key bindings)
pub fn execute_action(app: &mut AppState, action: &Action) -> io::Result<bool> {
    match action {
        Action::DisplayPanes => {
            let win = &app.windows[app.active_idx];
            let mut rects: Vec<(Vec<usize>, ratatui::prelude::Rect)> = Vec::new();
            compute_rects(&win.root, app.last_window_area, &mut rects);
            app.display_map.clear();
            for (i, (path, _)) in rects.into_iter().enumerate() {
                let n = i + 1;
                if n <= 10 { app.display_map.push((n, path)); } else { break; }
            }
            app.mode = Mode::PaneChooser { opened_at: Instant::now() };
        }
        Action::MoveFocus(dir) => {
            let d = *dir;
            switch_with_copy_save(app, |app| { crate::input::move_focus(app, d); });
        }
        Action::NewWindow => {
            let pty_system = portable_pty::native_pty_system();
            create_window(&*pty_system, app, None)?;
        }
        Action::SplitHorizontal => {
            split_active(app, LayoutKind::Horizontal)?;
        }
        Action::SplitVertical => {
            split_active(app, LayoutKind::Vertical)?;
        }
        Action::KillPane => {
            kill_active_pane(app)?;
        }
        Action::NextWindow => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + 1) % app.windows.len();
                });
            }
        }
        Action::PrevWindow => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                });
            }
        }
        Action::CopyMode => {
            enter_copy_mode(app);
        }
        Action::Paste => {
            paste_latest(app)?;
        }
        Action::Detach => {
            return Ok(true);
        }
        Action::RenameWindow => {
            app.mode = Mode::RenamePrompt { input: String::new() };
        }
        Action::WindowChooser => {
            let tree = build_choose_tree(app);
            let selected = tree.iter().position(|e| e.is_current_session && e.is_active_window && !e.is_session_header).unwrap_or(0);
            app.mode = Mode::WindowChooser { selected, tree };
        }
        Action::ZoomPane => {
            toggle_zoom(app);
        }
        Action::Command(cmd) => {
            execute_command_string(app, cmd)?;
        }
        Action::CommandChain(cmds) => {
            for cmd in cmds {
                execute_command_string(app, cmd)?;
            }
        }
        Action::SwitchTable(table) => {
            app.current_key_table = Some(table.clone());
        }
    }
    Ok(false)
}

pub fn execute_command_prompt(app: &mut AppState) -> io::Result<()> {
    let cmdline = match &app.mode { Mode::CommandPrompt { input, .. } => input.clone(), _ => String::new() };
    app.mode = Mode::Passthrough;
    let parts: Vec<&str> = cmdline.split_whitespace().collect();
    if parts.is_empty() { return Ok(()); }
    match parts[0] {
        "new-window" => {
            let pty_system = portable_pty::native_pty_system();
            create_window(&*pty_system, app, None)?;
        }
        "split-window" => {
            let kind = if parts.iter().any(|p| *p == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
            split_active(app, kind)?;
        }
        "kill-pane" => { kill_active_pane(app)?; }
        "capture-pane" => { capture_active_pane(app)?; }
        "save-buffer" => { if let Some(file) = parts.get(1) { save_latest_buffer(app, file)?; } }
        "list-sessions" => { println!("default"); }
        "attach-session" => { }
        "next-window" => {
            app.last_window_idx = app.active_idx;
            app.active_idx = (app.active_idx + 1) % app.windows.len();
        }
        "previous-window" => {
            app.last_window_idx = app.active_idx;
            app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
        }
        "select-window" => {
            if let Some(tidx) = parts.iter().position(|p| *p == "-t").and_then(|i| parts.get(i+1)) {
                if let Some(n) = parse_window_target(tidx) {
                    if n >= app.window_base_index {
                        let internal_idx = n - app.window_base_index;
                        if internal_idx < app.windows.len() {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = internal_idx;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Execute a command string (used by menus, hooks, confirm dialogs, etc.)
pub fn execute_command_string(app: &mut AppState, cmd: &str) -> io::Result<()> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() { return Ok(()); }
    
    match parts[0] {
        "new-window" | "neww" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "new-window\n");
            }
        }
        "split-window" | "splitw" => {
            let flag = if parts.iter().any(|p| *p == "-h") { "-h" } else { "-v" };
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("split-window {}\n", flag));
            }
        }
        "kill-pane" => {
            let _ = kill_active_pane(app);
        }
        "kill-window" | "killw" => {
            if app.windows.len() > 1 {
                let mut win = app.windows.remove(app.active_idx);
                kill_all_children(&mut win.root);
                if app.active_idx >= app.windows.len() {
                    app.active_idx = app.windows.len() - 1;
                }
            }
        }
        "next-window" | "next" => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + 1) % app.windows.len();
                });
            }
        }
        "previous-window" | "prev" => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                });
            }
        }
        "last-window" | "last" => {
            if app.last_window_idx < app.windows.len() {
                switch_with_copy_save(app, |app| {
                    let tmp = app.active_idx;
                    app.active_idx = app.last_window_idx;
                    app.last_window_idx = tmp;
                });
            }
        }
        "select-window" | "selectw" => {
            if let Some(t_pos) = parts.iter().position(|p| *p == "-t") {
                if let Some(t) = parts.get(t_pos + 1) {
                    if let Some(idx) = parse_window_target(t) {
                        if idx >= app.window_base_index {
                            let internal_idx = idx - app.window_base_index;
                            if internal_idx < app.windows.len() {
                                switch_with_copy_save(app, |app| {
                                    app.last_window_idx = app.active_idx;
                                    app.active_idx = internal_idx;
                                });
                            }
                        }
                    }
                }
            }
        }
        "select-pane" | "selectp" => {
            // Save/restore copy mode across pane switches (tmux parity #43)
            let is_last = parts.iter().any(|p| *p == "-l");
            if is_last {
                switch_with_copy_save(app, |app| {
                    let win = &mut app.windows[app.active_idx];
                    if !app.last_pane_path.is_empty() {
                        let tmp = win.active_path.clone();
                        win.active_path = app.last_pane_path.clone();
                        app.last_pane_path = tmp;
                    }
                });
                return Ok(());
            }
            let dir = if parts.iter().any(|p| *p == "-U") { FocusDir::Up }
                else if parts.iter().any(|p| *p == "-D") { FocusDir::Down }
                else if parts.iter().any(|p| *p == "-L") { FocusDir::Left }
                else if parts.iter().any(|p| *p == "-R") { FocusDir::Right }
                else { return Ok(()); };
            switch_with_copy_save(app, |app| {
                let win = &app.windows[app.active_idx];
                app.last_pane_path = win.active_path.clone();
                crate::input::move_focus(app, dir);
            });
        }
        "last-pane" | "lastp" => {
            switch_with_copy_save(app, |app| {
                let win = &mut app.windows[app.active_idx];
                if !app.last_pane_path.is_empty() {
                    let tmp = win.active_path.clone();
                    win.active_path = app.last_pane_path.clone();
                    app.last_pane_path = tmp;
                }
            });
        }
        "rename-window" | "renamew" => {
            if let Some(name) = parts.get(1) {
                let win = &mut app.windows[app.active_idx];
                win.name = name.to_string();
            }
        }
        "zoom-pane" | "zoom" | "resizep -Z" => {
            toggle_zoom(app);
        }
        "copy-mode" => {
            enter_copy_mode(app);
        }
        "display-panes" | "displayp" => {
            app.mode = Mode::PaneChooser { opened_at: Instant::now() };
        }
        "confirm-before" | "confirm" => {
            let rest = parts[1..].join(" ");
            app.mode = Mode::ConfirmMode {
                prompt: format!("Run '{}'?", rest),
                command: rest,
                input: String::new(),
            };
        }
        "display-menu" | "menu" => {
            let rest = parts[1..].join(" ");
            let menu = parse_menu_definition(&rest, None, None);
            if !menu.items.is_empty() {
                app.mode = Mode::MenuMode { menu };
            }
        }
        "display-popup" | "popup" => {
            // Parse -w width, -h height, -E close-on-exit flags
            let mut width: u16 = 80;
            let mut height: u16 = 24;
            let close_on_exit = parts.iter().any(|p| *p == "-E");
            if let Some(pos) = parts.iter().position(|p| *p == "-w") {
                if let Some(v) = parts.get(pos + 1) { width = v.parse().unwrap_or(80); }
            }
            if let Some(pos) = parts.iter().position(|p| *p == "-h") {
                if let Some(v) = parts.get(pos + 1) { height = v.parse().unwrap_or(24); }
            }
            // Collect command (non-flag args)
            let cmd_parts: Vec<&str> = parts[1..].iter()
                .filter(|a| !a.starts_with('-'))
                .copied()
                .collect();
            // Skip width/height values
            let rest = cmd_parts.join(" ");
            
            // Try PTY-based popup for interactive commands
            let pty_result = if !rest.is_empty() {
                Some(portable_pty::native_pty_system())
                    .and_then(|pty_sys| {
                        let pty_size = portable_pty::PtySize { rows: height.saturating_sub(2), cols: width.saturating_sub(2), pixel_width: 0, pixel_height: 0 };
                        let pair = pty_sys.openpty(pty_size).ok()?;
                        let mut cmd_builder = portable_pty::CommandBuilder::new(if cfg!(windows) { "pwsh" } else { "sh" });
                        if cfg!(windows) { cmd_builder.args(["-NoProfile", "-Command", &rest]); } else { cmd_builder.args(["-c", &rest]); }
                        let child = pair.slave.spawn_command(cmd_builder).ok()?;
                        // Close the slave handle immediately – required for ConPTY.
                        drop(pair.slave);
                        let term = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(pty_size.rows, pty_size.cols, 0)));
                        let term_reader = term.clone();
                        if let Ok(mut reader) = pair.master.try_clone_reader() {
                            std::thread::spawn(move || {
                                let mut buf = [0u8; 8192];
                                loop {
                                    match std::io::Read::read(&mut reader, &mut buf) {
                                        Ok(n) if n > 0 => { if let Ok(mut p) = term_reader.lock() { p.process(&buf[..n]); } }
                                        _ => break,
                                    }
                                }
                            });
                        }
                        let pty_writer = pair.master.take_writer().ok()?;
                        Some(PopupPty { master: pair.master, writer: pty_writer, child, term })
                    })
            } else { None };
            
            app.mode = Mode::PopupMode {
                command: rest,
                output: String::new(),
                process: None,
                width,
                height,
                close_on_exit,
                popup_pty: pty_result,
            };
        }
        "resize-pane" | "resizep" => {
            if parts.iter().any(|p| *p == "-Z") {
                toggle_zoom(app);
            } else {
                // Forward to server for actual resize
                if let Some(port) = app.control_port {
                    let _ = send_control_to_port(port, &format!("{}\n", cmd));
                }
            }
        }
        "swap-pane" | "swapp" => {
            if let Some(port) = app.control_port {
                let dir = if parts.iter().any(|p| *p == "-U") { "-U" } else { "-D" };
                let _ = send_control_to_port(port, &format!("swap-pane {}\n", dir));
            }
        }
        "rotate-window" | "rotatew" => {
            if let Some(port) = app.control_port {
                let flag = if parts.iter().any(|p| *p == "-D") { "-D" } else { "" };
                let _ = send_control_to_port(port, &format!("rotate-window {}\n", flag));
            }
        }
        "break-pane" | "breakp" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "break-pane\n");
            }
        }
        "respawn-pane" | "respawnp" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "respawn-pane\n");
            }
        }
        "toggle-sync" => {
            app.sync_input = !app.sync_input;
        }
        "set-option" | "set" | "set-window-option" | "setw" => {
            // Forward to server for option handling
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "bind-key" | "bind" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "unbind-key" | "unbind" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "source-file" | "source" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "send-keys" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "detach-client" | "detach" => {
            // handled by caller to set quit flag
        }
        "rename-session" => {
            if let Some(name) = parts.get(1) {
                app.session_name = name.to_string();
            }
        }
        "select-layout" | "selectl" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "next-layout" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "next-layout\n");
            }
        }
        "pipe-pane" | "pipep" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
        "choose-tree" | "choose-window" => {
            let tree = build_choose_tree(app);
            let selected = tree.iter().position(|e| e.is_current_session && e.is_active_window && !e.is_session_header).unwrap_or(0);
            app.mode = Mode::WindowChooser { selected, tree };
        }
        "command-prompt" => {
            // Support -I initial_text, -p prompt (ignored), -1 (ignored)
            let initial = parts.windows(2).find(|w| w[0] == "-I").map(|w| w[1].to_string()).unwrap_or_default();
            app.mode = Mode::CommandPrompt { input: initial.clone(), cursor: initial.len() };
        }
        "paste-buffer" | "pasteb" => {
            paste_latest(app)?;
        }
        "set-buffer" => {
            if let Some(text) = parts.get(1) {
                app.paste_buffers.insert(0, text.to_string());
                if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
            }
        }
        "delete-buffer" => {
            if !app.paste_buffers.is_empty() { app.paste_buffers.remove(0); }
        }
        "clear-history" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "clear-history\n");
            }
        }
        "kill-session" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "kill-session\n");
            }
        }
        _ => {
            // Forward unknown commands to server (catch-all for tmux compat)
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd));
            }
        }
    }
    Ok(())
}
