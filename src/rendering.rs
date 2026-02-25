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
use crate::tree::{active_pane_mut, split_with_gaps};

pub fn vt_to_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
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
    std::env::var("PSMUX_DIM_PREDICTIONS").map(|v| v == "1" || v.to_lowercase() == "true").unwrap_or(false)
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
    let dim_preds = app.prediction_dimming;
    let border_style = parse_tmux_style(&app.pane_border_style);
    let active_border_style = parse_tmux_style(&app.pane_active_border_style);
    let copy_cursor = if matches!(app.mode, Mode::CopyMode) { app.copy_pos } else { None };
    let win = &mut app.windows[app.active_idx];
    let active_rect = compute_active_rect(&win.root, &win.active_path, area);
    render_node(f, &mut win.root, &win.active_path, &mut Vec::new(), area, dim_preds, border_style, active_border_style, copy_cursor, active_rect);
}

pub fn render_node(
    f: &mut Frame,
    node: &mut Node,
    active_path: &Vec<usize>,
    cur_path: &mut Vec<usize>,
    area: Rect,
    dim_preds: bool,
    border_style: Style,
    active_border_style: Style,
    copy_cursor: Option<(u16, u16)>,
    active_rect: Option<Rect>,
) {
    match node {
        Node::Leaf(pane) => {
            let is_active = *cur_path == *active_path;
            // No borders on individual panes — separators are drawn by the
            // parent Split node with tmux-style split coloring.
            let inner = area;
            let target_rows = inner.height.max(1);
            let target_cols = inner.width.max(1);
            if pane.last_rows != target_rows || pane.last_cols != target_cols {
                let _ = pane.master.resize(PtySize { rows: target_rows, cols: target_cols, pixel_width: 0, pixel_height: 0 });
                if let Ok(mut parser) = pane.term.lock() {
                    parser.screen_mut().set_size(target_rows, target_cols);
                }
                pane.last_rows = target_rows;
                pane.last_cols = target_cols;
            }
            let parser_guard = pane.term.lock();
            let Ok(parser) = parser_guard else { return; };
            let screen = parser.screen();
            let (cur_r, cur_c) = screen.cursor_position();
            let mut lines: Vec<Line> = Vec::with_capacity(target_rows as usize);
            for r in 0..target_rows {
                let mut spans: Vec<Span> = Vec::with_capacity(target_cols as usize);
                let mut c = 0;
                while c < target_cols {
                    if let Some(cell) = screen.cell(r, c) {
                        let mut fg = vt_to_color(cell.fgcolor());
                        let mut bg = vt_to_color(cell.bgcolor());
                        if cell.inverse() { std::mem::swap(&mut fg, &mut bg); }
                        if dim_preds && !screen.alternate_screen()
                            && (r > cur_r || (r == cur_r && c >= cur_c))
                        {
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
            f.render_widget(Clear, inner);
            let para = Paragraph::new(Text::from(lines));
            f.render_widget(para, inner);
            if is_active {
                // In copy mode, use copy_pos for cursor; otherwise use PTY cursor.
                let (cr, cc) = copy_cursor.unwrap_or_else(|| screen.cursor_position());
                let cr = cr.min(target_rows.saturating_sub(1));
                let cc = cc.min(target_cols.saturating_sub(1));
                let cx = inner.x + cc;
                let cy = inner.y + cr;
                f.set_cursor(cx, cy);
            }
        }
        Node::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else { vec![(100 / children.len().max(1) as u16); children.len()] };
            let is_horizontal = *kind == LayoutKind::Horizontal;
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            for (i, child) in children.iter_mut().enumerate() {
                cur_path.push(i);
                if i < rects.len() {
                    render_node(f, child, active_path, cur_path, rects[i], dim_preds, border_style, active_border_style, copy_cursor, active_rect);
                }
                cur_path.pop();
            }
            // Draw separator lines — color each cell based on adjacency to active pane rect.
            // When both neighbours are direct leaves, use the midpoint half-highlight
            // so the colored half indicates which side is active.
            let buf = f.buffer_mut();
            for i in 0..children.len().saturating_sub(1) {
                if i >= rects.len() { break; }
                let both_leaves = matches!(&children[i], Node::Leaf(_))
                    && matches!(children.get(i + 1), Some(Node::Leaf(_)));

                if is_horizontal {
                    let sep_x = rects[i].x + rects[i].width;
                    if sep_x < buf.area.x + buf.area.width {
                        if both_leaves {
                            let left_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i;
                            let right_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i + 1;
                            let left_sty = if left_active { active_border_style } else { border_style };
                            let right_sty = if right_active { active_border_style } else { border_style };
                            let mid_y = area.y + area.height / 2;
                            for y in area.y..area.y + area.height {
                                let sty = if y < mid_y { left_sty } else { right_sty };
                                let idx = (y - buf.area.y) as usize * buf.area.width as usize + (sep_x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char('│');
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        } else {
                            for y in area.y..area.y + area.height {
                                let active = active_rect.map_or(false, |ar| {
                                    y >= ar.y && y < ar.y + ar.height
                                    && (sep_x == ar.x + ar.width || sep_x + 1 == ar.x)
                                });
                                let sty = if active { active_border_style } else { border_style };
                                let idx = (y - buf.area.y) as usize * buf.area.width as usize + (sep_x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char('│');
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        }
                    }
                } else {
                    let sep_y = rects[i].y + rects[i].height;
                    if sep_y < buf.area.y + buf.area.height {
                        if both_leaves {
                            let top_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i;
                            let bot_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i + 1;
                            let top_sty = if top_active { active_border_style } else { border_style };
                            let bot_sty = if bot_active { active_border_style } else { border_style };
                            let mid_x = area.x + area.width / 2;
                            for x in area.x..area.x + area.width {
                                let sty = if x < mid_x { top_sty } else { bot_sty };
                                let idx = (sep_y - buf.area.y) as usize * buf.area.width as usize + (x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char('─');
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        } else {
                            for x in area.x..area.x + area.width {
                                let active = active_rect.map_or(false, |ar| {
                                    x >= ar.x && x < ar.x + ar.width
                                    && (sep_y == ar.y + ar.height || sep_y + 1 == ar.y)
                                });
                                let sty = if active { active_border_style } else { border_style };
                                let idx = (sep_y - buf.area.y) as usize * buf.area.width as usize + (x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char('─');
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Compute the rectangle of the active pane by following the active_path through the tree.
fn compute_active_rect(node: &Node, active_path: &[usize], area: Rect) -> Option<Rect> {
    match node {
        Node::Leaf(_) => Some(area),
        Node::Split { kind, sizes, children } => {
            if active_path.is_empty() || children.is_empty() { return None; }
            let idx = active_path[0];
            if idx >= children.len() { return None; }
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1) as u16); children.len()]
            };
            let is_horizontal = *kind == LayoutKind::Horizontal;
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            if idx < rects.len() {
                compute_active_rect(&children[idx], &active_path[1..], rects[idx])
            } else {
                None
            }
        }
    }
}

pub fn expand_status(fmt: &str, app: &AppState, time_str: &str) -> String {
    let mut s = fmt.to_string();
    let window = &app.windows[app.active_idx];
    s = s.replace("#I", &(app.active_idx + app.window_base_index).to_string());
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
                    else if p == "dim" { cur_style = cur_style.add_modifier(Modifier::DIM); }
                    else if p == "italic" || p == "italics" { cur_style = cur_style.add_modifier(Modifier::ITALIC); }
                    else if p == "underline" || p == "underscore" { cur_style = cur_style.add_modifier(Modifier::UNDERLINED); }
                    else if p == "blink" { cur_style = cur_style.add_modifier(Modifier::SLOW_BLINK); }
                    else if p == "reverse" { cur_style = cur_style.add_modifier(Modifier::REVERSED); }
                    else if p == "hidden" { cur_style = cur_style.add_modifier(Modifier::HIDDEN); }
                    else if p == "strikethrough" { cur_style = cur_style.add_modifier(Modifier::CROSSED_OUT); }
                    else if p == "overline" { /* ratatui doesn't support overline natively */ }
                    else if p == "double-underscore" || p == "curly-underscore" || p == "dotted-underscore" || p == "dashed-underscore" {
                        cur_style = cur_style.add_modifier(Modifier::UNDERLINED);
                    }
                    else if p == "default" || p == "none" { cur_style = Style::default(); }
                    else if p == "nobold" { cur_style = cur_style.remove_modifier(Modifier::BOLD); }
                    else if p == "nodim" { cur_style = cur_style.remove_modifier(Modifier::DIM); }
                    else if p == "noitalics" || p == "noitalic" { cur_style = cur_style.remove_modifier(Modifier::ITALIC); }
                    else if p == "nounderline" || p == "nounderscore" { cur_style = cur_style.remove_modifier(Modifier::UNDERLINED); }
                    else if p == "noblink" { cur_style = cur_style.remove_modifier(Modifier::SLOW_BLINK); }
                    else if p == "noreverse" { cur_style = cur_style.remove_modifier(Modifier::REVERSED); }
                    else if p == "nohidden" { cur_style = cur_style.remove_modifier(Modifier::HIDDEN); }
                    else if p == "nostrikethrough" { cur_style = cur_style.remove_modifier(Modifier::CROSSED_OUT); }
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

/// Parse inline `#[fg=...,bg=...,bold]` style directives from pre-expanded text.
/// Unlike `parse_status()`, this does NOT re-expand status variables like `#S`, `%H:%M` etc.
/// Use this for text that has already been expanded by the format engine (e.g., window tab labels).
pub fn parse_inline_styles(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cur_style = base_style;
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some(end) = text[i + 2..].find(']') {
                let token = &text[i + 2..i + 2 + end];
                for part in token.split(',') {
                    let p = part.trim();
                    if p.starts_with("fg=") { cur_style = cur_style.fg(map_color(&p[3..])); }
                    else if p.starts_with("bg=") { cur_style = cur_style.bg(map_color(&p[3..])); }
                    else if p == "bold" { cur_style = cur_style.add_modifier(Modifier::BOLD); }
                    else if p == "dim" { cur_style = cur_style.add_modifier(Modifier::DIM); }
                    else if p == "italic" || p == "italics" { cur_style = cur_style.add_modifier(Modifier::ITALIC); }
                    else if p == "underline" || p == "underscore" { cur_style = cur_style.add_modifier(Modifier::UNDERLINED); }
                    else if p == "blink" { cur_style = cur_style.add_modifier(Modifier::SLOW_BLINK); }
                    else if p == "reverse" { cur_style = cur_style.add_modifier(Modifier::REVERSED); }
                    else if p == "hidden" { cur_style = cur_style.add_modifier(Modifier::HIDDEN); }
                    else if p == "strikethrough" { cur_style = cur_style.add_modifier(Modifier::CROSSED_OUT); }
                    else if p == "overline" { /* ratatui doesn't support overline natively */ }
                    else if p == "double-underscore" || p == "curly-underscore" || p == "dotted-underscore" || p == "dashed-underscore" {
                        cur_style = cur_style.add_modifier(Modifier::UNDERLINED);
                    }
                    else if p == "default" || p == "none" { cur_style = base_style; }
                    else if p == "nobold" { cur_style = cur_style.remove_modifier(Modifier::BOLD); }
                    else if p == "nodim" { cur_style = cur_style.remove_modifier(Modifier::DIM); }
                    else if p == "noitalics" || p == "noitalic" { cur_style = cur_style.remove_modifier(Modifier::ITALIC); }
                    else if p == "nounderline" || p == "nounderscore" { cur_style = cur_style.remove_modifier(Modifier::UNDERLINED); }
                    else if p == "noblink" { cur_style = cur_style.remove_modifier(Modifier::SLOW_BLINK); }
                    else if p == "noreverse" { cur_style = cur_style.remove_modifier(Modifier::REVERSED); }
                    else if p == "nohidden" { cur_style = cur_style.remove_modifier(Modifier::HIDDEN); }
                    else if p == "nostrikethrough" { cur_style = cur_style.remove_modifier(Modifier::CROSSED_OUT); }
                }
                i += 2 + end + 1;
                continue;
            }
        }
        let mut j = i;
        while j < bytes.len() && !(bytes[j] == b'#' && j + 1 < bytes.len() && bytes[j + 1] == b'[') {
            j += 1;
        }
        let chunk = &text[i..j];
        if !chunk.is_empty() {
            spans.push(Span::styled(chunk.to_string(), cur_style));
        }
        i = j;
    }
    spans
}

/// Calculate the visual display width of styled spans (sum of text content widths).
pub fn spans_visual_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.len()).sum()
}

pub fn map_color(name: &str) -> Color {
    let name = name.trim();
    // idx:N (psmux custom)
    if let Some(idx_str) = name.strip_prefix("idx:") {
        if let Ok(idx) = idx_str.parse::<u8>() {
            return Color::Indexed(idx);
        }
    }
    // rgb:R,G,B (psmux custom)
    if let Some(rgb_str) = name.strip_prefix("rgb:") {
        let parts: Vec<&str> = rgb_str.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (parts[0].parse::<u8>(), parts[1].parse::<u8>(), parts[2].parse::<u8>()) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    // #RRGGBB hex
    if let Some(hex_str) = name.strip_prefix('#') {
        if hex_str.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex_str[0..2], 16),
                u8::from_str_radix(&hex_str[2..4], 16),
                u8::from_str_radix(&hex_str[4..6], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    // colour0-colour255 / color0-color255 (tmux primary indexed color format)
    let lower = name.to_lowercase();
    if let Some(idx_str) = lower.strip_prefix("colour").or_else(|| lower.strip_prefix("color")) {
        if let Ok(idx) = idx_str.parse::<u8>() {
            return Color::Indexed(idx);
        }
    }
    match lower.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "brightblack" | "bright-black" => Color::DarkGray,
        "brightred" | "bright-red" => Color::LightRed,
        "brightgreen" | "bright-green" => Color::LightGreen,
        "brightyellow" | "bright-yellow" => Color::LightYellow,
        "brightblue" | "bright-blue" => Color::LightBlue,
        "brightmagenta" | "bright-magenta" => Color::LightMagenta,
        "brightcyan" | "bright-cyan" => Color::LightCyan,
        "brightwhite" | "bright-white" => Color::White,
        "default" | "terminal" => Color::Reset,
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

/// Parse a tmux style string (e.g. "bg=green,fg=black,bold") into a ratatui Style.
/// Used for status-style, pane-border-style, message-style, mode-style, etc.
pub fn parse_tmux_style(style_str: &str) -> Style {
    let mut style = Style::default();
    if style_str.is_empty() { return style; }
    for part in style_str.split(',') {
        let p = part.trim();
        if p.starts_with("fg=") { style = style.fg(map_color(&p[3..])); }
        else if p.starts_with("bg=") { style = style.bg(map_color(&p[3..])); }
        else if p == "bold" { style = style.add_modifier(Modifier::BOLD); }
        else if p == "dim" { style = style.add_modifier(Modifier::DIM); }
        else if p == "italic" || p == "italics" { style = style.add_modifier(Modifier::ITALIC); }
        else if p == "underline" || p == "underscore" { style = style.add_modifier(Modifier::UNDERLINED); }
        else if p == "blink" { style = style.add_modifier(Modifier::SLOW_BLINK); }
        else if p == "reverse" { style = style.add_modifier(Modifier::REVERSED); }
        else if p == "hidden" { style = style.add_modifier(Modifier::HIDDEN); }
        else if p == "strikethrough" { style = style.add_modifier(Modifier::CROSSED_OUT); }
        else if p == "default" || p == "none" { style = Style::default(); }
        else if p == "nobold" { style = style.remove_modifier(Modifier::BOLD); }
        else if p == "nodim" { style = style.remove_modifier(Modifier::DIM); }
        else if p == "noitalics" || p == "noitalic" { style = style.remove_modifier(Modifier::ITALIC); }
        else if p == "nounderline" || p == "nounderscore" { style = style.remove_modifier(Modifier::UNDERLINED); }
        else if p == "noblink" { style = style.remove_modifier(Modifier::SLOW_BLINK); }
        else if p == "noreverse" { style = style.remove_modifier(Modifier::REVERSED); }
        else if p == "nohidden" { style = style.remove_modifier(Modifier::HIDDEN); }
        else if p == "nostrikethrough" { style = style.remove_modifier(Modifier::CROSSED_OUT); }
    }
    style
}
