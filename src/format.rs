use std::env;

use crate::types::*;
use crate::tree::*;
use crate::config::format_key_binding;

/// Expand tmux format strings in text for the active window.
/// Supports both #X shorthand and #{variable_name} syntax.
#[inline]
pub fn expand_format(fmt: &str, app: &AppState) -> String {
    expand_format_for_window(fmt, app, app.active_idx)
}

/// Expand tmux format strings for a specific window index.
pub fn expand_format_for_window(fmt: &str, app: &AppState, win_idx: usize) -> String {
    let mut result = String::with_capacity(fmt.len() * 2);
    let bytes = fmt.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'#' && i + 1 < len {
            if bytes[i + 1] == b'{' {
                // #{variable_name} or #{?cond,true,false} or #{=N:var} syntax
                // Find matching closing brace (handle nesting)
                if let Some(close) = find_matching_brace(fmt, i + 2) {
                    let var = &fmt[i+2..close];
                    if var.starts_with('?') {
                        // Conditional: #{?cond,true_branch,false_branch}
                        result.push_str(&expand_conditional(var, app, win_idx));
                    } else if var.starts_with('=') {
                        // Truncation: #{=N:var} - truncate to N chars
                        result.push_str(&expand_truncation(var, app, win_idx));
                    } else {
                        result.push_str(&expand_format_var_for_window(var, app, win_idx));
                    }
                    i = close + 1; // skip past }
                    continue;
                }
            }
            // Shorthand #X format
            match bytes[i + 1] {
                b'S' => { result.push_str(&app.session_name); i += 2; continue; }
                b'I' => {
                    let n = win_idx + app.window_base_index;
                    result.push_str(&n.to_string());
                    i += 2; continue;
                }
                b'W' | b'T' => { result.push_str(&app.windows[win_idx].name); i += 2; continue; }
                b'P' => {
                    let win = &app.windows[win_idx];
                    let pid = get_active_pane_id(&win.root, &win.active_path).unwrap_or(0);
                    result.push_str(&pid.to_string());
                    i += 2; continue;
                }
                b'F' => {
                    if win_idx == app.active_idx { result.push('*'); }
                    else if win_idx == app.last_window_idx { result.push('-'); }
                    i += 2; continue;
                }
                b'H' | b'h' => {
                    result.push_str(&hostname_cached());
                    i += 2; continue;
                }
                b'D' => {
                    result.push_str(&chrono::Local::now().format("%Y-%m-%d").to_string());
                    i += 2; continue;
                }
                b'#' => { result.push('#'); i += 2; continue; }
                _ => {}
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    // Expand strftime %-sequences (e.g. %H:%M, %d-%b-%y) via chrono.
    // Only apply if the ORIGINAL format string contained '%' — avoids corrupting
    // expanded variables like pane_id (%3) that happen to contain '%'.
    if fmt.contains('%') && result.contains('%') {
        result = chrono::Local::now().format(&result).to_string();
    }
    result
}

fn expand_format_var_for_window(var: &str, app: &AppState, win_idx: usize) -> String {
    let win = &app.windows[win_idx];
    match var {
        "session_name" => app.session_name.clone(),
        "session_attached" => if app.attached_clients > 0 { "1".into() } else { "0".into() },
        "session_windows" => app.windows.len().to_string(),
        "window_index" => (win_idx + app.window_base_index).to_string(),
        "window_name" => win.name.clone(),
        "window_active" => if win_idx == app.active_idx { "1".into() } else { "0".into() },
        "window_panes" => count_panes(&win.root).to_string(),
        "window_flags" => {
            let mut flags = String::new();
            if win_idx == app.active_idx { flags.push('*'); }
            else if win_idx == app.last_window_idx { flags.push('-'); }
            if win.activity_flag { flags.push('#'); }
            flags
        }
        "window_activity_flag" => if win.activity_flag { "1".into() } else { "0".into() },
        "pane_index" => {
            get_active_pane_id(&win.root, &win.active_path).unwrap_or(0).to_string()
        }
        "pane_title" => win.name.clone(),
        "pane_width" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) { p.last_cols.to_string() } else { "0".into() }
        }
        "pane_height" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) { p.last_rows.to_string() } else { "0".into() }
        }
        "pane_active" => "1".to_string(),
        "pane_current_command" => {
            // Try to get the running command from pane title inference
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if !p.title.is_empty() { p.title.clone() } else { "shell".into() }
            } else { String::new() }
        }
        "pane_current_path" => {
            // Best-effort: use pane title which often contains the cwd
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                p.title.clone()
            } else { String::new() }
        }
        "pane_pid" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                p.child_pid.map(|pid| pid.to_string()).unwrap_or_default()
            } else { String::new() }
        }
        "cursor_x" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if let Ok(parser) = p.term.lock() {
                    let (_, c) = parser.screen().cursor_position();
                    return c.to_string();
                }
            }
            "0".into()
        }
        "cursor_y" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if let Ok(parser) = p.term.lock() {
                    let (r, _) = parser.screen().cursor_position();
                    return r.to_string();
                }
            }
            "0".into()
        }
        "pane_in_mode" => {
            match app.mode {
                crate::types::Mode::CopyMode | crate::types::Mode::CopySearch { .. } | crate::types::Mode::ClockMode => "1".into(),
                _ => "0".into(),
            }
        }
        "pane_mode" => {
            match app.mode {
                crate::types::Mode::CopyMode | crate::types::Mode::CopySearch { .. } => "copy-mode".into(),
                crate::types::Mode::ClockMode => "clock-mode".into(),
                _ => "".into(),
            }
        }
        "pane_synchronized" => if app.sync_input { "1".into() } else { "0".into() },
        "pane_dead" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if p.dead { return "1".into(); }
            }
            "0".into()
        }
        "client_width" => app.last_window_area.width.to_string(),
        "client_height" => (app.last_window_area.height + if app.status_visible { 1 } else { 0 }).to_string(),
        "history_size" | "history_limit" => app.history_limit.to_string(),
        "alternate_on" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if let Ok(parser) = p.term.lock() {
                    if parser.screen().alternate_screen() { return "1".into(); }
                }
            }
            "0".into()
        }
        "host" | "hostname" => hostname_cached(),
        "host_short" => {
            let h = hostname_cached();
            h.split('.').next().unwrap_or(&h).to_string()
        }
        "pid" => std::process::id().to_string(),
        "version" => VERSION.to_string(),
        "mouse" => if app.mouse_enabled { "on".into() } else { "off".into() },
        "prefix" => format_key_binding(&app.prefix_key),
        "status" => if app.status_visible { "on".into() } else { "off".into() },
        "session_id" => format!("${}", app.session_name),
        "window_id" => format!("@{}", win_idx),
        "pane_id" => {
            let pid = get_active_pane_id(&win.root, &win.active_path).unwrap_or(0);
            format!("%{}", pid)
        }
        "window_zoomed_flag" => if app.zoom_saved.is_some() { "1".into() } else { "0".into() },
        "scroll_position" | "scroll_region_upper" => app.copy_scroll_offset.to_string(),
        "copy_cursor_x" => app.copy_pos.map(|(_, c)| c.to_string()).unwrap_or("0".into()),
        "copy_cursor_y" => app.copy_pos.map(|(r, _)| r.to_string()).unwrap_or("0".into()),
        "selection_present" => if app.copy_anchor.is_some() { "1".into() } else { "0".into() },
        "search_present" => if !app.copy_search_query.is_empty() { "1".into() } else { "0".into() },
        "buffer_size" => app.paste_buffers.first().map(|b| b.len().to_string()).unwrap_or("0".into()),
        "buffer_sample" => {
            app.paste_buffers.last().map(|b| {
                let sample: String = b.chars().take(50).collect();
                sample
            }).unwrap_or_default()
        }
        "client_session" => app.session_name.clone(),
        "client_name" | "client_tty" => "/dev/pts/0".to_string(),
        "window_layout" => format!("{}panes", count_panes(&win.root)),
        "session_created" => app.created_at.timestamp().to_string(),
        "session_created_string" => app.created_at.format("%a %b %e %H:%M:%S %Y").to_string(),
        "start_time" => app.created_at.timestamp().to_string(),
        "mode_keys" => app.mode_keys.clone(),
        _ => String::new(),
    }
}

/// Cached hostname lookup — called frequently in format expansion.
fn hostname_cached() -> String {
    use std::sync::OnceLock;
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME.get_or_init(|| {
        env::var("COMPUTERNAME")
            .or_else(|_| env::var("HOSTNAME"))
            .unwrap_or_default()
    }).clone()
}

/// Find the matching closing brace for `#{...}`, handling nested `#{...}` inside.
/// `start` is the index of the first character after `#{`.
/// Returns the index of the matching `}`.
fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'}' {
            depth -= 1;
            if depth == 0 { return Some(i); }
        } else if i + 1 < bytes.len() && bytes[i] == b'#' && bytes[i + 1] == b'{' {
            depth += 1;
            i += 1; // skip the '{'
        }
        i += 1;
    }
    None
}

/// Expand a tmux conditional: `?cond,true_branch,false_branch`
/// `cond` is a format variable name. Truthy if non-empty and not "0".
fn expand_conditional(expr: &str, app: &AppState, win_idx: usize) -> String {
    // Skip the leading '?'
    let body = &expr[1..];
    // Split on first comma for condition, then find second comma for branches
    // Handle nested #{...} in branches by tracking brace depth
    let (cond, true_branch, false_branch) = split_conditional(body);
    
    // Check for comparison operators: ==, !=
    let is_true = if let Some(eq_pos) = cond.find("==") {
        let lhs = expand_format_for_window(&format!("#{{{}}}",&cond[..eq_pos]), app, win_idx);
        let rhs = expand_format_for_window(&format!("#{{{}}}",&cond[eq_pos+2..]), app, win_idx);
        lhs == rhs
    } else if let Some(neq_pos) = cond.find("!=") {
        let lhs = expand_format_for_window(&format!("#{{{}}}",&cond[..neq_pos]), app, win_idx);
        let rhs = expand_format_for_window(&format!("#{{{}}}",&cond[neq_pos+2..]), app, win_idx);
        lhs != rhs
    } else {
        let cond_val = expand_format_for_window(&format!("#{{{}}}",&cond), app, win_idx);
        !cond_val.is_empty() && cond_val != "0"
    };
    
    if is_true {
        expand_format_for_window(&true_branch, app, win_idx)
    } else {
        expand_format_for_window(&false_branch, app, win_idx)
    }
}

/// Expand truncation: `=N:variable` - truncate expanded variable to N characters.
fn expand_truncation(expr: &str, app: &AppState, win_idx: usize) -> String {
    // expr = "=N:var" — skip the leading '='
    let body = &expr[1..];
    if let Some(colon_pos) = body.find(':') {
        let n_str = &body[..colon_pos];
        let var = &body[colon_pos + 1..];
        let expanded = expand_format_for_window(&format!("#{{{}}}" ,var), app, win_idx);
        if let Ok(n) = n_str.parse::<usize>() {
            if expanded.len() > n {
                return expanded[..n].to_string();
            }
        }
        expanded
    } else {
        String::new()
    }
}

/// Split conditional body `cond,true_branch,false_branch` respecting nested #{...}.
fn split_conditional(s: &str) -> (String, String, String) {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut commas: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < bytes.len() && commas.len() < 2 {
        if i + 1 < bytes.len() && bytes[i] == b'#' && bytes[i + 1] == b'{' {
            depth += 1;
            i += 2;
            continue;
        }
        if bytes[i] == b'}' && depth > 0 {
            depth -= 1;
            i += 1;
            continue;
        }
        if bytes[i] == b',' && depth == 0 {
            commas.push(i);
        }
        i += 1;
    }
    match commas.len() {
        0 => (s.to_string(), String::new(), String::new()),
        1 => (s[..commas[0]].to_string(), s[commas[0]+1..].to_string(), String::new()),
        _ => (s[..commas[0]].to_string(), s[commas[0]+1..commas[1]].to_string(), s[commas[1]+1..].to_string()),
    }
}
