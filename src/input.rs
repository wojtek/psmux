use std::io::{self, Write};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use portable_pty::PtySystemSelection;
use ratatui::prelude::*;

use crate::types::*;
use crate::tree::*;
use crate::pane::*;
use crate::commands::*;
use crate::copy_mode::*;
use crate::layout::{cycle_top_layout, apply_layout};
use crate::window_ops::{toggle_zoom, swap_pane, break_pane_to_window};

/// Write a mouse event to the child PTY using the encoding the child requested.
fn write_mouse_event(master: &mut Box<dyn portable_pty::MasterPty>, button: u8, col: u16, row: u16, press: bool, enc: vt100::MouseProtocolEncoding) {
    use std::io::Write;
    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            let ch = if press { 'M' } else { 'm' };
            let _ = write!(master, "\x1b[<{};{};{}{}", button, col, row, ch);
            let _ = master.flush();
        }
        _ => {
            // Default / Utf8 X10-style encoding: \x1b[M Cb Cx Cy (all + 32)
            if press {
                let cb = (button + 32) as u8;
                let cx = ((col as u8).min(223)) + 32;
                let cy = ((row as u8).min(223)) + 32;
                let _ = master.write_all(&[0x1b, b'[', b'M', cb, cx, cy]);
                let _ = master.flush();
            }
            // X10-style has no release encoding for individual buttons
        }
    }
}

pub fn handle_key(app: &mut AppState, key: KeyEvent) -> io::Result<bool> {
    let is_ctrl_q = (matches!(key.code, KeyCode::Char('q')) && key.modifiers.contains(KeyModifiers::CONTROL))
        || matches!(key.code, KeyCode::Char('\x11'));
    if is_ctrl_q {
        return Ok(true);
    }

    match app.mode {
        Mode::Passthrough => {
            let is_ctrl_b = (key.code, key.modifiers) == app.prefix_key
                || matches!(key.code, KeyCode::Char(c) if c == '\u{0002}');
            if is_ctrl_b {
                app.mode = Mode::Prefix { armed_at: Instant::now() };
                return Ok(false);
            }
            // Check root key table for bindings (bind-key -n / bind-key -T root)
            let key_tuple = (key.code, key.modifiers);
            if let Some(bind) = app.key_tables.get("root").and_then(|t| t.iter().find(|b| b.key == key_tuple)).cloned() {
                return execute_action(app, &bind.action);
            }
            forward_key_to_active(app, key)?;
            Ok(false)
        }
        Mode::Prefix { armed_at } => {
            let elapsed = armed_at.elapsed().as_millis() as u64;
            
            let key_tuple = (key.code, key.modifiers);
            if let Some(bind) = app.key_tables.get("prefix").and_then(|t| t.iter().find(|b| b.key == key_tuple)).cloned() {
                app.mode = Mode::Passthrough;
                return execute_action(app, &bind.action);
            }
            
            let handled = match key.code {
                // Alt+Arrow: resize pane by 5 (must be before plain arrows)
                KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_vertical(app, -5); true
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_vertical(app, 5); true
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_horizontal(app, -5); true
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_horizontal(app, 5); true
                }
                // Ctrl+Arrow: resize pane by 1 (must be before plain arrows)
                KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_vertical(app, -1); true
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_vertical(app, 1); true
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_horizontal(app, -1); true
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_horizontal(app, 1); true
                }
                KeyCode::Left => { move_focus(app, FocusDir::Left); true }
                KeyCode::Right => { move_focus(app, FocusDir::Right); true }
                KeyCode::Up => { move_focus(app, FocusDir::Up); true }
                KeyCode::Down => { move_focus(app, FocusDir::Down); true }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let idx = d.to_digit(10).unwrap() as usize;
                    if idx >= app.window_base_index {
                        let internal_idx = idx - app.window_base_index;
                        if internal_idx < app.windows.len() {
                            app.active_idx = internal_idx;
                        }
                    }
                    true
                }
                KeyCode::Char('c') => {
                    let pty_system = PtySystemSelection::default()
                        .get()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
                    create_window(&*pty_system, app, None)?;
                    true
                }
                KeyCode::Char('n') => {
                    if !app.windows.is_empty() {
                        app.active_idx = (app.active_idx + 1) % app.windows.len();
                    }
                    true
                }
                KeyCode::Char('p') => {
                    if !app.windows.is_empty() {
                        app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                    }
                    true
                }
                KeyCode::Char('%') => {
                    split_active(app, LayoutKind::Horizontal)?;
                    true
                }
                KeyCode::Char('"') => {
                    split_active(app, LayoutKind::Vertical)?;
                    true
                }
                KeyCode::Char('x') => {
                    app.mode = Mode::ConfirmMode {
                        prompt: "kill-pane? (y/n)".into(),
                        command: "kill-pane".into(),
                        input: String::new(),
                    };
                    true
                }
                KeyCode::Char('d') => {
                    return Ok(true);
                }
                KeyCode::Char('w') => { app.mode = Mode::WindowChooser { selected: app.active_idx }; true }
                KeyCode::Char(',') => { app.mode = Mode::RenamePrompt { input: String::new() }; true }
                KeyCode::Char(' ') => { cycle_top_layout(app); true }
                KeyCode::Char('[') => { enter_copy_mode(app); true }
                KeyCode::Char(']') => { paste_latest(app)?; app.mode = Mode::Passthrough; true }
                KeyCode::Char(':') => {
                    app.mode = Mode::CommandPrompt { input: String::new() };
                    true
                }
                KeyCode::Char('q') => {
                    let win = &app.windows[app.active_idx];
                    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
                    compute_rects(&win.root, app.last_window_area, &mut rects);
                    app.display_map.clear();
                    for (i, (path, _)) in rects.into_iter().enumerate() {
                        let n = i + 1;
                        if n <= 10 { app.display_map.push((n, path)); } else { break; }
                    }
                    app.mode = Mode::PaneChooser { opened_at: Instant::now() };
                    true
                }
                // --- zoom pane (z) ---
                KeyCode::Char('z') => { toggle_zoom(app); true }
                // --- next pane (o) ---
                KeyCode::Char('o') => {
                    let win = &app.windows[app.active_idx];
                    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
                    compute_rects(&win.root, app.last_window_area, &mut rects);
                    if let Some(cur) = rects.iter().position(|r| r.0 == win.active_path) {
                        let next = (cur + 1) % rects.len();
                        let new_path = rects[next].0.clone();
                        let win = &mut app.windows[app.active_idx];
                        app.last_pane_path = win.active_path.clone();
                        win.active_path = new_path;
                    }
                    true
                }
                // --- last pane (;) ---
                KeyCode::Char(';') => {
                    let win = &mut app.windows[app.active_idx];
                    if !app.last_pane_path.is_empty() && path_exists(&win.root, &app.last_pane_path) {
                        let tmp = win.active_path.clone();
                        win.active_path = app.last_pane_path.clone();
                        app.last_pane_path = tmp;
                    }
                    true
                }
                // --- last window (l) ---
                KeyCode::Char('l') => {
                    if app.last_window_idx < app.windows.len() {
                        let tmp = app.active_idx;
                        app.active_idx = app.last_window_idx;
                        app.last_window_idx = tmp;
                    }
                    true
                }
                // --- swap pane up/left ({) ---
                KeyCode::Char('{') => { swap_pane(app, FocusDir::Up); true }
                // --- swap pane down/right (}) ---
                KeyCode::Char('}') => { swap_pane(app, FocusDir::Down); true }
                // --- break pane to new window (!) ---
                KeyCode::Char('!') => { break_pane_to_window(app); true }
                // --- kill window (&) with confirmation ---
                KeyCode::Char('&') => {
                    app.mode = Mode::ConfirmMode {
                        prompt: "kill-window? (y/n)".into(),
                        command: "kill-window".into(),
                        input: String::new(),
                    };
                    true
                }
                // --- rename session ($) ---
                KeyCode::Char('$') => {
                    app.mode = Mode::RenameSessionPrompt { input: String::new() };
                    true
                }
                // --- Meta+1..5 preset layouts (like tmux) ---
                KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "even-horizontal"); true
                }
                KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "even-vertical"); true
                }
                KeyCode::Char('3') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "main-horizontal"); true
                }
                KeyCode::Char('4') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "main-vertical"); true
                }
                KeyCode::Char('5') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "tiled"); true
                }
                // --- display pane info (i) ---
                KeyCode::Char('i') => {
                    // Display window/pane info in status bar (tmux prefix+i)
                    let win = &app.windows[app.active_idx];
                    let pane_count = crate::tree::count_panes(&win.root);
                    app.status_right = format!(
                        "#{} ({}) [{}x{}] panes:{}", 
                        app.active_idx, win.name,
                        app.last_window_area.width, app.last_window_area.height,
                        pane_count
                    );
                    true
                }
                // --- clock mode (t) ---
                KeyCode::Char('t') => {
                    app.mode = Mode::ClockMode;
                    true
                }
                _ => false,
            };

            if matches!(app.mode, Mode::Prefix { .. }) {
                if !handled && elapsed < app.escape_time_ms {
                    return Ok(false);
                }
                app.mode = Mode::Passthrough;
            }
            Ok(false)
        }
        Mode::CommandPrompt { .. } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Enter => { execute_command_prompt(app)?; }
                KeyCode::Backspace => {
                    if let Mode::CommandPrompt { input } = &mut app.mode { let _ = input.pop(); }
                }
                KeyCode::Char(c) => {
                    if let Mode::CommandPrompt { input } = &mut app.mode { input.push(c); }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::WindowChooser { selected } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Up | KeyCode::Left => { if selected > 0 { if let Mode::WindowChooser { selected: s } = &mut app.mode { *s -= 1; } } }
                KeyCode::Down | KeyCode::Right => { if selected + 1 < app.windows.len() { if let Mode::WindowChooser { selected: s } = &mut app.mode { *s += 1; } } }
                KeyCode::Enter => { if let Mode::WindowChooser { selected: s } = &mut app.mode { app.active_idx = *s; app.mode = Mode::Passthrough; } }
                _ => {}
            }
            Ok(false)
        }
        Mode::RenamePrompt { .. } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Enter => { if let Mode::RenamePrompt { input } = &mut app.mode { app.windows[app.active_idx].name = input.clone(); app.mode = Mode::Passthrough; } }
                KeyCode::Backspace => { if let Mode::RenamePrompt { input } = &mut app.mode { let _ = input.pop(); } }
                KeyCode::Char(c) => { if let Mode::RenamePrompt { input } = &mut app.mode { input.push(c); } }
                _ => {}
            }
            Ok(false)
        }
        Mode::RenameSessionPrompt { .. } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Enter => {
                    if let Mode::RenameSessionPrompt { input } = &mut app.mode {
                        app.session_name = input.clone();
                        app.mode = Mode::Passthrough;
                    }
                }
                KeyCode::Backspace => { if let Mode::RenameSessionPrompt { input } = &mut app.mode { let _ = input.pop(); } }
                KeyCode::Char(c) => { if let Mode::RenameSessionPrompt { input } = &mut app.mode { input.push(c); } }
                _ => {}
            }
            Ok(false)
        }
        Mode::CopyMode => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(']') => { 
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
                KeyCode::Left | KeyCode::Char('h') => { move_copy_cursor(app, -1, 0); }
                KeyCode::Right | KeyCode::Char('l') => { move_copy_cursor(app, 1, 0); }
                KeyCode::Up | KeyCode::Char('k') => { scroll_copy_up(app, 1); }
                KeyCode::Down | KeyCode::Char('j') => { scroll_copy_down(app, 1); }
                // Page scroll: C-b / PageUp = page up, C-f / PageDown = page down
                KeyCode::PageUp => { scroll_copy_up(app, 10); }
                KeyCode::PageDown => { scroll_copy_down(app, 10); }
                KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.mode_keys == "emacs" { move_copy_cursor(app, -1, 0); }
                    else { scroll_copy_up(app, 10); }
                }
                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.mode_keys == "emacs" { move_copy_cursor(app, 1, 0); }
                    else { scroll_copy_down(app, 10); }
                }
                // Half-page scroll: C-u / C-d
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half = app.windows.get(app.active_idx)
                        .and_then(|w| active_pane(&w.root, &w.active_path))
                        .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                    scroll_copy_up(app, half);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half = app.windows.get(app.active_idx)
                        .and_then(|w| active_pane(&w.root, &w.active_path))
                        .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                    scroll_copy_down(app, half);
                }
                KeyCode::Char('g') => { scroll_to_top(app); }
                KeyCode::Char('G') => { scroll_to_bottom(app); }
                // Word motions: w = next word, b = prev word, e = end of word
                KeyCode::Char('w') => { crate::copy_mode::move_word_forward(app); }
                KeyCode::Char('b') => { crate::copy_mode::move_word_backward(app); }
                KeyCode::Char('e') => { crate::copy_mode::move_word_end(app); }
                // Line motions: 0 = start, $ = end, ^ = first non-blank
                KeyCode::Char('0') => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::Char('$') => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('^') => { crate::copy_mode::move_to_first_nonblank(app); }
                KeyCode::Home => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::End => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('v') => { if let Some((r,c)) = current_prompt_pos(app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                KeyCode::Char('y') => { yank_selection(app)?; app.mode = Mode::Passthrough; app.copy_scroll_offset = 0; }
                // --- copy-mode search ---
                KeyCode::Char('/') => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: true };
                }
                KeyCode::Char('?') => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: false };
                }
                KeyCode::Char('n') => { search_next(app); }
                KeyCode::Char('N') => { search_prev(app); }
                // Emacs copy-mode keys
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => { scroll_copy_down(app, 1); }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => { scroll_copy_up(app, 1); }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => { scroll_copy_down(app, 10); }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => { scroll_copy_up(app, 10); }
                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::move_word_forward(app); }
                KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::move_word_backward(app); }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::ALT) => { yank_selection(app)?; app.mode = Mode::Passthrough; app.copy_scroll_offset = 0; }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: true };
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: false };
                }
                KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Set mark (anchor)
                    if let Some((r, c)) = current_prompt_pos(app) {
                        app.copy_anchor = Some((r, c));
                        app.copy_pos = Some((r, c));
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::CopySearch { .. } => {
            match key.code {
                KeyCode::Esc => {
                    // Cancel search, return to copy mode
                    app.mode = Mode::CopyMode;
                }
                KeyCode::Enter => {
                    // Execute search
                    if let Mode::CopySearch { ref input, forward } = app.mode {
                        let query = input.clone();
                        let fwd = forward;
                        app.copy_search_query = query.clone();
                        app.copy_search_forward = fwd;
                        search_copy_mode(app, &query, fwd);
                        // Jump to first match
                        if !app.copy_search_matches.is_empty() {
                            let (r, c, _) = app.copy_search_matches[0];
                            app.copy_pos = Some((r, c));
                        }
                    }
                    app.mode = Mode::CopyMode;
                }
                KeyCode::Backspace => {
                    if let Mode::CopySearch { ref mut input, .. } = app.mode { let _ = input.pop(); }
                }
                KeyCode::Char(c) => {
                    if let Mode::CopySearch { ref mut input, .. } = app.mode { input.push(c); }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::PaneChooser { .. } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let raw = d.to_digit(10).unwrap() as usize;
                    let choice = if raw == 0 { 10 } else { raw };
                    if let Some((_, path)) = app.display_map.iter().find(|(n, _)| *n == choice) {
                        let win = &mut app.windows[app.active_idx];
                        win.active_path = path.clone();
                        app.mode = Mode::Passthrough;
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::MenuMode { ref mut menu } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { 
                    app.mode = Mode::Passthrough; 
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if menu.selected > 0 {
                        menu.selected -= 1;
                        while menu.selected > 0 && menu.items.get(menu.selected).map(|i| i.is_separator).unwrap_or(false) {
                            menu.selected -= 1;
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if menu.selected + 1 < menu.items.len() {
                        menu.selected += 1;
                        while menu.selected + 1 < menu.items.len() && menu.items.get(menu.selected).map(|i| i.is_separator).unwrap_or(false) {
                            menu.selected += 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(item) = menu.items.get(menu.selected) {
                        if !item.is_separator && !item.command.is_empty() {
                            let cmd = item.command.clone();
                            app.mode = Mode::Passthrough;
                            let _ = execute_command_string(app, &cmd);
                        } else {
                            app.mode = Mode::Passthrough;
                        }
                    } else {
                        app.mode = Mode::Passthrough;
                    }
                }
                KeyCode::Char(c) => {
                    if let Some((_idx, item)) = menu.items.iter().enumerate().find(|(_, i)| i.key == Some(c)) {
                        if !item.is_separator && !item.command.is_empty() {
                            let cmd = item.command.clone();
                            app.mode = Mode::Passthrough;
                            let _ = execute_command_string(app, &cmd);
                        } else {
                            app.mode = Mode::Passthrough;
                        }
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::PopupMode { ref mut output, ref mut process, close_on_exit, .. } => {
            let mut should_close = false;
            let mut exit_status: Option<std::process::ExitStatus> = None;
            
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    if let Some(ref mut proc) = process {
                        let _ = proc.kill();
                    }
                    should_close = true;
                }
                KeyCode::Char(c) => {
                    output.push(c);
                }
                KeyCode::Enter => {
                    output.push('\n');
                }
                _ => {}
            }
            
            if let Some(ref mut proc) = process {
                if let Ok(Some(status)) = proc.try_wait() {
                    exit_status = Some(status);
                    if close_on_exit {
                        should_close = true;
                    }
                }
            }
            
            if let Some(status) = exit_status {
                if !close_on_exit {
                    output.push_str(&format!("\n[Process exited with status: {}]", status));
                }
            }
            
            if should_close {
                app.mode = Mode::Passthrough;
            }
            
            Ok(false)
        }
        Mode::ConfirmMode { ref prompt, ref command, ref mut input } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.mode = Mode::Passthrough;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let cmd = command.clone();
                    app.mode = Mode::Passthrough;
                    let _ = execute_command_string(app, &cmd);
                }
                KeyCode::Char(c) => {
                    input.push(c);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::ClockMode => {
            // Any key exits clock mode
            app.mode = Mode::Passthrough;
            Ok(false)
        }
    }
}

pub fn move_focus(app: &mut AppState, dir: FocusDir) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    let mut active_idx = None;
    for (i, (path, _)) in rects.iter().enumerate() { if *path == win.active_path { active_idx = Some(i); break; } }
    let Some(ai) = active_idx else { return; };
    let (_, arect) = &rects[ai];
    let mut best: Option<(usize, u32)> = None;
    for (i, (_, r)) in rects.iter().enumerate() {
        if i == ai { continue; }
        let candidate = match dir {
            FocusDir::Left => if r.x + r.width <= arect.x { Some((arect.x - (r.x + r.width)) as u32) } else { None },
            FocusDir::Right => if r.x >= arect.x + arect.width { Some((r.x - (arect.x + arect.width)) as u32) } else { None },
            FocusDir::Up => if r.y + r.height <= arect.y { Some((arect.y - (r.y + r.height)) as u32) } else { None },
            FocusDir::Down => if r.y >= arect.y + arect.height { Some((r.y - (arect.y + arect.height)) as u32) } else { None },
        };
        if let Some(dist) = candidate { if best.map_or(true, |(_,bd)| dist < bd) { best = Some((i, dist)); } }
    }
    if let Some((ni, _)) = best { win.active_path = rects[ni].0.clone(); }
}

pub fn forward_key_to_active(app: &mut AppState, key: KeyEvent) -> io::Result<()> {
    // Encode the key into bytes
    let encoded: Vec<u8> = match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::ALT) => {
            let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
            vec![0x1b, ctrl_char]
        }
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
            format!("\x1b{}", c).into_bytes()
        }
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
            vec![ctrl_char]
        }
        KeyCode::Char(c) if (c as u8) >= 0x01 && (c as u8) <= 0x1A => {
            vec![c as u8]
        }
        KeyCode::Char(c) => {
            format!("{}", c).into_bytes()
        }
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Backspace => b"\x08".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        _ => return Ok(()),
    };

    if app.sync_input {
        // Fan out to ALL panes in the current window
        let win = &mut app.windows[app.active_idx];
        fn write_all_panes(node: &mut Node, data: &[u8]) {
            match node {
                Node::Leaf(p) if !p.dead => { let _ = p.master.write_all(data); }
                Node::Leaf(_) => {}
                Node::Split { children, .. } => { for c in children { write_all_panes(c, data); } }
            }
        }
        write_all_panes(&mut win.root, &encoded);
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            if !active.dead {
                let _ = active.master.write_all(&encoded);
            }
        }
    }
    Ok(())
}

fn wheel_cell_for_area(area: Rect, x: u16, y: u16) -> (u16, u16) {
    // Convert global terminal coordinates to 1-based pane-local coordinates.
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_w = area.width.saturating_sub(2).max(1);
    let inner_h = area.height.saturating_sub(2).max(1);

    let col = x
        .saturating_sub(inner_x)
        .min(inner_w.saturating_sub(1))
        .saturating_add(1);
    let row = y
        .saturating_sub(inner_y)
        .min(inner_h.saturating_sub(1))
        .saturating_add(1);
    (col, row)
}

pub fn handle_mouse(app: &mut AppState, me: MouseEvent, window_area: Rect) -> io::Result<()> {
    use crossterm::event::{MouseEventKind, MouseButton};

    // --- Tab click: check if click is on the status bar row ---
    let status_row = window_area.y + window_area.height; // status bar is 1 row below window area
    if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) && me.row == status_row {
        for &(win_idx, x_start, x_end) in app.tab_positions.iter() {
            if me.column >= x_start && me.column < x_end {
                if win_idx < app.windows.len() {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = win_idx;
                }
                return Ok(());
            }
        }
        // Click was on status bar but not on a tab — ignore
        return Ok(());
    }

    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, window_area, &mut rects);
    let mut borders: Vec<(Vec<usize>, LayoutKind, usize, u16, u16)> = Vec::new();
    compute_split_borders(&win.root, window_area, &mut borders);
    let mut active_area = rects
        .iter()
        .find(|(path, _)| *path == win.active_path)
        .map(|(_, area)| *area);

    /// Convert absolute screen coordinates to pane-local 1-based cell coordinates,
    /// accounting for the 1px border on each side (Block with Borders::ALL).
    fn pane_cell(area: Rect, abs_x: u16, abs_y: u16) -> (u16, u16) {
        let col = abs_x.saturating_sub(area.x + 1) + 1;
        let row = abs_y.saturating_sub(area.y + 1) + 1;
        (col, row)
    }

    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Check if click is on a split border (for dragging)
            let mut on_border = false;
            let tol = 1u16;
            for (path, kind, idx, pos, total_px) in borders.iter() {
                match kind {
                    LayoutKind::Horizontal => {
                        if me.column >= pos.saturating_sub(tol) && me.column <= pos + tol {
                            if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) {
                                app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: *pos, start_y: me.row, left_initial: left, _right_initial: right, total_pixels: *total_px });
                            }
                            on_border = true;
                            break;
                        }
                    }
                    LayoutKind::Vertical => {
                        if me.row >= pos.saturating_sub(tol) && me.row <= pos + tol {
                            if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) {
                                app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: me.column, start_y: *pos, left_initial: left, _right_initial: right, total_pixels: *total_px });
                            }
                            on_border = true;
                            break;
                        }
                    }
                }
            }

            // Switch pane focus if clicking inside a pane
            for (path, area) in rects.iter() {
                if area.contains(ratatui::layout::Position { x: me.column, y: me.row }) {
                    win.active_path = path.clone();
                    active_area = Some(*area);
                }
            }

            // Forward left-click to child pane via Windows Console API
            if !on_border {
                if let Some(area) = active_area {
                    if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                        let col = me.column.saturating_sub(area.x + 1) as i16;
                        let row = me.row.saturating_sub(area.y + 1) as i16;
                        if active.child_pid.is_none() {
                            active.child_pid = unsafe { crate::platform::mouse_inject::get_child_pid(&*active.child) };
                        }
                        if let Some(pid) = active.child_pid {
                            crate::platform::mouse_inject::send_mouse_event(
                                pid, col, row,
                                crate::platform::mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED, 0,
                                true,
                            );
                        }
                    }
                }
            }

        }
        MouseEventKind::Down(MouseButton::Right) => {
            // Right-click not forwarded - reserved for psmux context menu
        }
        MouseEventKind::Down(MouseButton::Middle) => {
            // Middle-click not forwarded - reserved for paste
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let was_dragging = app.drag.is_some();
            app.drag = None;
            if was_dragging {
                resize_all_panes(app);
            } else if let Some(area) = active_area {
                // Forward mouse release via Windows Console API
                if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                    let col = me.column.saturating_sub(area.x + 1) as i16;
                    let row = me.row.saturating_sub(area.y + 1) as i16;
                    if let Some(pid) = active.child_pid {
                        crate::platform::mouse_inject::send_mouse_event(pid, col, row, 0, 0, true);
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Right) => {
            // Not forwarded
        }
        MouseEventKind::Up(MouseButton::Middle) => {
            // Not forwarded
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(d) = &app.drag {
                adjust_split_sizes(&mut win.root, d, me.column, me.row);
            } else {
                // Forward drag via Windows Console API
                if let Some(area) = active_area {
                    if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                        let col = me.column.saturating_sub(area.x + 1) as i16;
                        let row = me.row.saturating_sub(area.y + 1) as i16;
                        if let Some(pid) = active.child_pid {
                            crate::platform::mouse_inject::send_mouse_event(
                                pid, col, row,
                                crate::platform::mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED,
                                crate::platform::mouse_inject::MOUSE_MOVED,
                                true,
                            );
                        }
                    }
                }
            }
        }
        MouseEventKind::Moved => {
            // Don't forward bare motion - only forward drag events
            // Most TUI apps don't want constant mouse position updates
        }
        MouseEventKind::ScrollUp => {
            if matches!(app.mode, Mode::CopyMode) {
                scroll_copy_up(app, 3);
                return Ok(());
            }
            if let Some((path, area)) = rects.iter().find(|(_, area)| area.contains(ratatui::layout::Position { x: me.column, y: me.row })) {
                win.active_path = path.clone();
                active_area = Some(*area);
            }
            let (col, row) = active_area.map_or((1, 1), |area| wheel_cell_for_area(area, me.column, me.row));
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                let _ = write!(active.master, "\x1b[<64;{};{}M", col, row);
            }
        }
        MouseEventKind::ScrollDown => {
            if matches!(app.mode, Mode::CopyMode) {
                scroll_copy_down(app, 3);
                return Ok(());
            }
            if let Some((path, area)) = rects.iter().find(|(_, area)| area.contains(ratatui::layout::Position { x: me.column, y: me.row })) {
                win.active_path = path.clone();
                active_area = Some(*area);
            }
            let (col, row) = active_area.map_or((1, 1), |area| wheel_cell_for_area(area, me.column, me.row));
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                let _ = write!(active.master, "\x1b[<65;{};{}M", col, row);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn send_text_to_active(app: &mut AppState, text: &str) -> io::Result<()> {
    // In clock mode, any input exits back to passthrough
    if matches!(app.mode, Mode::ClockMode) {
        app.mode = Mode::Passthrough;
        return Ok(());
    }
    // In copy mode, interpret characters as copy-mode actions (never send to PTY)
    if matches!(app.mode, Mode::CopyMode) {
        for c in text.chars() {
            handle_copy_mode_char(app, c)?;
        }
        return Ok(());
    }
    // In copy-search mode, append characters to the search input
    if matches!(app.mode, Mode::CopySearch { .. }) {
        if let Mode::CopySearch { ref mut input, .. } = app.mode {
            for c in text.chars() {
                input.push(c);
            }
        }
        return Ok(());
    }

    if app.sync_input {
        // Fan out to ALL panes in the current window
        let win = &mut app.windows[app.active_idx];
        fn write_all_panes(node: &mut Node, text: &[u8]) {
            match node {
                Node::Leaf(p) => { let _ = p.master.write_all(text); let _ = p.master.flush(); }
                Node::Split { children, .. } => { for c in children { write_all_panes(c, text); } }
            }
        }
        write_all_panes(&mut win.root, text.as_bytes());
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = p.master.write_all(text.as_bytes());
            let _ = p.master.flush();
        }
    }
    Ok(())
}

/// Dispatch a single character as a copy-mode action.
fn handle_copy_mode_char(app: &mut AppState, c: char) -> io::Result<()> {
    match c {
        'q' | ']' | '\x1b' => {
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
        'h' => { move_copy_cursor(app, -1, 0); }
        'l' => { move_copy_cursor(app, 1, 0); }
        'k' => { scroll_copy_up(app, 1); }
        'j' => { scroll_copy_down(app, 1); }
        'g' => { scroll_to_top(app); }
        'G' => { scroll_to_bottom(app); }
        'w' => { crate::copy_mode::move_word_forward(app); }
        'b' => { crate::copy_mode::move_word_backward(app); }
        'e' => { crate::copy_mode::move_word_end(app); }
        '0' => { crate::copy_mode::move_to_line_start(app); }
        '$' => { crate::copy_mode::move_to_line_end(app); }
        '^' => { crate::copy_mode::move_to_first_nonblank(app); }
        'v' => {
            if let Some((r, c)) = current_prompt_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_pos = Some((r, c));
            }
        }
        'y' => { yank_selection(app)?; app.mode = Mode::Passthrough; app.copy_scroll_offset = 0; }
        '/' => { app.mode = Mode::CopySearch { input: String::new(), forward: true }; }
        '?' => { app.mode = Mode::CopySearch { input: String::new(), forward: false }; }
        'n' => { search_next(app); }
        'N' => { search_prev(app); }
        _ => {} // Swallow unrecognized characters in copy mode
    }
    Ok(())
}

pub fn send_key_to_active(app: &mut AppState, k: &str) -> io::Result<()> {
    // In clock mode, any key exits back to passthrough
    if matches!(app.mode, Mode::ClockMode) {
        app.mode = Mode::Passthrough;
        return Ok(());
    }
    // --- Copy-search mode: handle esc/enter/backspace ---
    if matches!(app.mode, Mode::CopySearch { .. }) {
        match k {
            "esc" => { app.mode = Mode::CopyMode; }
            "enter" => {
                if let Mode::CopySearch { ref input, forward } = app.mode {
                    let query = input.clone();
                    let fwd = forward;
                    app.copy_search_query = query.clone();
                    app.copy_search_forward = fwd;
                    search_copy_mode(app, &query, fwd);
                    if !app.copy_search_matches.is_empty() {
                        let (r, c, _) = app.copy_search_matches[0];
                        app.copy_pos = Some((r, c));
                    }
                }
                app.mode = Mode::CopyMode;
            }
            "backspace" => {
                if let Mode::CopySearch { ref mut input, .. } = app.mode { input.pop(); }
            }
            _ => {}
        }
        return Ok(());
    }

    // --- Copy mode: full vi-style key table ---
    if matches!(app.mode, Mode::CopyMode) {
        match k {
            "esc" | "q" => {
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
            "up" => { scroll_copy_up(app, 1); }
            "down" => { scroll_copy_down(app, 1); }
            "pageup" => { scroll_copy_up(app, 10); }
            "pagedown" => { scroll_copy_down(app, 10); }
            "left" => { move_copy_cursor(app, -1, 0); }
            "right" => { move_copy_cursor(app, 1, 0); }
            "home" => { crate::copy_mode::move_to_line_start(app); }
            "end" => { crate::copy_mode::move_to_line_end(app); }
            "C-b" | "c-b" => {
                if app.mode_keys == "emacs" { move_copy_cursor(app, -1, 0); }
                else { scroll_copy_up(app, 10); }
            }
            "C-f" | "c-f" => {
                if app.mode_keys == "emacs" { move_copy_cursor(app, 1, 0); }
                else { scroll_copy_down(app, 10); }
            }
            "C-n" | "c-n" => { scroll_copy_down(app, 1); }
            "C-p" | "c-p" => { scroll_copy_up(app, 1); }
            "C-a" | "c-a" => { crate::copy_mode::move_to_line_start(app); }
            "C-e" | "c-e" => { crate::copy_mode::move_to_line_end(app); }
            "C-v" | "c-v" => { scroll_copy_down(app, 10); }
            "M-v" | "m-v" => { scroll_copy_up(app, 10); }
            "M-f" | "m-f" => { crate::copy_mode::move_word_forward(app); }
            "M-b" | "m-b" => { crate::copy_mode::move_word_backward(app); }
            "M-w" | "m-w" => { yank_selection(app)?; app.mode = Mode::Passthrough; app.copy_scroll_offset = 0; }
            "C-s" | "c-s" => { app.mode = Mode::CopySearch { input: String::new(), forward: true }; }
            "C-r" | "c-r" => { app.mode = Mode::CopySearch { input: String::new(), forward: false }; }
            "C-g" | "c-g" => {
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
            "c-space" | "C-space" => {
                // Set mark (anchor) at current position
                if let Some((r, c)) = current_prompt_pos(app) {
                    app.copy_anchor = Some((r, c));
                    app.copy_pos = Some((r, c));
                }
            }
            "C-u" | "c-u" => {
                let half = app.windows.get(app.active_idx)
                    .and_then(|w| active_pane(&w.root, &w.active_path))
                    .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                scroll_copy_up(app, half);
            }
            "C-d" | "c-d" => {
                let half = app.windows.get(app.active_idx)
                    .and_then(|w| active_pane(&w.root, &w.active_path))
                    .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                scroll_copy_down(app, half);
            }
            _ => {}
        }
        return Ok(());
    }
    
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        match k {
            "enter" => { let _ = write!(p.master, "\r"); }
            "tab" => { let _ = write!(p.master, "\t"); }
            "backspace" => { let _ = p.master.write_all(&[0x7F]); }
            "delete" => { let _ = write!(p.master, "\x1b[3~"); }
            "esc" => { let _ = write!(p.master, "\x1b"); }
            "left" => { let _ = write!(p.master, "\x1b[D"); }
            "right" => { let _ = write!(p.master, "\x1b[C"); }
            "up" => { let _ = write!(p.master, "\x1b[A"); }
            "down" => { let _ = write!(p.master, "\x1b[B"); }
            "pageup" => { let _ = write!(p.master, "\x1b[5~"); }
            "pagedown" => { let _ = write!(p.master, "\x1b[6~"); }
            "home" => { let _ = write!(p.master, "\x1b[H"); }
            "end" => { let _ = write!(p.master, "\x1b[F"); }
            "insert" => { let _ = write!(p.master, "\x1b[2~"); }
            "space" => { let _ = write!(p.master, " "); }
            s if s.starts_with("f") && s.len() >= 2 && s.len() <= 3 => {
                if let Ok(n) = s[1..].parse::<u8>() {
                    let seq = match n {
                        1 => "\x1bOP",
                        2 => "\x1bOQ",
                        3 => "\x1bOR",
                        4 => "\x1bOS",
                        5 => "\x1b[15~",
                        6 => "\x1b[17~",
                        7 => "\x1b[18~",
                        8 => "\x1b[19~",
                        9 => "\x1b[20~",
                        10 => "\x1b[21~",
                        11 => "\x1b[23~",
                        12 => "\x1b[24~",
                        _ => "",
                    };
                    if !seq.is_empty() { let _ = write!(p.master, "{}", seq); }
                }
            }
            s if s.starts_with("C-") && s.len() == 3 => {
                let c = s.chars().nth(2).unwrap_or('c');
                let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
                let _ = p.master.write_all(&[ctrl_char]);
            }
            s if (s.starts_with("M-") || s.starts_with("m-")) && s.len() == 3 => {
                let c = s.chars().nth(2).unwrap_or('a');
                let _ = write!(p.master, "\x1b{}", c);
            }
            s if (s.starts_with("C-M-") || s.starts_with("c-m-")) && s.len() == 5 => {
                let c = s.chars().nth(4).unwrap_or('c');
                let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
                let _ = p.master.write_all(&[0x1b, ctrl_char]);
            }
            _ => {}
        }
    }
    Ok(())
}
