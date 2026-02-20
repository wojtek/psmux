use std::env;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::types::*;
use crate::commands::parse_command_to_action;

pub fn load_config(app: &mut AppState) {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let paths = vec![
        format!("{}\\.psmux.conf", home),
        format!("{}\\.psmuxrc", home),
        format!("{}\\.tmux.conf", home),
        format!("{}\\.config\\psmux\\psmux.conf", home),
    ];
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            parse_config_content(app, &content);
            break;
        }
    }
}

pub fn parse_config_content(app: &mut AppState, content: &str) {
    for line in content.lines() {
        parse_config_line(app, line);
    }
}

pub fn parse_config_line(app: &mut AppState, line: &str) {
    let l = line.trim();
    if l.is_empty() || l.starts_with('#') { return; }
    
    let l = if l.ends_with('\\') {
        l.trim_end_matches('\\').trim()
    } else {
        l
    };
    
    if l.starts_with("set-option ") || l.starts_with("set ") {
        parse_set_option(app, l);
    }
    else if l.starts_with("set -g ") {
        let rest = &l[7..];
        parse_option_value(app, rest, true);
    }
    else if l.starts_with("setw ") || l.starts_with("set-window-option ") {
        // setw maps to the same option parser (tmux window options overlap)
        parse_set_option(app, l);
    }
    else if l.starts_with("bind-key ") || l.starts_with("bind ") {
        parse_bind_key(app, l);
    }
    else if l.starts_with("unbind-key ") || l.starts_with("unbind ") {
        parse_unbind_key(app, l);
    }
    else if l.starts_with("source-file ") || l.starts_with("source ") {
        let parts: Vec<&str> = l.splitn(2, ' ').collect();
        if parts.len() > 1 {
            source_file(app, parts[1].trim());
        }
    }
    else if l.starts_with("run-shell ") || l.starts_with("run ") {
        parse_run_shell(app, l);
    }
    else if l.starts_with("if-shell ") || l.starts_with("if ") {
        parse_if_shell(app, l);
    }
    else if l.starts_with("set-hook ") {
        // Parse set-hook: set-hook [-g] hook-name command
        let parts: Vec<&str> = l.split_whitespace().collect();
        let mut i = 1;
        while i < parts.len() && parts[i].starts_with('-') { i += 1; }
        if i + 1 < parts.len() {
            let hook = parts[i].to_string();
            let cmd = parts[i+1..].join(" ");
            app.hooks.entry(hook).or_insert_with(Vec::new).push(cmd);
        }
    }
    else if l.starts_with("set-environment ") || l.starts_with("setenv ") {
        let parts: Vec<&str> = l.split_whitespace().collect();
        let mut i = 1;
        while i < parts.len() && parts[i].starts_with('-') { i += 1; }
        if i + 1 < parts.len() {
            app.environment.insert(parts[i].to_string(), parts[i+1..].join(" "));
        }
    }
}

fn parse_set_option(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 { return; }
    
    let mut i = 1;
    let mut is_global = false;
    
    while i < parts.len() {
        let p = parts[i];
        if p.starts_with('-') {
            if p.contains('g') { is_global = true; }
            i += 1;
            if p.contains('t') && i < parts.len() { i += 1; }
        } else {
            break;
        }
    }
    
    if i < parts.len() {
        let rest = parts[i..].join(" ");
        parse_option_value(app, &rest, is_global);
    }
}

pub fn parse_option_value(app: &mut AppState, rest: &str, _is_global: bool) {
    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
    if parts.is_empty() { return; }
    
    let key = parts[0].trim();
    let value = if parts.len() > 1 { 
        parts[1].trim().trim_matches('"').trim_matches('\'')
    } else { 
        "" 
    };
    
    match key {
        "status-left" => app.status_left = value.to_string(),
        "status-right" => app.status_right = value.to_string(),
        "mouse" => app.mouse_enabled = matches!(value, "on" | "true" | "1"),
        "prefix" => {
            if let Some(key) = parse_key_name(value) {
                app.prefix_key = key;
            }
        }
        "prefix2" => { app.environment.insert(key.to_string(), value.to_string()); }
        "escape-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.escape_time_ms = ms;
            }
        }
        "prediction-dimming" | "dim-predictions" => {
            app.prediction_dimming = !matches!(value, "off" | "false" | "0");
        }
        "cursor-style" => env::set_var("PSMUX_CURSOR_STYLE", value),
        "cursor-blink" => env::set_var("PSMUX_CURSOR_BLINK", if matches!(value, "on"|"true"|"1") { "1" } else { "0" }),
        "status" => {
            app.status_visible = matches!(value, "on" | "true" | "1" | "2");
        }
        "status-style" => {
            app.status_style = value.to_string();
        }
        "status-position" => {
            app.status_position = value.to_string();
        }
        "status-interval" => {
            if let Ok(n) = value.parse::<u64>() { app.status_interval = n; }
        }
        "status-justify" => { app.status_justify = value.to_string(); }
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
        "default-command" | "default-shell" => {
            app.default_shell = value.to_string();
        }
        "word-separators" => {
            app.word_separators = value.to_string();
        }
        "renumber-windows" => {
            app.renumber_windows = matches!(value, "on" | "true" | "1");
        }
        "mode-keys" => {
            app.mode_keys = value.to_string();
        }
        "focus-events" => {
            app.focus_events = matches!(value, "on" | "true" | "1");
        }
        "monitor-activity" => {
            app.monitor_activity = matches!(value, "on" | "true" | "1");
        }
        "visual-activity" => {
            app.visual_activity = matches!(value, "on" | "true" | "1");
        }
        "remain-on-exit" => {
            app.remain_on_exit = matches!(value, "on" | "true" | "1");
        }
        "aggressive-resize" => {
            app.aggressive_resize = matches!(value, "on" | "true" | "1");
        }
        "set-titles" => {
            app.set_titles = matches!(value, "on" | "true" | "1");
        }
        "set-titles-string" => {
            app.set_titles_string = value.to_string();
        }
        "status-keys" => { app.environment.insert(key.to_string(), value.to_string()); }
        "pane-border-style" => { app.pane_border_style = value.to_string(); }
        "pane-active-border-style" => { app.pane_active_border_style = value.to_string(); }
        "window-status-format" => { app.window_status_format = value.to_string(); }
        "window-status-current-format" => { app.window_status_current_format = value.to_string(); }
        "window-status-separator" => { app.window_status_separator = value.to_string(); }
        "automatic-rename" => {
            app.automatic_rename = matches!(value, "on" | "true" | "1");
        }
        "synchronize-panes" => {
            app.sync_input = matches!(value, "on" | "true" | "1");
        }
        "allow-rename" => { app.environment.insert(key.to_string(), value.to_string()); }
        "terminal-overrides" => { app.environment.insert(key.to_string(), value.to_string()); }
        "default-terminal" => { app.environment.insert(key.to_string(), value.to_string()); }
        "update-environment" => { app.environment.insert(key.to_string(), value.to_string()); }
        "bell-action" => { app.bell_action = value.to_string(); }
        "visual-bell" => { app.visual_bell = matches!(value, "on" | "true" | "1"); }
        "activity-action" => { app.environment.insert(key.to_string(), value.to_string()); }
        "silence-action" => { app.environment.insert(key.to_string(), value.to_string()); }
        "monitor-silence" => {
            if let Ok(n) = value.parse::<u64>() { app.monitor_silence = n; }
        }
        "message-style" => { app.message_style = value.to_string(); }
        "message-command-style" => { app.message_command_style = value.to_string(); }
        "mode-style" => { app.mode_style = value.to_string(); }
        "window-status-style" => { app.window_status_style = value.to_string(); }
        "window-status-current-style" => { app.window_status_current_style = value.to_string(); }
        "window-status-activity-style" => { app.window_status_activity_style = value.to_string(); }
        "window-status-bell-style" => { app.window_status_bell_style = value.to_string(); }
        "window-status-last-style" => { app.window_status_last_style = value.to_string(); }
        "status-left-style" => { app.status_left_style = value.to_string(); }
        "status-right-style" => { app.status_right_style = value.to_string(); }
        "clock-mode-colour" | "clock-mode-style" => { app.environment.insert(key.to_string(), value.to_string()); }
        "pane-border-format" | "pane-border-status" => { app.environment.insert(key.to_string(), value.to_string()); }
        "popup-style" | "popup-border-style" | "popup-border-lines" => { app.environment.insert(key.to_string(), value.to_string()); }
        "window-style" | "window-active-style" => { app.environment.insert(key.to_string(), value.to_string()); }
        "wrap-search" => { app.environment.insert(key.to_string(), value.to_string()); }
        "lock-after-time" | "lock-command" => { app.environment.insert(key.to_string(), value.to_string()); }
        _ => {
            // Store any unknown option in the environment map for plugin compat
            app.environment.insert(key.to_string(), value.to_string());
        }
    }
}

/// Split a bind-key command string on `\;` or bare `;` to produce sub-commands.
/// Handles: `split-window \; select-pane -D` → ["split-window", "select-pane -D"]
pub fn split_chained_commands_pub(command: &str) -> Vec<String> {
    split_chained_commands(command)
}
fn split_chained_commands(command: &str) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    let mut current = String::new();
    let tokens: Vec<&str> = command.split_whitespace().collect();
    
    for token in &tokens {
        if *token == "\\;" || *token == ";" {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                commands.push(trimmed);
            }
            current.clear();
        } else {
            if !current.is_empty() { current.push(' '); }
            current.push_str(token);
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        commands.push(trimmed);
    }
    commands
}

pub fn parse_bind_key(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 { return; }
    
    let mut i = 1;
    let mut _key_table = "prefix".to_string();
    let mut _repeatable = false;
    
    while i < parts.len() {
        let p = parts[i];
        // A flag must start with '-' AND be longer than 1 char (e.g. "-r", "-n", "-T").
        // A bare "-" is a valid key name, not a flag.
        if p.starts_with('-') && p.len() > 1 {
            if p.contains('r') { _repeatable = true; }
            if p.contains('n') { _key_table = "root".to_string(); }
            if p.contains('T') {
                i += 1;
                if i < parts.len() { _key_table = parts[i].to_string(); }
            }
            i += 1;
        } else {
            break;
        }
    }
    
    if i >= parts.len() { return; }
    let key_str = parts[i];
    i += 1;
    
    if i >= parts.len() { return; }
    let command = parts[i..].join(" ");
    
    // Split on `\;` or `;` to support command chaining (like tmux `bind x split-window \; select-pane -D`)
    let sub_commands: Vec<String> = split_chained_commands(&command);
    
    if let Some(key) = parse_key_name(key_str) {
        let key = normalize_key_for_binding(key);
        let action = if sub_commands.len() > 1 {
            // Multiple chained commands
            Action::CommandChain(sub_commands)
        } else if let Some(a) = parse_command_to_action(&command) {
            a
        } else {
            return;
        };
        let table = app.key_tables.entry(_key_table).or_default();
        table.retain(|b| b.key != key);
        table.push(Bind { key, action, repeat: _repeatable });
    }
}

pub fn parse_unbind_key(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 { return; }
    
    let mut i = 1;
    let mut unbind_all = false;
    
    while i < parts.len() {
        let p = parts[i];
        if p.starts_with('-') {
            if p.contains('a') { unbind_all = true; }
            if p.contains('T') { i += 1; }
            i += 1;
        } else {
            break;
        }
    }
    
    if unbind_all {
        app.key_tables.clear();
        return;
    }
    
    if i < parts.len() {
        if let Some(key) = parse_key_name(parts[i]) {
            let key = normalize_key_for_binding(key);
            // Remove from all tables
            for table in app.key_tables.values_mut() {
                table.retain(|b| b.key != key);
            }
        }
    }
}

/// Normalize a key tuple for binding comparison.
/// Strips SHIFT from Char events since the character itself encodes shift information.
/// e.g., '|' already implies Shift was pressed, so (Char('|'), SHIFT) and (Char('|'), NONE) should match.
pub fn normalize_key_for_binding(key: (KeyCode, KeyModifiers)) -> (KeyCode, KeyModifiers) {
    match key.0 {
        KeyCode::Char(_) => (key.0, key.1.difference(KeyModifiers::SHIFT)),
        _ => key,
    }
}

pub fn parse_key_name(name: &str) -> Option<(KeyCode, KeyModifiers)> {
    let name = name.trim();
    
    if name.starts_with("C-") || name.starts_with("^") {
        let ch = if name.starts_with("C-") {
            name.chars().nth(2)
        } else {
            name.chars().nth(1)
        };
        if let Some(c) = ch {
            return Some((KeyCode::Char(c.to_ascii_lowercase()), KeyModifiers::CONTROL));
        }
    }
    
    if name.starts_with("M-") {
        if let Some(c) = name.chars().nth(2) {
            return Some((KeyCode::Char(c.to_ascii_lowercase()), KeyModifiers::ALT));
        }
    }
    
    if name.starts_with("S-") {
        let rest = &name[2..];
        if rest.eq_ignore_ascii_case("Tab") {
            return Some((KeyCode::BackTab, KeyModifiers::NONE));
        }
        if let Some(c) = rest.chars().next() {
            if rest.len() == 1 {
                return Some((KeyCode::Char(c.to_ascii_uppercase()), KeyModifiers::SHIFT));
            }
        }
    }
    
    match name.to_uppercase().as_str() {
        "ENTER" => return Some((KeyCode::Enter, KeyModifiers::NONE)),
        "TAB" => return Some((KeyCode::Tab, KeyModifiers::NONE)),
        "BTAB" => return Some((KeyCode::BackTab, KeyModifiers::NONE)),
        "ESCAPE" | "ESC" => return Some((KeyCode::Esc, KeyModifiers::NONE)),
        "SPACE" => return Some((KeyCode::Char(' '), KeyModifiers::NONE)),
        "BSPACE" | "BACKSPACE" => return Some((KeyCode::Backspace, KeyModifiers::NONE)),
        "UP" => return Some((KeyCode::Up, KeyModifiers::NONE)),
        "DOWN" => return Some((KeyCode::Down, KeyModifiers::NONE)),
        "LEFT" => return Some((KeyCode::Left, KeyModifiers::NONE)),
        "RIGHT" => return Some((KeyCode::Right, KeyModifiers::NONE)),
        "HOME" => return Some((KeyCode::Home, KeyModifiers::NONE)),
        "END" => return Some((KeyCode::End, KeyModifiers::NONE)),
        "PAGEUP" | "PPAGE" | "PGUP" => return Some((KeyCode::PageUp, KeyModifiers::NONE)),
        "PAGEDOWN" | "NPAGE" | "PGDN" => return Some((KeyCode::PageDown, KeyModifiers::NONE)),
        "INSERT" | "IC" => return Some((KeyCode::Insert, KeyModifiers::NONE)),
        "DELETE" | "DC" => return Some((KeyCode::Delete, KeyModifiers::NONE)),
        "F1" => return Some((KeyCode::F(1), KeyModifiers::NONE)),
        "F2" => return Some((KeyCode::F(2), KeyModifiers::NONE)),
        "F3" => return Some((KeyCode::F(3), KeyModifiers::NONE)),
        "F4" => return Some((KeyCode::F(4), KeyModifiers::NONE)),
        "F5" => return Some((KeyCode::F(5), KeyModifiers::NONE)),
        "F6" => return Some((KeyCode::F(6), KeyModifiers::NONE)),
        "F7" => return Some((KeyCode::F(7), KeyModifiers::NONE)),
        "F8" => return Some((KeyCode::F(8), KeyModifiers::NONE)),
        "F9" => return Some((KeyCode::F(9), KeyModifiers::NONE)),
        "F10" => return Some((KeyCode::F(10), KeyModifiers::NONE)),
        "F11" => return Some((KeyCode::F(11), KeyModifiers::NONE)),
        "F12" => return Some((KeyCode::F(12), KeyModifiers::NONE)),
        _ => {}
    }
    
    if name.len() == 1 {
        if let Some(c) = name.chars().next() {
            return Some((KeyCode::Char(c), KeyModifiers::NONE));
        }
    }
    
    None
}

pub fn source_file(app: &mut AppState, path: &str) {
    let path = path.trim().trim_matches('"').trim_matches('\'');
    let expanded = if path.starts_with('~') {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    };
    
    if let Ok(content) = std::fs::read_to_string(&expanded) {
        parse_config_content(app, &content);
    }
}

/// Parse a key string like "C-a", "M-x", "F1", "Space" into (KeyCode, KeyModifiers)
pub fn parse_key_string(key: &str) -> Option<(KeyCode, KeyModifiers)> {
    let key = key.trim();
    let mut mods = KeyModifiers::empty();
    let mut key_part = key;
    
    while key_part.len() > 2 {
        if key_part.starts_with("C-") || key_part.starts_with("c-") {
            mods |= KeyModifiers::CONTROL;
            key_part = &key_part[2..];
        } else if key_part.starts_with("M-") || key_part.starts_with("m-") {
            mods |= KeyModifiers::ALT;
            key_part = &key_part[2..];
        } else if key_part.starts_with("S-") || key_part.starts_with("s-") {
            mods |= KeyModifiers::SHIFT;
            key_part = &key_part[2..];
        } else {
            break;
        }
    }
    
    let keycode = match key_part.to_lowercase().as_str() {
        "a" => KeyCode::Char('a'),
        "b" => KeyCode::Char('b'),
        "c" => KeyCode::Char('c'),
        "d" => KeyCode::Char('d'),
        "e" => KeyCode::Char('e'),
        "f" => KeyCode::Char('f'),
        "g" => KeyCode::Char('g'),
        "h" => KeyCode::Char('h'),
        "i" => KeyCode::Char('i'),
        "j" => KeyCode::Char('j'),
        "k" => KeyCode::Char('k'),
        "l" => KeyCode::Char('l'),
        "m" => KeyCode::Char('m'),
        "n" => KeyCode::Char('n'),
        "o" => KeyCode::Char('o'),
        "p" => KeyCode::Char('p'),
        "q" => KeyCode::Char('q'),
        "r" => KeyCode::Char('r'),
        "s" => KeyCode::Char('s'),
        "t" => KeyCode::Char('t'),
        "u" => KeyCode::Char('u'),
        "v" => KeyCode::Char('v'),
        "w" => KeyCode::Char('w'),
        "x" => KeyCode::Char('x'),
        "y" => KeyCode::Char('y'),
        "z" => KeyCode::Char('z'),
        "0" => KeyCode::Char('0'),
        "1" => KeyCode::Char('1'),
        "2" => KeyCode::Char('2'),
        "3" => KeyCode::Char('3'),
        "4" => KeyCode::Char('4'),
        "5" => KeyCode::Char('5'),
        "6" => KeyCode::Char('6'),
        "7" => KeyCode::Char('7'),
        "8" => KeyCode::Char('8'),
        "9" => KeyCode::Char('9'),
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "btab" | "backtab" => KeyCode::BackTab,
        "escape" | "esc" => KeyCode::Esc,
        "backspace" | "bspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "ppage" => KeyCode::PageUp,
        "pagedown" | "npage" => KeyCode::PageDown,
        "insert" | "ic" => KeyCode::Insert,
        "delete" | "dc" => KeyCode::Delete,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        "\"" => KeyCode::Char('"'),
        "%" => KeyCode::Char('%'),
        "," => KeyCode::Char(','),
        "." => KeyCode::Char('.'),
        ":" => KeyCode::Char(':'),
        ";" => KeyCode::Char(';'),
        "[" => KeyCode::Char('['),
        "]" => KeyCode::Char(']'),
        "{" => KeyCode::Char('{'),
        "}" => KeyCode::Char('}'),
        _ => {
            if key_part.len() == 1 {
                KeyCode::Char(key_part.chars().next().unwrap())
            } else {
                return None;
            }
        }
    };
    
    Some((keycode, mods))
}

/// Format a key binding back to string representation
pub fn format_key_binding(key: &(KeyCode, KeyModifiers)) -> String {
    let (keycode, mods) = key;
    let mut result = String::new();
    
    if mods.contains(KeyModifiers::CONTROL) {
        result.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        result.push_str("M-");
    }
    if mods.contains(KeyModifiers::SHIFT) {
        result.push_str("S-");
    }
    
    let key_str = match keycode {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BTab".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Backspace => "BSpace".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PPage".to_string(),
        KeyCode::PageDown => "NPage".to_string(),
        KeyCode::Insert => "IC".to_string(),
        KeyCode::Delete => "DC".to_string(),
        KeyCode::F(n) => format!("F{}", n),
        _ => "?".to_string(),
    };
    
    result.push_str(&key_str);
    result
}

/// Execute a run-shell / run command from config.
/// Syntax: run-shell [-b] <command>
/// Without -b, output is silently discarded (we're in config parsing).
/// With -b, the command runs in the background.
fn parse_run_shell(_app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 { return; }
    let mut background = false;
    let mut cmd_parts: Vec<&str> = Vec::new();
    for p in &parts[1..] {
        if *p == "-b" { background = true; }
        else { cmd_parts.push(p); }
    }
    let shell_cmd = cmd_parts.join(" ");
    // Strip surrounding quotes if present
    let shell_cmd = shell_cmd.trim_matches(|c| c == '\'' || c == '"');
    if shell_cmd.is_empty() { return; }

    if background {
        #[cfg(windows)]
        { let _ = std::process::Command::new("cmd").args(["/C", shell_cmd]).spawn(); }
        #[cfg(not(windows))]
        { let _ = std::process::Command::new("sh").args(["-c", shell_cmd]).spawn(); }
    } else {
        #[cfg(windows)]
        { let _ = std::process::Command::new("cmd").args(["/C", shell_cmd]).output(); }
        #[cfg(not(windows))]
        { let _ = std::process::Command::new("sh").args(["-c", shell_cmd]).output(); }
    }
}

/// Execute an if-shell / if command from config.
/// Syntax: if-shell [-bF] <condition> <true-cmd> [<false-cmd>]
/// Runs the condition command (or evaluates format with -F), then executes the
/// appropriate branch command as a config line.
fn parse_if_shell(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 { return; }

    let mut format_mode = false;
    let mut _background = false;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 1;
    while i < parts.len() {
        match parts[i] {
            "-b" => { _background = true; }
            "-F" => { format_mode = true; }
            "-bF" | "-Fb" => { _background = true; format_mode = true; }
            "-t" => { i += 1; } // skip target
            s => {
                // Handle quoted strings that might span multiple parts
                if s.starts_with('"') || s.starts_with('\'') {
                    let quote = s.chars().next().unwrap();
                    if s.ends_with(quote) && s.len() > 1 {
                        positional.push(s[1..s.len()-1].to_string());
                    } else {
                        let mut buf = s[1..].to_string();
                        i += 1;
                        while i < parts.len() {
                            buf.push(' ');
                            buf.push_str(parts[i]);
                            if parts[i].ends_with(quote) {
                                buf.truncate(buf.len() - 1);
                                break;
                            }
                            i += 1;
                        }
                        positional.push(buf);
                    }
                } else {
                    positional.push(s.to_string());
                }
            }
        }
        i += 1;
    }

    if positional.len() < 2 { return; }
    let condition = &positional[0];
    let true_cmd = &positional[1];
    let false_cmd = positional.get(2);

    let success = if format_mode {
        !condition.is_empty() && condition != "0"
    } else {
        #[cfg(windows)]
        { std::process::Command::new("cmd").args(["/C", condition]).status().map(|s| s.success()).unwrap_or(false) }
        #[cfg(not(windows))]
        { std::process::Command::new("sh").args(["-c", condition]).status().map(|s| s.success()).unwrap_or(false) }
    };

    let cmd_to_run = if success { Some(true_cmd) } else { false_cmd };
    if let Some(cmd) = cmd_to_run {
        // Execute the branch as a config line (recursive — supports set, bind, source, etc.)
        parse_config_line(app, cmd);
    }
}
