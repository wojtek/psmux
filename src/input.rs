use std::io::{self, Write};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use portable_pty::native_pty_system;
use ratatui::prelude::*;

use crate::types::{AppState, Mode, FocusDir, LayoutKind, DragState, Node};
use crate::tree::{active_pane, active_pane_mut, compute_rects, compute_split_borders,
    split_sizes_at, adjust_split_sizes, path_exists, resize_all_panes};
use crate::pane::{create_window, split_active};
use crate::commands::{execute_action, execute_command_prompt, execute_command_string};
use crate::config::normalize_key_for_binding;
use crate::copy_mode::{enter_copy_mode, exit_copy_mode, switch_with_copy_save, move_copy_cursor,
    scroll_copy_up, scroll_copy_down, paste_latest, yank_selection,
    search_copy_mode, search_next, search_prev, scroll_to_top, scroll_to_bottom,
    save_copy_state_to_pane, restore_copy_state_from_pane};
use crate::layout::{cycle_top_layout, apply_layout};
use crate::window_ops::{toggle_zoom, swap_pane, break_pane_to_window};

/// Write a mouse event to the child PTY using the encoding the child requested.
fn write_mouse_event(master: &mut dyn std::io::Write, button: u8, col: u16, row: u16, press: bool, enc: vt100::MouseProtocolEncoding) {
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
    match app.mode {
        Mode::Passthrough => {
            // Check switch-client -T key table first
            if let Some(table_name) = app.current_key_table.take() {
                let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
                if let Some(bind) = app.key_tables.get(&table_name)
                    .and_then(|t| t.iter().find(|b| b.key == key_tuple))
                    .cloned()
                {
                    return execute_action(app, &bind.action);
                }
                // Key not found in table — fall through to normal dispatch
            }
            let is_prefix = (key.code, key.modifiers) == app.prefix_key
                || matches!(key.code, KeyCode::Char(c) if c == '\u{0002}')
                || app.prefix2_key.map_or(false, |p2| (key.code, key.modifiers) == p2);
            if is_prefix {
                app.mode = Mode::Prefix { armed_at: Instant::now() };
                return Ok(false);
            }
            // Check root key table for bindings (bind-key -n / bind-key -T root)
            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
            if let Some(bind) = app.key_tables.get("root").and_then(|t| t.iter().find(|b| b.key == key_tuple)).cloned() {
                return execute_action(app, &bind.action);
            }
            forward_key_to_active(app, key)?;
            Ok(false)
        }
        Mode::Prefix { armed_at } => {
            let elapsed = armed_at.elapsed().as_millis() as u64;
            
            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
            if let Some(bind) = app.key_tables.get("prefix").and_then(|t| t.iter().find(|b| b.key == key_tuple)).cloned() {
                if bind.repeat {
                    // Stay in prefix mode for repeat-time window
                    app.mode = Mode::Prefix { armed_at: Instant::now() };
                } else {
                    app.mode = Mode::Passthrough;
                }
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
                KeyCode::Left => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Left)); true }
                KeyCode::Right => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Right)); true }
                KeyCode::Up => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Up)); true }
                KeyCode::Down => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Down)); true }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let idx = d.to_digit(10).unwrap() as usize;
                    if idx >= app.window_base_index {
                        let internal_idx = idx - app.window_base_index;
                        if internal_idx < app.windows.len() {
                            switch_with_copy_save(app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                        }
                    }
                    true
                }
                KeyCode::Char('c') => {
                    let pty_system = native_pty_system();
                    create_window(&*pty_system, app, None)?;
                    true
                }
                KeyCode::Char('n') => {
                    if !app.windows.is_empty() {
                        switch_with_copy_save(app, |app| {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = (app.active_idx + 1) % app.windows.len();
                        });
                    }
                    true
                }
                KeyCode::Char('p') => {
                    if !app.windows.is_empty() {
                        switch_with_copy_save(app, |app| {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                        });
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
                KeyCode::Char('w') => {
                    let tree = crate::commands::build_choose_tree(app);
                    let selected = tree.iter().position(|e| e.is_current_session && e.is_active_window && !e.is_session_header).unwrap_or(0);
                    app.mode = Mode::WindowChooser { selected, tree };
                    true
                }
                KeyCode::Char(',') => { app.mode = Mode::RenamePrompt { input: String::new() }; true }
                KeyCode::Char(' ') => { cycle_top_layout(app); true }
                KeyCode::Char('[') => { enter_copy_mode(app); true }
                KeyCode::Char(']') => { paste_latest(app)?; app.mode = Mode::Passthrough; true }
                KeyCode::Char(':') => {
                    app.mode = Mode::CommandPrompt { input: String::new(), cursor: 0 };
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
                    switch_with_copy_save(app, |app| {
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
                    });
                    true
                }
                // --- last pane (;) ---
                KeyCode::Char(';') => {
                    switch_with_copy_save(app, |app| {
                        let win = &mut app.windows[app.active_idx];
                        if !app.last_pane_path.is_empty() && path_exists(&win.root, &app.last_pane_path) {
                            let tmp = win.active_path.clone();
                            win.active_path = app.last_pane_path.clone();
                            app.last_pane_path = tmp;
                        }
                    });
                    true
                }
                // --- last window (l) ---
                KeyCode::Char('l') => {
                    if app.last_window_idx < app.windows.len() {
                        switch_with_copy_save(app, |app| {
                            let tmp = app.active_idx;
                            app.active_idx = app.last_window_idx;
                            app.last_window_idx = tmp;
                        });
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
                // --- buffer chooser (=) ---
                KeyCode::Char('=') => {
                    app.mode = Mode::BufferChooser { selected: 0 };
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
                KeyCode::Enter => {
                    // Save to history before executing
                    if let Mode::CommandPrompt { input, .. } = &app.mode {
                        if !input.is_empty() {
                            let cmd = input.clone();
                            app.command_history.push(cmd);
                            app.command_history_idx = app.command_history.len();
                        }
                    }
                    execute_command_prompt(app)?;
                }
                KeyCode::Backspace => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        if *cursor > 0 {
                            input.remove(*cursor - 1);
                            *cursor -= 1;
                        }
                    }
                }
                KeyCode::Delete => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        if *cursor < input.len() {
                            input.remove(*cursor);
                        }
                    }
                }
                KeyCode::Left => {
                    if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                        if *cursor > 0 { *cursor -= 1; }
                    }
                }
                KeyCode::Right => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        if *cursor < input.len() { *cursor += 1; }
                    }
                }
                KeyCode::Home => {
                    if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                        *cursor = 0;
                    }
                }
                KeyCode::End => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        *cursor = input.len();
                    }
                }
                KeyCode::Up => {
                    // Cycle through command history (older)
                    if app.command_history_idx > 0 {
                        app.command_history_idx -= 1;
                        let cmd = app.command_history[app.command_history_idx].clone();
                        let len = cmd.len();
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            *input = cmd;
                            *cursor = len;
                        }
                    }
                }
                KeyCode::Down => {
                    // Cycle through command history (newer)
                    if app.command_history_idx < app.command_history.len() {
                        app.command_history_idx += 1;
                        let cmd = if app.command_history_idx < app.command_history.len() {
                            app.command_history[app.command_history_idx].clone()
                        } else {
                            String::new()
                        };
                        let len = cmd.len();
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            *input = cmd;
                            *cursor = len;
                        }
                    }
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+A: move to beginning
                    if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                        *cursor = 0;
                    }
                }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+E: move to end
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        *cursor = input.len();
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+U: kill line (clear from cursor to start)
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        input.drain(..*cursor);
                        *cursor = 0;
                    }
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+K: kill to end of line
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        input.truncate(*cursor);
                    }
                }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+W: delete word backwards
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        let mut pos = *cursor;
                        while pos > 0 && input.as_bytes().get(pos - 1) == Some(&b' ') { pos -= 1; }
                        while pos > 0 && input.as_bytes().get(pos - 1) != Some(&b' ') { pos -= 1; }
                        input.drain(pos..*cursor);
                        *cursor = pos;
                    }
                }
                KeyCode::Char(c) => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        input.insert(*cursor, c);
                        *cursor += 1;
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::WindowChooser { selected, ref tree } => {
            let tree_len = tree.len();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 { if let Mode::WindowChooser { selected: s, .. } = &mut app.mode { *s -= 1; } }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected + 1 < tree_len { if let Mode::WindowChooser { selected: s, .. } = &mut app.mode { *s += 1; } }
                }
                KeyCode::Enter => {
                    if let Mode::WindowChooser { selected: s, ref tree } = &app.mode {
                        let entry = &tree[*s];
                        if entry.is_current_session {
                            // Same session: switch window directly
                            if let Some(wi) = entry.window_index {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = wi;
                            }
                        } else {
                            // Different session: set env and trigger switch
                            std::env::set_var("PSMUX_SWITCH_TO", &entry.session_name);
                        }
                    }
                    app.mode = Mode::Passthrough;
                }
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    // Quick-select by window number
                    let n = c.to_digit(10).unwrap_or(0) as usize;
                    if let Some(idx) = tree.iter().position(|e| !e.is_session_header && e.window_index == Some(n) && e.is_current_session) {
                        if let Mode::WindowChooser { selected: s, .. } = &mut app.mode { *s = idx; }
                    }
                }
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
            // Check copy-mode key table for user bindings first (used by plugins like tmux-yank)
            let table_name = if app.mode_keys == "vi" { "copy-mode-vi" } else { "copy-mode" };
            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
            if let Some(bind) = app.key_tables.get(table_name)
                .and_then(|t| t.iter().find(|b| b.key == key_tuple))
                .cloned()
            {
                return execute_action(app, &bind.action);
            }
            // Handle register pending state (waiting for a-z after ")
            if app.copy_register_pending {
                app.copy_register_pending = false;
                if let KeyCode::Char(ch) = key.code {
                    if ch.is_ascii_lowercase() {
                        app.copy_register = Some(ch);
                    }
                }
                return Ok(false);
            }
            // Handle text-object pending state (waiting for w/W after a/i)
            if let Some(prefix) = app.copy_text_object_pending.take() {
                if let KeyCode::Char(ch) = key.code {
                    match (prefix, ch) {
                        (0, 'w') => { crate::copy_mode::select_a_word(app); }
                        (1, 'w') => { crate::copy_mode::select_inner_word(app); }
                        (0, 'W') => { crate::copy_mode::select_a_word_big(app); }
                        (1, 'W') => { crate::copy_mode::select_inner_word_big(app); }
                        _ => {}
                    }
                }
                return Ok(false);
            }
            // Handle find-char pending state (waiting for char after f/F/t/T)
            if let Some(pending) = app.copy_find_char_pending.take() {
                let n = app.copy_count.take().unwrap_or(1);
                if let KeyCode::Char(ch) = key.code {
                    match pending {
                        0 => { for _ in 0..n { crate::copy_mode::find_char_forward(app, ch); } }
                        1 => { for _ in 0..n { crate::copy_mode::find_char_backward(app, ch); } }
                        2 => { for _ in 0..n { crate::copy_mode::find_char_to_forward(app, ch); } }
                        3 => { for _ in 0..n { crate::copy_mode::find_char_to_backward(app, ch); } }
                        _ => {}
                    }
                }
                return Ok(false);
            }
            // Handle numeric prefix accumulation for copy-mode motions (vi-style)
            if let KeyCode::Char(d) = key.code {
                if d.is_ascii_digit() && !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) {
                    let digit = d.to_digit(10).unwrap() as usize;
                    if let Some(count) = app.copy_count {
                        // Accumulate: multiply by 10 and add digit (cap at 9999)
                        app.copy_count = Some((count * 10 + digit).min(9999));
                        return Ok(false);
                    } else if digit >= 1 {
                        // Start new count with 1-9
                        app.copy_count = Some(digit);
                        return Ok(false);
                    }
                    // digit == 0 with no existing count → fall through to line-start handler
                }
            }
            let copy_repeat = app.copy_count.take().unwrap_or(1);
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(']') => { 
                    exit_copy_mode(app);
                }
                // Ctrl+C exits copy mode (tmux parity, fixes #25)
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    exit_copy_mode(app);
                }
                KeyCode::Left | KeyCode::Char('h') => { for _ in 0..copy_repeat { move_copy_cursor(app, -1, 0); } }
                KeyCode::Right | KeyCode::Char('l') => { for _ in 0..copy_repeat { move_copy_cursor(app, 1, 0); } }
                KeyCode::Up | KeyCode::Char('k') => { for _ in 0..copy_repeat { move_copy_cursor(app, 0, -1); } }
                KeyCode::Down | KeyCode::Char('j') => { for _ in 0..copy_repeat { move_copy_cursor(app, 0, 1); } }
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
                // Emacs copy-mode keys (must be before unqualified char matches)
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => { scroll_copy_down(app, 1); }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => { scroll_copy_up(app, 1); }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => { scroll_copy_up(app, 10); }
                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::move_word_forward(app); }
                KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::move_word_backward(app); }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::ALT) => { yank_selection(app)?; exit_copy_mode(app); }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: true };
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: false };
                }
                KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    exit_copy_mode(app);
                }
                KeyCode::Char('g') => { scroll_to_top(app); }
                KeyCode::Char('G') => { scroll_to_bottom(app); }
                // Word motions: w = next word, b = prev word, e = end of word
                KeyCode::Char('w') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_forward(app); } }
                KeyCode::Char('b') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_backward(app); } }
                KeyCode::Char('e') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_end(app); } }
                // WORD motions: W = next WORD, B = prev WORD, E = end WORD
                KeyCode::Char('W') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_forward_big(app); } }
                KeyCode::Char('B') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_backward_big(app); } }
                KeyCode::Char('E') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_end_big(app); } }
                // Screen position: H = top, M = middle, L = bottom
                KeyCode::Char('H') => { crate::copy_mode::move_to_screen_top(app); }
                KeyCode::Char('M') => { crate::copy_mode::move_to_screen_middle(app); }
                KeyCode::Char('L') => { crate::copy_mode::move_to_screen_bottom(app); }
                // Find char: f/F/t/T — sets pending state for next char
                KeyCode::Char('f') => { app.copy_find_char_pending = Some(0); app.copy_count = Some(copy_repeat); }
                KeyCode::Char('F') => { app.copy_find_char_pending = Some(1); app.copy_count = Some(copy_repeat); }
                KeyCode::Char('t') => { app.copy_find_char_pending = Some(2); app.copy_count = Some(copy_repeat); }
                KeyCode::Char('T') => { app.copy_find_char_pending = Some(3); app.copy_count = Some(copy_repeat); }
                // D = copy from cursor to end of line
                KeyCode::Char('D') => { crate::copy_mode::copy_end_of_line(app)?; exit_copy_mode(app); }
                // Line motions: 0 = start, $ = end, ^ = first non-blank
                KeyCode::Char('0') => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::Char('$') => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('^') => { crate::copy_mode::move_to_first_nonblank(app); }
                KeyCode::Home => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::End => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // vi: toggle rectangle selection, emacs: page down
                    if app.mode_keys == "emacs" {
                        scroll_copy_down(app, 10);
                    } else {
                        app.copy_selection_mode = crate::types::SelectionMode::Rect;
                    }
                }
                KeyCode::Char('v') => {
                    // Start char-wise selection (vi visual mode)
                    if let Some((r,c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r,c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r,c));
                        app.copy_selection_mode = crate::types::SelectionMode::Char;
                    }
                }
                KeyCode::Char('V') => {
                    // Start line-wise selection (vi visual-line mode)
                    if let Some((r,c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r,c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r,c));
                        app.copy_selection_mode = crate::types::SelectionMode::Line;
                    }
                }
                KeyCode::Char('o') => {
                    // Swap cursor and anchor
                    if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                        app.copy_anchor = Some(p);
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some(a);
                    }
                }
                KeyCode::Char('A') => {
                    // Append to buffer (yank + append to buffer 0)
                    if let (Some(_), Some(_)) = (app.copy_anchor, app.copy_pos) {
                        // Save current buffer 0
                        let prev = app.paste_buffers.first().cloned().unwrap_or_default();
                        yank_selection(app)?;
                        // buffer 0 is now the new yank; prepend old text
                        if let Some(buf) = app.paste_buffers.first_mut() {
                            let new_text = buf.clone();
                            *buf = format!("{}{}", prev, new_text);
                        }
                        exit_copy_mode(app);
                    }
                }
                // Space = begin selection (vi mode), Enter = copy-selection-and-cancel
                KeyCode::Char(' ') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some((r,c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r,c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r,c));
                        app.copy_selection_mode = crate::types::SelectionMode::Char;
                    }
                }
                KeyCode::Enter => {
                    // Copy selection and exit copy mode (vi Enter)
                    if app.copy_anchor.is_some() {
                        yank_selection(app)?;
                    }
                    exit_copy_mode(app);
                }
                KeyCode::Char('y') => { yank_selection(app)?; exit_copy_mode(app); }
                // --- copy-mode search ---
                KeyCode::Char('/') => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: true };
                }
                KeyCode::Char('?') => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: false };
                }
                KeyCode::Char('n') => { search_next(app); }
                KeyCode::Char('N') => { search_prev(app); }
                KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Set mark (anchor)
                    if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r, c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r, c));
                    }
                }
                // Named register prefix: " then a-z
                KeyCode::Char('"') => { app.copy_register_pending = true; }
                // Text-object prefixes: a/i then w/W
                KeyCode::Char('a') if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    app.copy_text_object_pending = Some(0);
                }
                KeyCode::Char('i') if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    app.copy_text_object_pending = Some(1);
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
        Mode::PopupMode { ref mut output, ref mut process, close_on_exit, ref mut popup_pty, .. } => {
            let mut should_close = false;
            let mut exit_status: Option<std::process::ExitStatus> = None;
            
            // If we have a PTY popup, forward keys to it
            if let Some(ref mut pty) = popup_pty {
                match key.code {
                    KeyCode::Esc => {
                        // Check if the child has exited
                        if let Ok(Some(_)) = pty.child.try_wait() {
                            should_close = true;
                        } else {
                            // Forward Escape to the PTY
                            let _ = pty.writer.write_all(b"\x1b");
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                            let ctrl = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                            let _ = pty.writer.write_all(&[ctrl]);
                        } else {
                            let mut buf = [0u8; 4];
                            let s = c.encode_utf8(&mut buf);
                            let _ = pty.writer.write_all(s.as_bytes());
                        }
                    }
                    KeyCode::Enter => { let _ = pty.writer.write_all(b"\r"); }
                    KeyCode::Backspace => { let _ = pty.writer.write_all(b"\x7f"); }
                    KeyCode::Tab => { let _ = pty.writer.write_all(b"\t"); }
                    KeyCode::BackTab => { let _ = pty.writer.write_all(b"\x1b[Z"); }
                    KeyCode::Up => { let _ = pty.writer.write_all(b"\x1b[A"); }
                    KeyCode::Down => { let _ = pty.writer.write_all(b"\x1b[B"); }
                    KeyCode::Right => { let _ = pty.writer.write_all(b"\x1b[C"); }
                    KeyCode::Left => { let _ = pty.writer.write_all(b"\x1b[D"); }
                    KeyCode::Home => { let _ = pty.writer.write_all(b"\x1b[H"); }
                    KeyCode::End => { let _ = pty.writer.write_all(b"\x1b[F"); }
                    KeyCode::PageUp => { let _ = pty.writer.write_all(b"\x1b[5~"); }
                    KeyCode::PageDown => { let _ = pty.writer.write_all(b"\x1b[6~"); }
                    KeyCode::Delete => { let _ = pty.writer.write_all(b"\x1b[3~"); }
                    _ => {}
                }
                // Check if child exited
                if let Ok(Some(_status)) = pty.child.try_wait() {
                    if close_on_exit {
                        should_close = true;
                    }
                }
            } else {
                // Non-PTY popup (static output)
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
            }
            
            if should_close {
                app.mode = Mode::Passthrough;
            }
            
            Ok(false)
        }
        Mode::ConfirmMode { prompt: _, ref command, ref mut input } => {
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
        Mode::BufferChooser { selected } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 {
                        if let Mode::BufferChooser { selected: s } = &mut app.mode { *s -= 1; }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = app.paste_buffers.len().saturating_sub(1);
                    if selected < max {
                        if let Mode::BufferChooser { selected: s } = &mut app.mode { *s += 1; }
                    }
                }
                KeyCode::Enter => {
                    // Paste selected buffer
                    if selected < app.paste_buffers.len() {
                        let text = app.paste_buffers[selected].clone();
                        app.mode = Mode::Passthrough;
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = write!(p.writer, "{}", text);
                        }
                    } else {
                        app.mode = Mode::Passthrough;
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    // Delete selected buffer
                    if selected < app.paste_buffers.len() {
                        app.paste_buffers.remove(selected);
                        if let Mode::BufferChooser { selected: s } = &mut app.mode {
                            if *s >= app.paste_buffers.len() && *s > 0 { *s -= 1; }
                        }
                        if app.paste_buffers.is_empty() { app.mode = Mode::Passthrough; }
                    }
                }
                _ => {}
            }
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
    if let Some(ni) = find_best_pane_in_direction(&rects, ai, arect, dir) {
        win.active_path = rects[ni].0.clone();
    }
}

/// Spatial pane navigation: find the best pane in the given direction.
/// Prefers panes that overlap on the perpendicular axis (visually adjacent),
/// then picks the closest by primary-axis gap, tie-broken by perpendicular
/// center-distance for intuitive navigation in asymmetric layouts.
pub fn find_best_pane_in_direction(
    rects: &[(Vec<usize>, Rect)],
    ai: usize,
    arect: &Rect,
    dir: FocusDir,
) -> Option<usize> {
    // Center of the active pane (scaled by 2 to avoid fractional math)
    let acx = arect.x as i32 * 2 + arect.width as i32;
    let acy = arect.y as i32 * 2 + arect.height as i32;

    // Check whether two 1-D ranges [a_start, a_start+a_len) and [b_start, b_start+b_len) overlap
    let ranges_overlap = |a_start: u16, a_len: u16, b_start: u16, b_len: u16| -> bool {
        let a_end = a_start + a_len;
        let b_end = b_start + b_len;
        a_start < b_end && b_start < a_end
    };

    // (index, primary_gap, perp_center_dist, has_perp_overlap)
    let mut best: Option<(usize, u32, i32, bool)> = None;

    for (i, (_, r)) in rects.iter().enumerate() {
        if i == ai { continue; }
        // Primary-axis gap: the pane must be in the correct direction
        let (primary_gap, perp_overlap) = match dir {
            FocusDir::Left => {
                if r.x + r.width > arect.x { continue; }
                let gap = (arect.x - (r.x + r.width)) as u32;
                let overlap = ranges_overlap(r.y, r.height, arect.y, arect.height);
                (gap, overlap)
            }
            FocusDir::Right => {
                if r.x < arect.x + arect.width { continue; }
                let gap = (r.x - (arect.x + arect.width)) as u32;
                let overlap = ranges_overlap(r.y, r.height, arect.y, arect.height);
                (gap, overlap)
            }
            FocusDir::Up => {
                if r.y + r.height > arect.y { continue; }
                let gap = (arect.y - (r.y + r.height)) as u32;
                let overlap = ranges_overlap(r.x, r.width, arect.x, arect.width);
                (gap, overlap)
            }
            FocusDir::Down => {
                if r.y < arect.y + arect.height { continue; }
                let gap = (r.y - (arect.y + arect.height)) as u32;
                let overlap = ranges_overlap(r.x, r.width, arect.x, arect.width);
                (gap, overlap)
            }
        };

        // Perpendicular center distance (how far off-center the candidate is)
        let rcx = r.x as i32 * 2 + r.width as i32;
        let rcy = r.y as i32 * 2 + r.height as i32;
        let perp_dist = match dir {
            FocusDir::Left | FocusDir::Right => (rcy - acy).abs(),
            FocusDir::Up | FocusDir::Down => (rcx - acx).abs(),
        };

        let dominated = if let Some((_, bg, bd, bo)) = best {
            // Prefer: (1) perp-overlapping over non-overlapping,
            //         (2) smaller primary gap, (3) smaller perp distance
            if perp_overlap && !bo {
                false  // new candidate has overlap, current best doesn't → new wins
            } else if !perp_overlap && bo {
                true   // current best has overlap, new doesn't → new loses
            } else if primary_gap < bg {
                false  // closer on primary axis
            } else if primary_gap > bg {
                true   // farther on primary axis
            } else {
                perp_dist >= bd  // same gap → pick closer center
            }
        } else {
            false  // no best yet
        };

        if !dominated {
            best = Some((i, primary_gap, perp_dist, perp_overlap));
        }
    }

    best.map(|(idx, _, _, _)| idx)
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
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
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
                Node::Leaf(p) if !p.dead => { let _ = p.writer.write_all(data); let _ = p.writer.flush(); }
                Node::Leaf(_) => {}
                Node::Split { children, .. } => { for c in children { write_all_panes(c, data); } }
            }
        }
        write_all_panes(&mut win.root, &encoded);
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            if !active.dead {
                let _ = active.writer.write_all(&encoded);
                let _ = active.writer.flush();
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
                    switch_with_copy_save(app, |app| {
                        app.last_window_idx = app.active_idx;
                        app.active_idx = win_idx;
                    });
                }
                return Ok(());
            }
        }
        // Click was on status bar but not on a tab — ignore
        return Ok(());
    }

    // If a left-click lands on a different pane while in copy mode, save
    // copy state to the current pane and restore from the new pane (tmux parity #43).
    if matches!(me.kind, crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left))
        && matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. })
    {
        let win = &app.windows[app.active_idx];
        let mut rects_check: Vec<(Vec<usize>, Rect)> = Vec::new();
        compute_rects(&win.root, window_area, &mut rects_check);
        let mut clicked_new_path: Option<Vec<usize>> = None;
        for (path, area) in rects_check.iter() {
            if area.contains(ratatui::layout::Position { x: me.column, y: me.row }) {
                if *path != win.active_path {
                    clicked_new_path = Some(path.clone());
                }
                break;
            }
        }
        if let Some(np) = clicked_new_path {
            // Save copy state to current pane, reset its scrollback
            save_copy_state_to_pane(app);
            // Switch active pane path
            {
                let win = &mut app.windows[app.active_idx];
                app.last_pane_path = win.active_path.clone();
                win.active_path = np;
            }
            // Restore from new pane (likely Passthrough if it wasn't in copy mode)
            restore_copy_state_from_pane(app);
        }
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

            // Forward left-click to child pane
            if !on_border {
                if let Some(area) = active_area {
                    if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                        let col = me.column.saturating_sub(area.x + 1) as i16;
                        let row = me.row.saturating_sub(area.y + 1) as i16;
                        // Check if child has requested VT mouse protocol
                        let (mode, enc) = {
                            if let Ok(parser) = active.term.lock() {
                                let s = parser.screen();
                                (s.mouse_protocol_mode(), s.mouse_protocol_encoding())
                            } else {
                                (vt100::MouseProtocolMode::None, vt100::MouseProtocolEncoding::Default)
                            }
                        };
                        if mode != vt100::MouseProtocolMode::None {
                            let vt_col = (col + 1).max(1) as u16;
                            let vt_row = (row + 1).max(1) as u16;
                            write_mouse_event(&mut active.writer, 0, vt_col, vt_row, true, enc);
                        } else {
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
                // Forward mouse release
                if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                    let col = me.column.saturating_sub(area.x + 1) as i16;
                    let row = me.row.saturating_sub(area.y + 1) as i16;
                    let (mode, enc) = {
                        if let Ok(parser) = active.term.lock() {
                            let s = parser.screen();
                            (s.mouse_protocol_mode(), s.mouse_protocol_encoding())
                        } else {
                            (vt100::MouseProtocolMode::None, vt100::MouseProtocolEncoding::Default)
                        }
                    };
                    if mode != vt100::MouseProtocolMode::None {
                        let vt_col = (col + 1).max(1) as u16;
                        let vt_row = (row + 1).max(1) as u16;
                        write_mouse_event(&mut active.writer, 0, vt_col, vt_row, false, enc);
                    } else if let Some(pid) = active.child_pid {
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
                // Forward drag to child pane
                if let Some(area) = active_area {
                    if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                        let col = me.column.saturating_sub(area.x + 1) as i16;
                        let row = me.row.saturating_sub(area.y + 1) as i16;
                        let (mode, enc) = {
                            if let Ok(parser) = active.term.lock() {
                                let s = parser.screen();
                                (s.mouse_protocol_mode(), s.mouse_protocol_encoding())
                            } else {
                                (vt100::MouseProtocolMode::None, vt100::MouseProtocolEncoding::Default)
                            }
                        };
                        if mode != vt100::MouseProtocolMode::None {
                            let vt_col = (col + 1).max(1) as u16;
                            let vt_row = (row + 1).max(1) as u16;
                            // button 0 + 32 = drag modifier
                            write_mouse_event(&mut active.writer, 32, vt_col, vt_row, true, enc);
                        } else {
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
                let _ = write!(active.writer, "\x1b[<64;{};{}M", col, row);
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
                let _ = write!(active.writer, "\x1b[<65;{};{}M", col, row);
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
                Node::Leaf(p) => { let _ = p.writer.write_all(text); let _ = p.writer.flush(); }
                Node::Split { children, .. } => { for c in children { write_all_panes(c, text); } }
            }
        }
        write_all_panes(&mut win.root, text.as_bytes());
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = p.writer.write_all(text.as_bytes());
            let _ = p.writer.flush();
        }
    }
    Ok(())
}

/// Dispatch a single character as a copy-mode action.
fn handle_copy_mode_char(app: &mut AppState, c: char) -> io::Result<()> {
    // Handle text-object pending state (waiting for w/W after a/i)
    if let Some(prefix) = app.copy_text_object_pending.take() {
        match (prefix, c) {
            (0, 'w') => { crate::copy_mode::select_a_word(app); }
            (1, 'w') => { crate::copy_mode::select_inner_word(app); }
            (0, 'W') => { crate::copy_mode::select_a_word_big(app); }
            (1, 'W') => { crate::copy_mode::select_inner_word_big(app); }
            _ => {}
        }
        return Ok(());
    }
    // Handle find-char pending state (waiting for char after f/F/t/T)
    if let Some(pending) = app.copy_find_char_pending.take() {
        match pending {
            0 => crate::copy_mode::find_char_forward(app, c),
            1 => crate::copy_mode::find_char_backward(app, c),
            2 => crate::copy_mode::find_char_to_forward(app, c),
            3 => crate::copy_mode::find_char_to_backward(app, c),
            _ => {}
        }
        return Ok(());
    }
    match c {
        'q' | ']' | '\x1b' => {
            exit_copy_mode(app);
        }
        'h' => { move_copy_cursor(app, -1, 0); }
        'l' => { move_copy_cursor(app, 1, 0); }
        'k' => { move_copy_cursor(app, 0, -1); }
        'j' => { move_copy_cursor(app, 0, 1); }
        'g' => { scroll_to_top(app); }
        'G' => { scroll_to_bottom(app); }
        'w' => { crate::copy_mode::move_word_forward(app); }
        'b' => { crate::copy_mode::move_word_backward(app); }
        'e' => { crate::copy_mode::move_word_end(app); }
        'W' => { crate::copy_mode::move_word_forward_big(app); }
        'B' => { crate::copy_mode::move_word_backward_big(app); }
        'E' => { crate::copy_mode::move_word_end_big(app); }
        'H' => { crate::copy_mode::move_to_screen_top(app); }
        'M' => { crate::copy_mode::move_to_screen_middle(app); }
        'L' => { crate::copy_mode::move_to_screen_bottom(app); }
        'f' => { app.copy_find_char_pending = Some(0); }
        'F' => { app.copy_find_char_pending = Some(1); }
        't' => { app.copy_find_char_pending = Some(2); }
        'T' => { app.copy_find_char_pending = Some(3); }
        'D' => { crate::copy_mode::copy_end_of_line(app)?; exit_copy_mode(app); }
        '0' => { crate::copy_mode::move_to_line_start(app); }
        '$' => { crate::copy_mode::move_to_line_end(app); }
        '^' => { crate::copy_mode::move_to_first_nonblank(app); }
        ' ' => {
            if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some((r, c));
                app.copy_selection_mode = crate::types::SelectionMode::Char;
            }
        }
        'v' => {
            if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some((r, c));
                app.copy_selection_mode = crate::types::SelectionMode::Char;
            }
        }
        'V' => {
            if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some((r, c));
                app.copy_selection_mode = crate::types::SelectionMode::Line;
            }
        }
        'o' => {
            if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                app.copy_anchor = Some(p);
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some(a);
            }
        }
        'A' => {
            if let (Some(_), Some(_)) = (app.copy_anchor, app.copy_pos) {
                let prev = app.paste_buffers.first().cloned().unwrap_or_default();
                yank_selection(app)?;
                if let Some(buf) = app.paste_buffers.first_mut() {
                    let new_text = buf.clone();
                    *buf = format!("{}{}", prev, new_text);
                }
                exit_copy_mode(app);
            }
        }
        'y' => { yank_selection(app)?; exit_copy_mode(app); }
        '/' => { app.mode = Mode::CopySearch { input: String::new(), forward: true }; }
        '?' => { app.mode = Mode::CopySearch { input: String::new(), forward: false }; }
        'n' => { search_next(app); }
        'N' => { search_prev(app); }
        'i' => { app.copy_text_object_pending = Some(1); }  // inner text object
        'a' => { app.copy_text_object_pending = Some(0); }  // a text object
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
                exit_copy_mode(app);
            }
            "enter" => {
                // Copy selection and exit copy mode (vi Enter)
                if app.copy_anchor.is_some() {
                    yank_selection(app)?;
                }
                exit_copy_mode(app);
            }
            "space" => {
                // Begin selection (like v in vi mode)
                if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                    app.copy_anchor = Some((r, c));
                    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                    app.copy_pos = Some((r, c));
                    app.copy_selection_mode = crate::types::SelectionMode::Char;
                }
            }
            "up" => { move_copy_cursor(app, 0, -1); }
            "down" => { move_copy_cursor(app, 0, 1); }
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
            "C-n" | "c-n" => { move_copy_cursor(app, 0, 1); }
            "C-p" | "c-p" => { move_copy_cursor(app, 0, -1); }
            "C-a" | "c-a" => { crate::copy_mode::move_to_line_start(app); }
            "C-e" | "c-e" => { crate::copy_mode::move_to_line_end(app); }
            "C-v" | "c-v" => { scroll_copy_down(app, 10); }
            "M-v" | "m-v" => { scroll_copy_up(app, 10); }
            "M-f" | "m-f" => { crate::copy_mode::move_word_forward(app); }
            "M-b" | "m-b" => { crate::copy_mode::move_word_backward(app); }
            "M-w" | "m-w" => { yank_selection(app)?; exit_copy_mode(app); }
            "C-s" | "c-s" => { app.mode = Mode::CopySearch { input: String::new(), forward: true }; }
            "C-r" | "c-r" => { app.mode = Mode::CopySearch { input: String::new(), forward: false }; }
            "C-c" | "c-c" => {
                exit_copy_mode(app);
            }
            "C-g" | "c-g" => {
                exit_copy_mode(app);
            }
            "c-space" | "C-space" => {
                // Set mark (anchor) at current position
                if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                    app.copy_anchor = Some((r, c));
                    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
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
            "enter" => { let _ = write!(p.writer, "\r"); }
            "tab" => { let _ = write!(p.writer, "\t"); }
            "btab" | "backtab" => { let _ = write!(p.writer, "\x1b[Z"); }
            "backspace" => { let _ = p.writer.write_all(&[0x7F]); }
            "delete" => { let _ = write!(p.writer, "\x1b[3~"); }
            "esc" => { let _ = write!(p.writer, "\x1b"); }
            "left" => { let _ = write!(p.writer, "\x1b[D"); }
            "right" => { let _ = write!(p.writer, "\x1b[C"); }
            "up" => { let _ = write!(p.writer, "\x1b[A"); }
            "down" => { let _ = write!(p.writer, "\x1b[B"); }
            "pageup" => { let _ = write!(p.writer, "\x1b[5~"); }
            "pagedown" => { let _ = write!(p.writer, "\x1b[6~"); }
            "home" => { let _ = write!(p.writer, "\x1b[H"); }
            "end" => { let _ = write!(p.writer, "\x1b[F"); }
            "insert" => { let _ = write!(p.writer, "\x1b[2~"); }
            "space" => { let _ = write!(p.writer, " "); }
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
                    if !seq.is_empty() { let _ = write!(p.writer, "{}", seq); }
                }
            }
            s if s.starts_with("C-") && s.len() == 3 => {
                let c = s.chars().nth(2).unwrap_or('c');
                let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
                let _ = p.writer.write_all(&[ctrl_char]);
            }
            s if (s.starts_with("M-") || s.starts_with("m-")) && s.len() == 3 => {
                let c = s.chars().nth(2).unwrap_or('a');
                let _ = write!(p.writer, "\x1b{}", c);
            }
            s if (s.starts_with("C-M-") || s.starts_with("c-m-")) && s.len() == 5 => {
                let c = s.chars().nth(4).unwrap_or('c');
                let ctrl_char = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a' - 1);
                let _ = p.writer.write_all(&[0x1b, ctrl_char]);
            }
            _ => {}
        }
        let _ = p.writer.flush();
    }
    Ok(())
}
