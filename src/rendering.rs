use std::io::{self, Write};
use std::env;
use ratatui::prelude::*;
use ratatui::widgets::*;
use ratatui::style::{Style, Modifier};
use unicode_width::UnicodeWidthStr;
use crossterm::style::Print;
use crossterm::execute;
use portable_pty::PtySize;

use crate::types::*;
use crate::tree::active_pane_mut;

pub fn vt_to_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => match i {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Rgb(128, 128, 128),
            8 => Color::Rgb(80, 80, 80),
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            _ => Color::Reset,
        },
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn dim_color(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb((r as u16 * 2 / 5) as u8, (g as u16 * 2 / 5) as u8, (b as u16 * 2 / 5) as u8),
        Color::Black => Color::Rgb(40, 40, 40),
        Color::White | Color::Gray | Color::DarkGray => Color::Rgb(100, 100, 100),
        Color::LightRed => Color::Rgb(150, 80, 80),
        Color::LightGreen => Color::Rgb(80, 150, 80),
        Color::LightYellow => Color::Rgb(150, 150, 80),
        Color::LightBlue => Color::Rgb(80, 120, 180),
        Color::LightMagenta => Color::Rgb(150, 80, 150),
        Color::LightCyan => Color::Rgb(80, 150, 150),
        _ => Color::Rgb(80, 80, 80),
    }
}

pub fn dim_predictions_enabled() -> bool {
    std::env::var("PSMUX_DIM_PREDICTIONS").map(|v| v != "0" && v.to_lowercase() != "false").unwrap_or(true)
}

pub fn apply_cursor_style<W: Write>(out: &mut W) -> io::Result<()> {
    let style = env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string());
    let blink = env::var("PSMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0";
    let code = match style.as_str() {
        "block" => if blink { 1 } else { 2 },
        "underline" => if blink { 3 } else { 4 },
        "bar" | "beam" => if blink { 5 } else { 6 },
        _ => if blink { 5 } else { 6 },
    };
    execute!(out, Print(format!("\x1b[{} q", code)))?;
    Ok(())
}

pub fn render_window(f: &mut Frame, app: &mut AppState, area: Rect) {
    let win = &mut app.windows[app.active_idx];
    render_node(f, &mut win.root, &win.active_path, &mut Vec::new(), area);
}

pub fn render_node(f: &mut Frame, node: &mut Node, active_path: &Vec<usize>, cur_path: &mut Vec<usize>, area: Rect) {
    match node {
        Node::Leaf(pane) => {
            let is_active = *cur_path == *active_path;
            let title = if is_active { "* pane" } else { " pane" };
            let pane_block = Block::default().borders(Borders::ALL).title(title);
            let inner = pane_block.inner(area);
            let target_rows = inner.height.max(1);
            let target_cols = inner.width.max(1);
            if pane.last_rows != target_rows || pane.last_cols != target_cols {
                let _ = pane.master.resize(PtySize { rows: target_rows, cols: target_cols, pixel_width: 0, pixel_height: 0 });
                let mut parser = pane.term.lock().unwrap();
                parser.screen_mut().set_size(target_rows, target_cols);
                pane.last_rows = target_rows;
                pane.last_cols = target_cols;
            }
            let parser = pane.term.lock().unwrap();
            let screen = parser.screen();
            let (cur_r, cur_c) = screen.cursor_position();
            let dim_preds = dim_predictions_enabled();
            let mut lines: Vec<Line> = Vec::with_capacity(target_rows as usize);
            for r in 0..target_rows {
                let mut spans: Vec<Span> = Vec::with_capacity(target_cols as usize);
                let mut c = 0;
                while c < target_cols {
                    if let Some(cell) = screen.cell(r, c) {
                        let mut fg = vt_to_color(cell.fgcolor());
                        let mut bg = vt_to_color(cell.bgcolor());
                        if cell.inverse() { std::mem::swap(&mut fg, &mut bg); }
                        if dim_preds && (r > cur_r || (r == cur_r && c >= cur_c)) {
                            fg = dim_color(fg);
                        }
                        let mut style = Style::default().fg(fg).bg(bg);
                        if cell.dim() { style = style.add_modifier(Modifier::DIM); }
                        if cell.bold() { style = style.add_modifier(Modifier::BOLD); }
                        if cell.italic() { style = style.add_modifier(Modifier::ITALIC); }
                        if cell.underline() { style = style.add_modifier(Modifier::UNDERLINED); }
                        let text = cell.contents().to_string();
                        let w = UnicodeWidthStr::width(text.as_str()) as u16;
                        if w == 0 {
                            spans.push(Span::styled(" ", style));
                            c += 1;
                        } else if w >= 2 {
                            spans.push(Span::styled(text, style));
                            c += 2; // skip continuation cell
                        } else {
                            spans.push(Span::styled(text, style));
                            c += 1;
                        }
                    } else {
                        spans.push(Span::raw(" "));
                        c += 1;
                    }
                }
                lines.push(Line::from(spans));
            }
            f.render_widget(pane_block, area);
            f.render_widget(Clear, inner);
            let para = Paragraph::new(Text::from(lines));
            f.render_widget(para, inner);
            if is_active {
                let (cr, cc) = screen.cursor_position();
                let cr = cr.min(target_rows.saturating_sub(1));
                let cc = cc.min(target_cols.saturating_sub(1));
                let cx = inner.x + cc;
                let cy = inner.y + cr;
                f.set_cursor(cx, cy);
            }
        }
        Node::Split { kind, sizes, children } => {
            let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
            } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
            let rects = match *kind {
                LayoutKind::Horizontal => Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area),
                LayoutKind::Vertical => Layout::default().direction(Direction::Vertical).constraints(constraints).split(area),
            };
            for (i, child) in children.iter_mut().enumerate() {
                cur_path.push(i);
                render_node(f, child, active_path, cur_path, rects[i]);
                cur_path.pop();
            }
        }
    }
}

pub fn expand_status(fmt: &str, app: &AppState, time_str: &str) -> String {
    let mut s = fmt.to_string();
    let window = &app.windows[app.active_idx];
    s = s.replace("#I", &(app.active_idx + 1).to_string());
    s = s.replace("#W", &window.name);
    s = s.replace("#S", "psmux");
    s = s.replace("%H:%M", time_str);
    s
}

pub fn parse_status(fmt: &str, app: &AppState, time_str: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cur_style = Style::default();
    let mut i = 0;
    while i < fmt.len() {
        if fmt.as_bytes()[i] == b'#' && i + 1 < fmt.len() && fmt.as_bytes()[i+1] == b'[' {
            if let Some(end) = fmt[i+2..].find(']') { 
                let token = &fmt[i+2..i+2+end];
                for part in token.split(',') {
                    let p = part.trim();
                    if p.starts_with("fg=") { cur_style = cur_style.fg(map_color(&p[3..])); }
                    else if p.starts_with("bg=") { cur_style = cur_style.bg(map_color(&p[3..])); }
                    else if p == "bold" { cur_style = cur_style.add_modifier(Modifier::BOLD); }
                    else if p == "italic" { cur_style = cur_style.add_modifier(Modifier::ITALIC); }
                    else if p == "underline" { cur_style = cur_style.add_modifier(Modifier::UNDERLINED); }
                    else if p == "default" { cur_style = Style::default(); }
                }
                i += 2 + end + 1; 
                continue;
            }
        }
        let mut j = i;
        while j < fmt.len() && !(fmt.as_bytes()[j] == b'#' && j + 1 < fmt.len() && fmt.as_bytes()[j+1] == b'[') { j += 1; }
        let chunk = &fmt[i..j];
        let text = expand_status(chunk, app, time_str);
        spans.push(Span::styled(text, cur_style));
        i = j;
    }
    spans
}

pub fn map_color(name: &str) -> Color {
    if let Some(idx_str) = name.strip_prefix("idx:") {
        if let Ok(idx) = idx_str.parse::<u8>() {
            return Color::Indexed(idx);
        }
    }
    if let Some(rgb_str) = name.strip_prefix("rgb:") {
        let parts: Vec<&str> = rgb_str.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (parts[0].parse::<u8>(), parts[1].parse::<u8>(), parts[2].parse::<u8>()) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    match name.to_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "default" => Color::Reset,
        _ => Color::Reset,
    }
}

pub fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(height),
            Constraint::Percentage(50),
        ])
        .split(r);
    let middle = popup_layout[1];
    let width = (middle.width * percent_x) / 100;
    let x = middle.x + (middle.width - width) / 2;
    Rect { x, y: middle.y, width, height }
}
