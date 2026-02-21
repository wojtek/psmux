use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{PtySize, PtySystemSelection};
use ratatui::prelude::*;

use crate::types::*;
use crate::tree::*;
use crate::pane::{create_window, detect_shell, build_default_shell, set_tmux_env};
use crate::copy_mode::{scroll_copy_up, scroll_copy_down, yank_selection};
use crate::platform::mouse_inject;

/// Mouse debug logger — writes to ~/.psmux/mouse_debug.log when enabled.
/// Set PSMUX_MOUSE_DEBUG=1 environment variable to enable.
fn mouse_log(msg: &str) {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    static CHECKED: AtomicBool = AtomicBool::new(false);
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static COUNT: AtomicU32 = AtomicU32::new(0);

    if !CHECKED.swap(true, Ordering::Relaxed) {
        let on = std::env::var("PSMUX_MOUSE_DEBUG").map_or(false, |v| v == "1" || v == "true");
        ENABLED.store(on, Ordering::Relaxed);
    }
    if !ENABLED.load(Ordering::Relaxed) { return; }

    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if n > 500 { return; } // cap log size

    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let path = format!("{}/.psmux/mouse_debug.log", home);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
    }
}

/// Convert screen coordinates to 0-based pane-local coordinates.
/// No border offset — panes are borderless (tmux-style).
fn pane_inner_cell_0based(area: Rect, abs_x: u16, abs_y: u16) -> (i16, i16) {
    let col = abs_x as i16 - area.x as i16;
    let row = abs_y as i16 - area.y as i16;
    (col, row)
}

/// Convert screen coordinates to 1-based pane-local coordinates.
fn pane_inner_cell(area: Rect, abs_x: u16, abs_y: u16) -> (u16, u16) {
    let col = abs_x.saturating_sub(area.x) + 1;
    let row = abs_y.saturating_sub(area.y) + 1;
    (col, row)
}

/// Write a mouse event to the child PTY using the encoding the child requested.
fn write_mouse_event_remote(master: &mut Box<dyn portable_pty::MasterPty>, button: u8, col: u16, row: u16, press: bool, enc: vt100::MouseProtocolEncoding) {
    use std::io::Write;
    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            let ch = if press { 'M' } else { 'm' };
            let _ = write!(master, "\x1b[<{};{};{}{}", button, col, row, ch);
            let _ = master.flush();
        }
        _ => {
            if press {
                let cb = (button + 32) as u8;
                let cx = ((col as u8).min(223)) + 32;
                let cy = ((row as u8).min(223)) + 32;
                let _ = master.write_all(&[0x1b, b'[', b'M', cb, cx, cy]);
                let _ = master.flush();
            }
        }
    }
}

/// Inject a mouse event into a pane via Windows Console API (WriteConsoleInputW).
///
/// For native Windows console apps: WriteConsoleInputW injects MOUSE_EVENT records
/// that ReadConsoleInput returns.  This works for apps like pstop, Far Manager, etc.
fn inject_mouse(pane: &mut Pane, col: i16, row: i16, button_state: u32, event_flags: u32) -> bool {
    if pane.child_pid.is_none() {
        pane.child_pid = unsafe { mouse_inject::get_child_pid(&*pane.child) };
    }
    if let Some(pid) = pane.child_pid {
        mouse_inject::send_mouse_event(pid, col, row, button_state, event_flags, false)
    } else {
        false
    }
}

/// Returns true if the window's foreground process is a VT bridge (wsl, ssh)
/// that needs VT mouse injection instead of Console API mouse injection.
fn is_vt_bridge(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("wsl") || lower.contains("ssh")
}

/// Query the pane's vt100 parser for the child's mouse protocol mode and encoding.
fn pane_mouse_protocol(pane: &Pane) -> (vt100::MouseProtocolMode, vt100::MouseProtocolEncoding) {
    if let Ok(parser) = pane.term.lock() {
        let s = parser.screen();
        (s.mouse_protocol_mode(), s.mouse_protocol_encoding())
    } else {
        (vt100::MouseProtocolMode::None, vt100::MouseProtocolEncoding::Default)
    }
}

/// Check if the pane is likely running a fullscreen TUI app (htop, vim, etc.)
/// by detecting alternate screen buffer usage.
///
/// ConPTY never passes DECSET 1049h (alternate screen) to the output pipe,
/// so `screen.alternate_screen()` is always false.  Use the same heuristic
/// as layout.rs: if the last row of the screen has non-blank content, the
/// pane is running a fullscreen app.
fn is_fullscreen_tui(pane: &Pane) -> bool {
    if let Ok(parser) = pane.term.lock() {
        let screen = parser.screen();
        // Fast check: if the parser reports alternate screen, trust it
        if screen.alternate_screen() {
            return true;
        }
        // Heuristic: check if the last row has non-blank content.
        // TUI apps fill the entire screen, shell prompts leave the bottom empty.
        let last_row = pane.last_rows.saturating_sub(1);
        for col in 0..pane.last_cols {
            if let Some(cell) = screen.cell(last_row, col) {
                let t = cell.contents();
                if !t.is_empty() && t != " " {
                    return true;
                }
            }
        }
    }
    false
}

/// Detect whether a pane has a VT bridge descendant (wsl.exe, ssh.exe, etc.)
/// by walking the process tree.  Result is cached for 2 seconds per pane
/// to avoid expensive CreateToolhelp32Snapshot on every mouse event.
fn detect_vt_bridge(pane: &mut Pane) -> bool {
    // Check cache first (2 second TTL)
    if let Some((ts, cached)) = pane.vt_bridge_cache {
        if ts.elapsed().as_secs() < 2 {
            return cached;
        }
    }
    // Ensure child_pid is resolved
    if pane.child_pid.is_none() {
        pane.child_pid = unsafe { mouse_inject::get_child_pid(&*pane.child) };
    }
    let result = if let Some(pid) = pane.child_pid {
        crate::platform::process_info::has_vt_bridge_descendant(pid)
    } else {
        false
    };
    pane.vt_bridge_cache = Some((std::time::Instant::now(), result));
    result
}

/// Inject a mouse event into a pane using the best available method.
///
/// Strategy:
///   1. If the vt100 parser detected mouse protocol (child sent DECSET 1000h),
///      use VT injection with the child's requested encoding.
///   2. If the process tree contains a VT bridge (wsl.exe, ssh.exe, etc.)
///      AND a fullscreen TUI app is running (alternate screen buffer active),
///      inject SGR mouse as KEY_EVENT records via WriteConsoleInputW.
///      This bypasses ConPTY entirely — the raw escape sequence characters
///      reach wsl.exe → Linux PTY → htop/vim/etc.
///      When not in fullscreen mode (bash prompt), skip VT injection to
///      prevent garbage characters.  Fall back to Win32 MOUSE_EVENT which
///      is harmlessly ignored by wsl.exe.
///   3. Otherwise (native console apps like pstop), use Win32 console injection
///      which directly writes MOUSE_EVENT records to the console input buffer.
fn inject_mouse_combined(pane: &mut Pane, col: i16, row: i16, vt_button: u8, press: bool,
                          button_state: u32, event_flags: u32, win_name: &str) {
    let (mode, enc) = pane_mouse_protocol(pane);
    let vt_bridge = detect_vt_bridge(pane);

    mouse_log(&format!("inject_mouse_combined: col={} row={} vt_btn={} press={} btn_state=0x{:X} evt_flags=0x{:X} win={} mode={:?} enc={:?} vt_bridge={}",
        col, row, vt_button, press, button_state, event_flags, win_name, mode, enc, vt_bridge));

    if mode != vt100::MouseProtocolMode::None {
        // Child explicitly requested mouse tracking — use VT injection with child's encoding
        let vt_col = (col + 1).max(1) as u16;
        let vt_row = (row + 1).max(1) as u16;
        mouse_log(&format!("  -> VT injection (parser mode): enc={:?} vt_col={} vt_row={}", enc, vt_col, vt_row));
        write_mouse_event_remote(&mut pane.master, vt_button, vt_col, vt_row, press, enc);
    } else if vt_bridge {
        // VT bridge (WSL/SSH): check if a fullscreen TUI app is running
        let fullscreen = is_fullscreen_tui(pane);
        mouse_log(&format!("  -> VT bridge: fullscreen={}", fullscreen));
        if fullscreen {
            // TUI app detected (htop, vim, etc.) — inject SGR mouse as KEY_EVENTs.
            // This bypasses ConPTY and delivers raw escape sequence characters to
            // wsl.exe → Linux PTY → the TUI app.
            let vt_col = (col + 1).max(1) as u16;
            let vt_row = (row + 1).max(1) as u16;
            let ch = if press { 'M' } else { 'm' };
            let sgr_seq = format!("\x1b[<{};{};{}{}", vt_button, vt_col, vt_row, ch);
            mouse_log(&format!("  -> Console VT injection (KEY_EVENTs): seq={:?}", sgr_seq));
            if pane.child_pid.is_none() {
                pane.child_pid = unsafe { mouse_inject::get_child_pid(&*pane.child) };
            }
            if let Some(pid) = pane.child_pid {
                let ok = mouse_inject::send_vt_sequence(pid, sgr_seq.as_bytes());
                mouse_log(&format!("  -> Console VT inject result: {}", ok));
            }
        } else {
            // Shell prompt (no fullscreen TUI) — use Win32 MOUSE_EVENT injection.
            // wsl.exe ignores MOUSE_EVENT records (it uses ReadFile, not ReadConsoleInput),
            // so this is harmless — no garbage characters are printed.
            mouse_log(&format!("  -> Win32 injection (vt_bridge fallback): col={} row={} btn=0x{:X} flags=0x{:X}", col, row, button_state, event_flags));
            let ok = inject_mouse(pane, col, row, button_state, event_flags);
            mouse_log(&format!("  -> Win32 inject result: {}", ok));
        }
    } else {
        // Native console app — Win32 console injection only
        mouse_log(&format!("  -> Win32 injection (native): col={} row={} btn=0x{:X} flags=0x{:X}", col, row, button_state, event_flags));
        let ok = inject_mouse(pane, col, row, button_state, event_flags);
        mouse_log(&format!("  -> Win32 inject result: {}", ok));
    }
}

pub fn toggle_zoom(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    if app.zoom_saved.is_none() {
        let mut saved: Vec<(Vec<usize>, Vec<u16>)> = Vec::new();
        for depth in 0..win.active_path.len() {
            let p = win.active_path[..depth].to_vec();
            if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &p) {
                let idx = win.active_path.get(depth).copied().unwrap_or(0);
                saved.push((p.clone(), sizes.clone()));
                for i in 0..sizes.len() { sizes[i] = if i == idx { 100 } else { 0 }; }
            }
        }
        app.zoom_saved = Some(saved);
    } else {
        if let Some(saved) = app.zoom_saved.take() {
            for (p, sz) in saved.into_iter() {
                if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &p) { *sizes = sz; }
            }
        }
    }
    // Resize all panes so child PTYs are notified of the new dimensions.
    // Without this, zoomed panes keep their pre-zoom size and child apps
    // (neovim, bottom, etc.) render in only half the screen. (issue #35)
    resize_all_panes(app);
}

/// Compute tab positions on the server side to match the client's status bar layout.
/// The client renders: "[session_name] idx: window_name idx: window_name ..."
pub fn update_tab_positions(app: &mut AppState) {
    let mut tab_pos: Vec<(usize, u16, u16)> = Vec::new();
    let mut cursor_x: u16 = 0;
    // Session label: "[session_name] "
    let session_label_len = app.session_name.len() as u16 + 3; // '[' + name + ']' + ' '
    cursor_x += session_label_len;
    // Window tabs: "idx: window_name " for each window
    for (i, w) in app.windows.iter().enumerate() {
        let display_idx = i + app.window_base_index;
        let label = format!("{}: {} ", display_idx, w.name);
        let start_x = cursor_x;
        cursor_x += label.len() as u16;
        tab_pos.push((i, start_x, cursor_x));
    }
    app.tab_positions = tab_pos;
}

pub fn remote_mouse_down(app: &mut AppState, x: u16, y: u16) {
    // Recompute tab positions to match client rendering
    update_tab_positions(app);

    // Check tab click on status bar
    let status_row = app.last_window_area.y + app.last_window_area.height;
    if y == status_row {
        for &(win_idx, x_start, x_end) in app.tab_positions.iter() {
            if x >= x_start && x < x_end && win_idx < app.windows.len() {
                app.last_window_idx = app.active_idx;
                app.active_idx = win_idx;
                return;
            }
        }
        return;
    }

    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    let mut active_area: Option<Rect> = None;
    for (path, area) in rects.iter() {
        if area.contains(ratatui::layout::Position { x, y }) {
            win.active_path = path.clone();
            active_area = Some(*area);
        }
    }

    if matches!(app.mode, Mode::CopyMode) {
        if let Some(area) = active_area {
            let (row, col) = copy_cell_for_area(area, x, y);
            app.copy_anchor = Some((row, col));
            app.copy_pos = Some((row, col));
        }
        return;
    }

    let mut on_border = false;
    let mut borders: Vec<(Vec<usize>, LayoutKind, usize, u16, u16)> = Vec::new();
    compute_split_borders(&win.root, app.last_window_area, &mut borders);
    let tol = 1u16;
    for (path, kind, idx, pos, total_px) in borders.iter() {
        match kind {
            LayoutKind::Horizontal => {
                if x >= pos.saturating_sub(tol) && x <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: *pos, start_y: y, left_initial: left, _right_initial: right, total_pixels: *total_px }); } on_border = true; break; }
            }
            LayoutKind::Vertical => {
                if y >= pos.saturating_sub(tol) && y <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: x, start_y: *pos, left_initial: left, _right_initial: right, total_pixels: *total_px }); } on_border = true; break; }
            }
        }
    }

    // Forward left-click to child pane
    if !on_border {
        if let Some(area) = active_area {
            let (col, row) = pane_inner_cell_0based(area, x, y);
            let win_name = win.name.clone();
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                inject_mouse_combined(active, col, row, 0, true,
                    mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED, 0, &win_name);
            }
        }
    }
}

pub fn remote_mouse_drag(app: &mut AppState, x: u16, y: u16) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);

    if matches!(app.mode, Mode::CopyMode) {
        if let Some((path, area)) = rects.iter().find(|(_, area)| area.contains(ratatui::layout::Position { x, y })) {
            win.active_path = path.clone();
            let (row, col) = copy_cell_for_area(*area, x, y);
            if app.copy_anchor.is_none() {
                app.copy_anchor = Some((row, col));
            }
            app.copy_pos = Some((row, col));
        }
        return;
    }

    if let Some(d) = &app.drag {
        adjust_split_sizes(&mut win.root, d, x, y);
    } else {
        // Forward drag to child pane
        if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
            let (col, row) = pane_inner_cell_0based(area, x, y);
            let win_name = win.name.clone();
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                inject_mouse_combined(active, col, row, 32, true,
                    mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED, mouse_inject::MOUSE_MOVED, &win_name);
            }
        }
    }
}

pub fn remote_mouse_up(app: &mut AppState, x: u16, y: u16) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);

    if matches!(app.mode, Mode::CopyMode) {
        if let Some((path, area)) = rects.iter().find(|(_, area)| area.contains(ratatui::layout::Position { x, y })) {
            win.active_path = path.clone();
            let (row, col) = copy_cell_for_area(*area, x, y);
            if app.copy_anchor.is_none() {
                app.copy_anchor = Some((row, col));
            }
            app.copy_pos = Some((row, col));
        }
        let _ = yank_selection(app);
        return;
    }

    // If we were dragging a border, resize all panes to match new layout
    let was_dragging = app.drag.is_some();
    app.drag = None;
    if was_dragging {
        resize_all_panes(app);
        return;
    }

    // Forward mouse release to child pane
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let (col, row) = pane_inner_cell_0based(area, x, y);
        let win_name = win.name.clone();
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            inject_mouse_combined(active, col, row, 0, false,
                0, 0, &win_name);
        }
    }
}

/// Forward a non-left mouse button press/release to the child.
pub fn remote_mouse_button(app: &mut AppState, x: u16, y: u16, button: u8, press: bool) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let (col, row) = pane_inner_cell_0based(area, x, y);
        let win_name = win.name.clone();
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            let sgr_btn = match button {
                1 => 1u8, // middle
                2 => 2u8, // right
                _ => 0u8,
            };
            let button_state = if press {
                match button {
                    1 => mouse_inject::FROM_LEFT_2ND_BUTTON_PRESSED,
                    2 => mouse_inject::RIGHTMOST_BUTTON_PRESSED,
                    _ => 0,
                }
            } else {
                0
            };
            inject_mouse_combined(active, col, row, sgr_btn, press,
                button_state, 0, &win_name);
        }
    }
}

/// Forward mouse motion to the child PTY - currently disabled to avoid garbage.
/// Most TUI apps don't want constant mouse position updates without button held.
pub fn remote_mouse_motion(_app: &mut AppState, _x: u16, _y: u16) {
    // Don't forward bare motion - only forward drag events
}

fn wheel_cell_for_area(area: Rect, x: u16, y: u16) -> (u16, u16) {
    // Convert global terminal coordinates to 1-based pane-local coordinates (no border offset).
    let col = x.saturating_sub(area.x).min(area.width.saturating_sub(1)).saturating_add(1);
    let row = y.saturating_sub(area.y).min(area.height.saturating_sub(1)).saturating_add(1);
    (col, row)
}

fn copy_cell_for_area(area: Rect, x: u16, y: u16) -> (u16, u16) {
    // Convert global terminal coordinates to 0-based pane-local coordinates (no border offset).
    let col = x.saturating_sub(area.x).min(area.width.saturating_sub(1));
    let row = y.saturating_sub(area.y).min(area.height.saturating_sub(1));
    (row, col)
}

fn remote_scroll_wheel(app: &mut AppState, x: u16, y: u16, up: bool) {
    if matches!(app.mode, Mode::CopyMode) {
        if up { scroll_copy_up(app, 3); } else { scroll_copy_down(app, 3); }
        return;
    }

    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);

    let mut target_area: Option<Rect> = None;
    for (path, area) in &rects {
        if area.contains(ratatui::layout::Position { x, y }) {
            win.active_path = path.clone();
            target_area = Some(*area);
            break;
        }
    }
    if target_area.is_none() {
        target_area = rects
            .iter()
            .find(|(path, _)| *path == win.active_path)
            .map(|(_, area)| *area);
    }

    let (col, row) = target_area.map_or((0, 0), |area| pane_inner_cell_0based(area, x, y));
    let win_name = win.name.clone();
    let sgr_btn: u8 = if up { 64 } else { 65 };
    let wheel_delta: i16 = if up { 120 } else { -120 };
    let button_state = ((wheel_delta as i32) << 16) as u32;
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        inject_mouse_combined(p, col, row, sgr_btn, true,
            button_state, mouse_inject::MOUSE_WHEELED, &win_name);
    }
}

pub fn remote_scroll_up(app: &mut AppState, x: u16, y: u16) { remote_scroll_wheel(app, x, y, true); }
pub fn remote_scroll_down(app: &mut AppState, x: u16, y: u16) { remote_scroll_wheel(app, x, y, false); }

pub fn swap_pane(app: &mut AppState, dir: FocusDir) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    
    let mut active_idx = None;
    for (i, (path, _)) in rects.iter().enumerate() { 
        if *path == win.active_path { active_idx = Some(i); break; } 
    }
    let Some(ai) = active_idx else { return; };
    let (_, arect) = &rects[ai];
    
    if let Some(ni) = crate::input::find_best_pane_in_direction(&rects, ai, arect, dir) {
        win.active_path = rects[ni].0.clone();
    }
}

pub fn resize_pane_vertical(app: &mut AppState, amount: i16) {
    let win = &mut app.windows[app.active_idx];
    if win.active_path.is_empty() { return; }
    
    for depth in (0..win.active_path.len()).rev() {
        let parent_path = win.active_path[..depth].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path) {
            if *kind == LayoutKind::Vertical {
                let idx = win.active_path[depth];
                if idx < sizes.len() {
                    let new_size = (sizes[idx] as i16 + amount).max(1) as u16;
                    let diff = new_size as i16 - sizes[idx] as i16;
                    sizes[idx] = new_size;
                    if idx + 1 < sizes.len() {
                        sizes[idx + 1] = (sizes[idx + 1] as i16 - diff).max(1) as u16;
                    } else if idx > 0 {
                        sizes[idx - 1] = (sizes[idx - 1] as i16 - diff).max(1) as u16;
                    }
                }
                return;
            }
        }
    }
}

pub fn resize_pane_horizontal(app: &mut AppState, amount: i16) {
    let win = &mut app.windows[app.active_idx];
    if win.active_path.is_empty() { return; }
    
    for depth in (0..win.active_path.len()).rev() {
        let parent_path = win.active_path[..depth].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path) {
            if *kind == LayoutKind::Horizontal {
                let idx = win.active_path[depth];
                if idx < sizes.len() {
                    let new_size = (sizes[idx] as i16 + amount).max(1) as u16;
                    let diff = new_size as i16 - sizes[idx] as i16;
                    sizes[idx] = new_size;
                    if idx + 1 < sizes.len() {
                        sizes[idx + 1] = (sizes[idx + 1] as i16 - diff).max(1) as u16;
                    } else if idx > 0 {
                        sizes[idx - 1] = (sizes[idx - 1] as i16 - diff).max(1) as u16;
                    }
                }
                return;
            }
        }
    }
}

/// Absolute resize: set the active pane's share to an exact size.
/// axis is "x" (width/horizontal) or "y" (height/vertical).
pub fn resize_pane_absolute(app: &mut AppState, axis: &str, target: u16) {
    let win = &mut app.windows[app.active_idx];
    if win.active_path.is_empty() { return; }
    let target_kind = if axis == "x" { LayoutKind::Horizontal } else { LayoutKind::Vertical };
    for depth in (0..win.active_path.len()).rev() {
        let parent_path = win.active_path[..depth].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path) {
            if *kind == target_kind {
                let idx = win.active_path[depth];
                if idx < sizes.len() {
                    let old = sizes[idx];
                    let new = target.max(1);
                    let diff = new as i16 - old as i16;
                    sizes[idx] = new;
                    // Absorb the difference from a neighbour
                    if idx + 1 < sizes.len() {
                        sizes[idx + 1] = (sizes[idx + 1] as i16 - diff).max(1) as u16;
                    } else if idx > 0 {
                        sizes[idx - 1] = (sizes[idx - 1] as i16 - diff).max(1) as u16;
                    }
                }
                return;
            }
        }
    }
}

pub fn rotate_panes(app: &mut AppState, reverse: bool) {
    let win = &mut app.windows[app.active_idx];
    match &mut win.root {
        Node::Split { children, .. } if children.len() >= 2 => {
            if reverse {
                // Rotate counter-clockwise: first element goes to end
                let first = children.remove(0);
                children.push(first);
            } else {
                // Rotate clockwise: last element goes to front
                let last = children.pop().unwrap();
                children.insert(0, last);
            }
        }
        _ => {}
    }
}

pub fn break_pane_to_window(app: &mut AppState) {
    let src_idx = app.active_idx;
    let src_path = app.windows[src_idx].active_path.clone();
    
    // Extract the active pane from the current window using tree operations
    let src_root = std::mem::replace(&mut app.windows[src_idx].root,
        Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
    let (remaining, extracted) = crate::tree::extract_node(src_root, &src_path);
    
    if let Some(pane_node) = extracted {
        let src_empty = remaining.is_none();
        if let Some(rem) = remaining {
            app.windows[src_idx].root = rem;
            app.windows[src_idx].active_path = crate::tree::first_leaf_path(&app.windows[src_idx].root);
        }
        
        // Determine the window name from the pane
        let win_name = match &pane_node {
            Node::Leaf(p) => p.title.clone(),
            _ => format!("win {}", app.windows.len() + 1),
        };
        
        // Create new window containing the extracted pane
        app.windows.push(Window {
            root: pane_node,
            active_path: vec![],
            name: win_name,
            id: app.next_win_id,
            activity_flag: false,
            bell_flag: false,
            silence_flag: false,
            last_output_time: std::time::Instant::now(),
            last_seen_version: 0,
            manual_rename: false,
            layout_index: 0,
        });
        app.next_win_id += 1;
        
        if src_empty {
            app.windows.remove(src_idx);
        }
        
        // Switch to the new window
        app.active_idx = app.windows.len() - 1;
    } else {
        // Extraction failed — restore
        if let Some(rem) = remaining {
            app.windows[src_idx].root = rem;
        }
    }
}

pub fn respawn_active_pane(app: &mut AppState) -> io::Result<()> {
    let pty_system = PtySystemSelection::default().get().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
    let win = &mut app.windows[app.active_idx];
    let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) else { return Ok(()); };
    let pane_id = pane.id;
    
    let size = PtySize { rows: pane.last_rows, cols: pane.last_cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    let mut shell_cmd = if !app.default_shell.is_empty() {
        build_default_shell(&app.default_shell)
    } else {
        detect_shell()
    };
    set_tmux_env(&mut shell_cmd, pane_id, app.control_port, app.socket_name.as_deref());
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // Close the slave handle immediately – required for ConPTY.
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
    
    pane.master = pair.master;
    pane.child = child;
    pane.term = term;
    pane.data_version = data_version;
    pane.child_pid = None;
    pane.dead = false;
    
    Ok(())
}
