use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{PtySize, PtySystemSelection};
use ratatui::prelude::*;

use crate::types::*;
use crate::tree::*;
use crate::pane::{create_window, detect_shell};
use crate::copy_mode::{scroll_copy_up, scroll_copy_down, yank_selection};

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
}

pub fn remote_mouse_down(app: &mut AppState, x: u16, y: u16) {
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
    let mut borders: Vec<(Vec<usize>, LayoutKind, usize, u16)> = Vec::new();
    compute_split_borders(&win.root, app.last_window_area, &mut borders);
    let tol = 1u16;
    for (path, kind, idx, pos) in borders.iter() {
        match kind {
            LayoutKind::Horizontal => {
                if x >= pos.saturating_sub(tol) && x <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: x, start_y: y, left_initial: left, _right_initial: right }); } on_border = true; break; }
            }
            LayoutKind::Vertical => {
                if y >= pos.saturating_sub(tol) && y <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: x, start_y: y, left_initial: left, _right_initial: right }); } on_border = true; break; }
            }
        }
    }

    // Forward left-click to child PTY
    if !on_border {
        if let Some(area) = active_area {
            let col = x.saturating_sub(area.x) + 1;
            let row = y.saturating_sub(area.y) + 1;
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                let _ = write!(active.master, "\x1b[<0;{};{}M", col, row);
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
        // Forward drag to child PTY (SGR: button 32 = motion + left held)
        if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
            let col = x.saturating_sub(area.x) + 1;
            let row = y.saturating_sub(area.y) + 1;
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                let _ = write!(active.master, "\x1b[<32;{};{}M", col, row);
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

    app.drag = None;

    // Forward mouse release to child PTY (SGR: button 0 release = lowercase m)
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let col = x.saturating_sub(area.x) + 1;
        let row = y.saturating_sub(area.y) + 1;
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = write!(active.master, "\x1b[<0;{};{}m", col, row);
        }
    }
}

/// Forward a non-left mouse button press/release to the child PTY.
/// `button`: 1 = middle, 2 = right (SGR encoding).
/// `press`: true = press (M), false = release (m).
pub fn remote_mouse_button(app: &mut AppState, x: u16, y: u16, button: u8, press: bool) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let col = x.saturating_sub(area.x) + 1;
        let row = y.saturating_sub(area.y) + 1;
        let end_char = if press { 'M' } else { 'm' };
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = write!(active.master, "\x1b[<{};{};{}{}", button, col, row, end_char);
        }
    }
}

/// Forward mouse motion (no button held) to the child PTY.
pub fn remote_mouse_motion(app: &mut AppState, x: u16, y: u16) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let col = x.saturating_sub(area.x) + 1;
        let row = y.saturating_sub(area.y) + 1;
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = write!(active.master, "\x1b[<35;{};{}M", col, row);
        }
    }
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

fn copy_cell_for_area(area: Rect, x: u16, y: u16) -> (u16, u16) {
    // Convert global terminal coordinates to 0-based pane-local coordinates.
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_w = area.width.saturating_sub(2).max(1);
    let inner_h = area.height.saturating_sub(2).max(1);

    let col = x
        .saturating_sub(inner_x)
        .min(inner_w.saturating_sub(1));
    let row = y
        .saturating_sub(inner_y)
        .min(inner_h.saturating_sub(1));
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

    let (col, row) = target_area.map_or((1, 1), |area| wheel_cell_for_area(area, x, y));
    let code = if up { 64 } else { 65 };
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        let _ = write!(p.master, "\x1b[<{};{};{}M", code, col, row);
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
    
    let mut best: Option<(usize, u32)> = None;
    for (i, (_, r)) in rects.iter().enumerate() {
        if i == ai { continue; }
        let candidate = match dir {
            FocusDir::Left => if r.x + r.width <= arect.x { Some((arect.x - (r.x + r.width)) as u32) } else { None },
            FocusDir::Right => if r.x >= arect.x + arect.width { Some((r.x - (arect.x + arect.width)) as u32) } else { None },
            FocusDir::Up => if r.y + r.height <= arect.y { Some((arect.y - (r.y + r.height)) as u32) } else { None },
            FocusDir::Down => if r.y >= arect.y + arect.height { Some((r.y - (arect.y + arect.height)) as u32) } else { None },
        };
        if let Some(dist) = candidate { 
            if best.map_or(true, |(_,bd)| dist < bd) { best = Some((i, dist)); } 
        }
    }
    
    if let Some((ni, _)) = best { 
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

pub fn rotate_panes(app: &mut AppState, _reverse: bool) {
    let win = &mut app.windows[app.active_idx];
    match &mut win.root {
        Node::Split { children, .. } if children.len() >= 2 => {
            let last_idx = children.len() - 1;
            children.swap(0, last_idx);
        }
        _ => {}
    }
}

pub fn break_pane_to_window(app: &mut AppState) {
    let pty_system = match PtySystemSelection::default().get() {
        Ok(p) => p,
        Err(_) => return,
    };
    let _ = create_window(&*pty_system, app, None);
}

pub fn respawn_active_pane(app: &mut AppState) -> io::Result<()> {
    let pty_system = PtySystemSelection::default().get().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
    let win = &mut app.windows[app.active_idx];
    let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) else { return Ok(()); };
    
    let size = PtySize { rows: pane.last_rows, cols: pane.last_cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    let shell_cmd = detect_shell();
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 1000)));
    let term_reader = term.clone();
    let mut reader = pair.master.try_clone_reader().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    
    thread::spawn(move || {
        let mut local = [0u8; 8192];
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => { let mut parser = term_reader.lock().unwrap(); parser.process(&local[..n]); }
                Ok(_) => thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    });
    
    pane.master = pair.master;
    pane.child = child;
    pane.term = term;
    
    Ok(())
}
