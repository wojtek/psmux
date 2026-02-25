use std::io::{self, Write, BufRead as _};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::env;

use crossterm::event::{self, Event, KeyEventKind};
use portable_pty::{PtySize, native_pty_system};
use ratatui::prelude::*;
use ratatui::widgets::*;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ratatui::style::{Style, Modifier};
use chrono::Local;

use crate::types::{AppState, CtrlReq, LayoutKind, Mode};
use crate::tree::{active_pane_mut, compute_rects, resize_all_panes, kill_all_children,
    find_window_index_by_id, focus_pane_by_id, focus_pane_by_index, reap_children};
use crate::pane::{create_window, split_active_with_command, kill_active_pane};
use crate::input::{handle_key, handle_mouse, send_text_to_active, send_key_to_active};
use crate::rendering::{render_window, parse_status, centered_rect};
use crate::style::{parse_tmux_style, parse_inline_styles, spans_visual_width};
use crate::config::load_config;
use crate::cli::parse_target;
use crate::copy_mode::{enter_copy_mode, move_copy_cursor, current_prompt_pos, yank_selection,
    capture_active_pane_text, capture_active_pane_range, capture_active_pane_styled};
use crate::layout::dump_layout_json;
use crate::window_ops::{toggle_zoom, remote_mouse_down, remote_mouse_drag, remote_mouse_up,
    remote_mouse_button, remote_mouse_motion, remote_scroll_up, remote_scroll_down};
use crate::util::{list_windows_json, list_tree_json};

pub fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let pty_system = native_pty_system();

    let mut app = AppState::new(
        env::var("PSMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string())
    );
    app.last_window_area = Rect { x: 0, y: 0, width: 0, height: 0 };
    app.attached_clients = 1;

    load_config(&mut app);

    create_window(&*pty_system, &mut app, None)?;

    let (tx, rx) = mpsc::channel::<CtrlReq>();
    app.control_rx = Some(rx);
    let pipe_base = app.port_file_base();
    let first_pipe = crate::pipe::create_server_pipe(&pipe_base, true)?;
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let dir = format!("{}\\.psmux", home);
    let _ = std::fs::create_dir_all(&dir);
    let keypath = format!("{}\\{}.key", dir, app.port_file_base());
    // Generate a session key for auth
    let session_key: String = {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let s = RandomState::new();
        let mut h = s.build_hasher();
        h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64);
        h.write_u64(std::process::id() as u64);
        format!("{:016x}", h.finish())
    };
    let _ = std::fs::write(&keypath, &session_key);
    let pipe_base_clone = pipe_base.clone();
    thread::spawn(move || {
        let mut current_pipe = first_pipe;
        loop {
            if crate::pipe::wait_for_connection(current_pipe).is_err() { break; }
            let mut stream = crate::pipe::PipeStream::from_handle(current_pipe);
            // Create next pipe instance before handling this connection
            match crate::pipe::create_server_pipe(&pipe_base_clone, false) {
                Ok(h) => current_pipe = h,
                Err(_) => break,
            }
            {
                let mut line = String::new();
                let read_stream = stream.try_clone().unwrap();
                let mut r = io::BufReader::new(read_stream);
                let _ = r.read_line(&mut line);
                
                // Check for optional TARGET line (for session:window.pane addressing)
                let mut global_target_win: Option<usize> = None;
                let mut global_target_pane: Option<usize> = None;
                let mut global_pane_is_id = false;
                if line.trim().starts_with("TARGET ") {
                    let target_spec = line.trim().strip_prefix("TARGET ").unwrap_or("");
                    let parsed = parse_target(target_spec);
                    global_target_win = parsed.window;
                    global_target_pane = parsed.pane;
                    global_pane_is_id = parsed.pane_is_id;
                    // Now read the actual command line
                    line.clear();
                    let _ = r.read_line(&mut line);
                }
                
                let mut parts = line.split_whitespace();
                let cmd = parts.next().unwrap_or("");
                // parse optional target specifier
                let args: Vec<&str> = parts.by_ref().collect();
                let mut target_win: Option<usize> = global_target_win;
                let mut target_pane: Option<usize> = global_target_pane;
                let mut pane_is_id = global_pane_is_id;
                let mut start_line: Option<i32> = None;
                let mut end_line: Option<i32> = None;
                let mut i = 0;
                while i < args.len() {
                    if args[i] == "-t" {
                        if let Some(v) = args.get(i+1) {
                            // Parse using parse_target for consistent handling
                            let pt = parse_target(v);
                            if pt.window.is_some() { target_win = pt.window; }
                            if pt.pane.is_some() { 
                                target_pane = pt.pane;
                                pane_is_id = pt.pane_is_id;
                            }
                        }
                        i += 2; continue;
                    } else if args[i] == "-S" {
                        if let Some(v) = args.get(i+1) { if let Ok(n) = v.parse::<i32>() { start_line = Some(n); } }
                        i += 2; continue;
                    } else if args[i] == "-E" {
                        if let Some(v) = args.get(i+1) { if let Ok(n) = v.parse::<i32>() { end_line = Some(n); } }
                        i += 2; continue;
                    }
                    i += 1;
                }
                if let Some(wid) = target_win { let _ = tx.send(CtrlReq::FocusWindow(wid)); }
                if let Some(pid) = target_pane { 
                    if pane_is_id {
                        let _ = tx.send(CtrlReq::FocusPane(pid));
                    } else {
                        let _ = tx.send(CtrlReq::FocusPaneByIndex(pid));
                    }
                }
                match cmd {
                    "new-window" => {
                        let name: Option<String> = args.windows(2).find(|w| w[0] == "-n").map(|w| w[1].trim_matches('"').to_string());
                        let cmd_str: Option<String> = args.iter()
                            .find(|a| !a.starts_with('-') && args.windows(2).all(|w| !(w[0] == "-n" && w[1] == **a)))
                            .map(|s| s.trim_matches('"').to_string());
                        let _ = tx.send(CtrlReq::NewWindow(cmd_str, name, false, None));
                    }
                    "split-window" => {
                        let kind = if args.iter().any(|a| *a == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                        // Parse optional command - find first non-flag argument after flags
                        let cmd_str: Option<String> = args.iter()
                            .find(|a| !a.starts_with('-'))
                            .map(|s| s.trim_matches('"').to_string());
                        let (rtx, _rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::SplitWindow(kind, cmd_str, false, None, None, rtx));
                    }
                    "kill-pane" => { let _ = tx.send(CtrlReq::KillPane); }
                    "capture-pane" => {
                        let escape_seqs = args.iter().any(|a| *a == "-e");
                        let (rtx, rrx) = mpsc::channel::<String>();
                        if escape_seqs {
                            let _ = tx.send(CtrlReq::CapturePaneStyled(rtx, start_line, end_line));
                        } else if start_line.is_some() || end_line.is_some() {
                            let _ = tx.send(CtrlReq::CapturePaneRange(rtx, start_line, end_line));
                        } else {
                            let _ = tx.send(CtrlReq::CapturePane(rtx));
                        }
                        if let Ok(text) = rrx.recv() { let _ = write!(stream, "{}", text); }
                    }
                    "client-attach" => { let _ = tx.send(CtrlReq::ClientAttach); let _ = write!(stream, "ok\n"); }
                    "client-detach" => { let _ = tx.send(CtrlReq::ClientDetach); let _ = write!(stream, "ok\n"); }
                    "session-info" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::SessionInfo(rtx));
                        if let Ok(line) = rrx.recv() { let _ = write!(stream, "{}", line); let _ = stream.flush(); }
                    }
                    _ => {}
                }
            }
        }
    });

    let mut last_resize = Instant::now();
    let mut quit = false;
    loop {
        terminal.draw(|f| {
            let area = f.area();
            let status_at_top = app.status_position == "top";
            let constraints = if status_at_top {
                vec![Constraint::Length(1), Constraint::Min(1)]
            } else {
                vec![Constraint::Min(1), Constraint::Length(1)]
            };
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(area);

            let (content_chunk, status_chunk) = if status_at_top {
                (chunks[1], chunks[0])
            } else {
                (chunks[0], chunks[1])
            };

            app.last_window_area = content_chunk;
            render_window(f, &mut app, content_chunk);

            let _mode_str = match app.mode { 
                Mode::Passthrough => "", 
                Mode::Prefix { .. } => "PREFIX", 
                Mode::CommandPrompt { .. } => ":", 
                Mode::WindowChooser { .. } => "W", 
                Mode::RenamePrompt { .. } => "REN", 
                Mode::RenameSessionPrompt { .. } => "REN-S",
                Mode::CopyMode => "CPY", 
                Mode::CopySearch { .. } => "SEARCH",
                Mode::PaneChooser { .. } => "PANE",
                Mode::MenuMode { .. } => "MENU",
                Mode::PopupMode { .. } => "POPUP",
                Mode::ConfirmMode { .. } => "CONFIRM",
                Mode::ClockMode => "CLOCK",
                Mode::BufferChooser { .. } => "BUF",
            };
            let time_str = Local::now().format("%H:%M").to_string();

            // Parse status-style to get the base status bar style (tmux default: bg=green,fg=black)
            let base_status_style = parse_tmux_style(&app.status_style);
            
            // Expand status-left using the format engine for full format var support
            let expanded_left = crate::format::expand_format(&app.status_left, &app);
            let status_spans = parse_status(&expanded_left, &app, &time_str);
            
            // Expand status-right using the format engine
            let expanded_right = crate::format::expand_format(&app.status_right, &app);
            let mut right_spans = parse_status(&expanded_right, &app, &time_str);

            // Build status bar: left status + window tabs + right-aligned time
            let left_style = if app.status_left_style.is_empty() {
                base_status_style
            } else {
                parse_tmux_style(&app.status_left_style)
            };
            let mut combined: Vec<Span<'static>> = status_spans.into_iter().map(|s| {
                // Apply left style as base, but let inline #[...] overrides win
                if s.style == Style::default() {
                    Span::styled(s.content.into_owned(), left_style)
                } else { s }
            }).collect();
            combined.push(Span::styled(" ".to_string(), base_status_style));

            // Track x position for tab click detection
            let status_x = chunks[1].x;
            let mut cursor_x: u16 = status_x;
            for s in combined.iter() {
                cursor_x += s.content.len() as u16;
            }

            // Parse window-status styles
            let ws_style = if app.window_status_style.is_empty() {
                base_status_style
            } else {
                parse_tmux_style(&app.window_status_style)
            };
            let wsc_style = if app.window_status_current_style.is_empty() {
                // tmux default: no special current style, just same as status
                base_status_style
            } else {
                parse_tmux_style(&app.window_status_current_style)
            };
            let wsa_style = if app.window_status_activity_style.is_empty() {
                base_status_style.add_modifier(Modifier::REVERSED)
            } else {
                parse_tmux_style(&app.window_status_activity_style)
            };
            let wsb_style = if app.window_status_bell_style.is_empty() {
                base_status_style.add_modifier(Modifier::REVERSED)
            } else {
                parse_tmux_style(&app.window_status_bell_style)
            };
            let wsl_style = if app.window_status_last_style.is_empty() {
                base_status_style
            } else {
                parse_tmux_style(&app.window_status_last_style)
            };

            // Render window tabs using window-status-format / window-status-current-format
            let mut tab_pos: Vec<(usize, u16, u16)> = Vec::new();
            let sep = &app.window_status_separator;
            for (i, _w) in app.windows.iter().enumerate() {
                if i > 0 {
                    combined.push(Span::styled(sep.clone(), base_status_style));
                    cursor_x += sep.len() as u16;
                }
                let fmt = if i == app.active_idx {
                    &app.window_status_current_format
                } else {
                    &app.window_status_format
                };
                let label = crate::format::expand_format_for_window(fmt, &app, i);
                
                // Choose style based on window state
                let win = &app.windows[i];
                let fallback_style = if i == app.active_idx {
                    wsc_style
                } else if win.bell_flag {
                    wsb_style
                } else if win.activity_flag {
                    wsa_style
                } else if i == app.last_window_idx {
                    wsl_style
                } else {
                    ws_style
                };
                // Parse inline #[fg=...,bg=...] style directives from theme format strings
                let tab_spans = parse_inline_styles(&label, fallback_style);
                let start_x = cursor_x;
                let visual_w = spans_visual_width(&tab_spans) as u16;
                cursor_x += visual_w;
                tab_pos.push((i, start_x, cursor_x));
                combined.extend(tab_spans);
            }
            app.tab_positions = tab_pos;

            // Right-align the status-right
            let right_style = if app.status_right_style.is_empty() {
                base_status_style
            } else {
                parse_tmux_style(&app.status_right_style)
            };
            combined.push(Span::styled(" ".to_string(), base_status_style));
            for s in right_spans.drain(..) {
                if s.style == Style::default() {
                    combined.push(Span::styled(s.content.into_owned(), right_style));
                } else {
                    combined.push(s);
                }
            }
            let status_bar = Paragraph::new(Line::from(combined)).style(base_status_style);
            f.render_widget(Clear, status_chunk);
            f.render_widget(status_bar, status_chunk);

            // Command prompt — render at bottom (tmux style), not centered popup
            if let Mode::CommandPrompt { input, cursor } = &app.mode {
                let msg_style = parse_tmux_style(&app.message_command_style);
                let prompt_text = format!(":{}", input);
                let prompt_area = status_chunk; // Replace the status bar line
                let para = Paragraph::new(prompt_text).style(msg_style);
                f.render_widget(Clear, prompt_area);
                f.render_widget(para, prompt_area);
                // Place cursor at the right position in the prompt
                let cx = prompt_area.x + 1 + *cursor as u16; // +1 for ':'
                f.set_cursor_position((cx, prompt_area.y));
            }

            if let Mode::WindowChooser { selected, ref tree } = app.mode {
                let mut lines: Vec<Line> = Vec::new();
                for (i, entry) in tree.iter().enumerate() {
                    let marker = if i == selected { ">" } else { " " };
                    if entry.is_session_header {
                        let tag = if entry.is_current_session { " (attached)" } else { "" };
                        lines.push(Line::from(format!("{} {} {}{}",
                            marker,
                            if entry.is_current_session { "▼" } else { "▶" },
                            entry.session_name,
                            tag,
                        )).style(Style::default().fg(Color::Yellow).add_modifier(ratatui::style::Modifier::BOLD)));
                    } else {
                        let active_mark = if entry.is_active_window { "*" } else { " " };
                        let wi = entry.window_index.unwrap_or(0);
                        lines.push(Line::from(format!("{}   {}: {}{} ({} panes) [{}]",
                            marker, wi, entry.window_name, active_mark,
                            entry.window_panes, entry.window_size,
                        )));
                    }
                }
                let height = (lines.len() as u16 + 2).min(20);
                let overlay = Paragraph::new(Text::from(lines)).block(Block::default().borders(Borders::ALL).title("choose-tree"));
                let oa = centered_rect(70, height, area);
                f.render_widget(Clear, oa);
                f.render_widget(overlay, oa);
            }

            if let Mode::BufferChooser { selected } = app.mode {
                let mut lines: Vec<Line> = Vec::new();
                if app.paste_buffers.is_empty() {
                    lines.push(Line::from("  (no buffers)"));
                } else {
                    for (i, buf) in app.paste_buffers.iter().enumerate() {
                        let marker = if i == selected { ">" } else { " " };
                        let preview: String = buf.chars().take(40).map(|c| if c == '\n' { '↵' } else { c }).collect();
                        lines.push(Line::from(format!("{} {:>2}: {:>5} bytes  {}", marker, i, buf.len(), preview)));
                    }
                }
                let height = (lines.len() as u16 + 2).min(15);
                let overlay = Paragraph::new(Text::from(lines)).block(Block::default().borders(Borders::ALL).title("choose-buffer (enter=paste, d=delete, esc=close)"));
                let oa = centered_rect(70, height, area);
                f.render_widget(Clear, oa);
                f.render_widget(overlay, oa);
            }

            if let Mode::RenamePrompt { input } = &app.mode {
                let overlay = Paragraph::new(format!("rename: {}", input)).block(Block::default().borders(Borders::ALL).title("rename window"));
                let oa = centered_rect(60, 3, area);
                f.render_widget(Clear, oa);
                f.render_widget(overlay, oa);
            }

            if let Mode::RenameSessionPrompt { input } = &app.mode {
                let overlay = Paragraph::new(format!("rename: {}", input)).block(Block::default().borders(Borders::ALL).title("rename session"));
                let oa = centered_rect(60, 3, area);
                f.render_widget(Clear, oa);
                f.render_widget(overlay, oa);
            }

            if let Mode::PaneChooser { .. } = &app.mode {
                let win = &app.windows[app.active_idx];
                let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
                compute_rects(&win.root, app.last_window_area, &mut rects);
                for (i, (_, r)) in rects.iter().enumerate() {
                    let n = i + 1;
                    if n > 9 { break; }
                    let bw = 7u16;
                    let bh = 3u16;
                    let bx = r.x + r.width.saturating_sub(bw) / 2;
                    let by = r.y + r.height.saturating_sub(bh) / 2;
                    let b = Rect { x: bx, y: by, width: bw, height: bh };
                    let block = Block::default().borders(Borders::ALL).style(Style::default().bg(Color::Yellow).fg(Color::Black));
                    let inner = block.inner(b);
                    let disp = if n == 10 { 0 } else { n };
                    let line = Line::from(Span::styled(format!(" {} ", disp), Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)));
                    let para = Paragraph::new(line).alignment(Alignment::Center);
                    f.render_widget(Clear, b);
                    f.render_widget(block, b);
                    f.render_widget(para, inner);
                }
            }

            // Render Menu mode
            if let Mode::MenuMode { menu } = &app.mode {
                let item_count = menu.items.len();
                let height = (item_count as u16 + 2).min(20);
                let width = menu.items.iter().map(|i| i.name.len()).max().unwrap_or(10).max(menu.title.len()) as u16 + 8;
                
                // Calculate position based on x/y or center
                let menu_area = if let (Some(x), Some(y)) = (menu.x, menu.y) {
                    let x = if x < 0 { (area.width as i16 + x).max(0) as u16 } else { x as u16 };
                    let y = if y < 0 { (area.height as i16 + y).max(0) as u16 } else { y as u16 };
                    Rect { x: x.min(area.width.saturating_sub(width)), y: y.min(area.height.saturating_sub(height)), width, height }
                } else {
                    centered_rect((width * 100 / area.width.max(1)).max(30), height, area)
                };
                
                let title = if menu.title.is_empty() { "Menu" } else { &menu.title };
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(title);
                
                let mut lines: Vec<Line> = Vec::new();
                for (i, item) in menu.items.iter().enumerate() {
                    if item.is_separator {
                        lines.push(Line::from("─".repeat(width.saturating_sub(2) as usize)));
                    } else {
                        let marker = if i == menu.selected { ">" } else { " " };
                        let key_str = item.key.map(|k| format!("({})", k)).unwrap_or_default();
                        let style = if i == menu.selected {
                            Style::default().bg(Color::Blue).fg(Color::White)
                        } else {
                            Style::default()
                        };
                        lines.push(Line::from(Span::styled(
                            format!("{} {} {}", marker, item.name, key_str),
                            style
                        )));
                    }
                }
                
                let para = Paragraph::new(Text::from(lines)).block(block);
                f.render_widget(Clear, menu_area);
                f.render_widget(para, menu_area);
            }

            // Render Popup mode
            if let Mode::PopupMode { command, output, width, height, ref popup_pty, .. } = &app.mode {
                let w = (*width).min(area.width.saturating_sub(4));
                let h = (*height).min(area.height.saturating_sub(4));
                let popup_area = Rect {
                    x: (area.width.saturating_sub(w)) / 2,
                    y: (area.height.saturating_sub(h)) / 2,
                    width: w,
                    height: h,
                };
                
                let title = if command.is_empty() { "Popup" } else { command };
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow))
                    .title(title);
                
                // If we have a PTY, render its VT output
                let content = if let Some(pty) = popup_pty {
                    if let Ok(parser) = pty.term.lock() {
                        let screen = parser.screen();
                        let inner_h = h.saturating_sub(2);
                        let inner_w = w.saturating_sub(2);
                        let mut lines: Vec<Line<'static>> = Vec::new();
                        for row in 0..inner_h {
                            let mut spans: Vec<Span<'static>> = Vec::new();
                            let mut current_text = String::new();
                            let mut current_style = Style::default();
                            for col in 0..inner_w {
                                if let Some(cell) = screen.cell(row, col) {
                                    let mut style = Style::default();
                                    // Map vt100 colors to ratatui colors
                                    match cell.fgcolor() {
                                        vt100::Color::Default => {}
                                        vt100::Color::Idx(n) => { style = style.fg(Color::Indexed(n)); }
                                        vt100::Color::Rgb(r, g, b) => { style = style.fg(Color::Rgb(r, g, b)); }
                                    }
                                    match cell.bgcolor() {
                                        vt100::Color::Default => {}
                                        vt100::Color::Idx(n) => { style = style.bg(Color::Indexed(n)); }
                                        vt100::Color::Rgb(r, g, b) => { style = style.bg(Color::Rgb(r, g, b)); }
                                    }
                                    if cell.bold() { style = style.add_modifier(Modifier::BOLD); }
                                    if cell.italic() { style = style.add_modifier(Modifier::ITALIC); }
                                    if cell.underline() { style = style.add_modifier(Modifier::UNDERLINED); }
                                    if cell.inverse() { style = style.add_modifier(Modifier::REVERSED); }
                                    let ch = cell.contents();
                                    if style != current_style {
                                        if !current_text.is_empty() {
                                            spans.push(Span::styled(std::mem::take(&mut current_text), current_style));
                                        }
                                        current_style = style;
                                    }
                                    if ch.is_empty() { current_text.push(' '); } else { current_text.push_str(&ch); }
                                } else {
                                    current_text.push(' ');
                                }
                            }
                            if !current_text.is_empty() {
                                spans.push(Span::styled(current_text, current_style));
                            }
                            lines.push(Line::from(spans));
                        }
                        Text::from(lines)
                    } else {
                        Text::from(output.as_str())
                    }
                } else {
                    Text::from(output.as_str())
                };
                
                let para = Paragraph::new(content)
                    .block(block);
                
                f.render_widget(Clear, popup_area);
                f.render_widget(para, popup_area);
            }

            // Render Confirm mode
            if let Mode::ConfirmMode { prompt, input, .. } = &app.mode {
                let width = (prompt.len() as u16 + 10).min(80);
                let confirm_area = centered_rect((width * 100 / area.width.max(1)).max(40), 3, area);
                
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red))
                    .title("Confirm");
                
                let text = format!("{} {}", prompt, input);
                let para = Paragraph::new(text).block(block);
                
                f.render_widget(Clear, confirm_area);
                f.render_widget(para, confirm_area);
            }

            // Render Copy-mode search prompt
            if let Mode::CopySearch { input, forward } = &app.mode {
                let dir = if *forward { "/" } else { "?" };
                let width = (input.len() as u16 + 10).min(80).max(30);
                let search_area = Rect {
                    x: area.x,
                    y: area.y + area.height.saturating_sub(2),
                    width: width.min(area.width),
                    height: 1,
                };
                let text = format!("{}{}", dir, input);
                let para = Paragraph::new(text)
                    .style(Style::default().fg(Color::Yellow).bg(Color::Black));
                f.render_widget(para, search_area);
            }
        })?;

        if let Mode::PaneChooser { opened_at } = &app.mode {
            if opened_at.elapsed() > Duration::from_millis(1500) { app.mode = Mode::Passthrough; }
        }

        if event::poll(Duration::from_millis(20))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                    if handle_key(&mut app, key)? {
                        quit = true;
                    }
                }
                Event::Mouse(me) => {
                    if app.mouse_enabled {
                        let area = app.last_window_area;
                        handle_mouse(&mut app, me, area)?;
                    }
                }
                Event::Resize(cols, rows) => {
                    if last_resize.elapsed() > Duration::from_millis(50) {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = pane.master.resize(PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 });
                            if let Ok(mut parser) = pane.term.lock() {
                                parser.screen_mut().set_size(rows, cols);
                            }
                        }
                        last_resize = Instant::now();
                    }
                }
                _ => {}
            }
        }

        loop {
            let req = if let Some(rx) = app.control_rx.as_ref() { rx.try_recv().ok() } else { None };
            let Some(req) = req else { break; };
            match req {
                CtrlReq::NewWindow(cmd, name, _detached, _start_dir) => {
                    let pty_system = native_pty_system();
                    create_window(&*pty_system, &mut app, cmd.as_deref())?;
                    if let Some(n) = name { app.windows.last_mut().map(|w| w.name = n); }
                    resize_all_panes(&mut app);
                }
                CtrlReq::SplitWindow(k, cmd, _detached, _start_dir, _size_pct, resp) => { let _ = resp.send(if let Err(e) = split_active_with_command(&mut app, k, cmd.as_deref(), None) { format!("{e}") } else { String::new() }); resize_all_panes(&mut app); }
                CtrlReq::KillPane => { let _ = kill_active_pane(&mut app); resize_all_panes(&mut app); }
                CtrlReq::CapturePane(resp) => {
                    if let Some(text) = capture_active_pane_text(&mut app)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneStyled(resp, s, e) => {
                    if let Some(text) = capture_active_pane_styled(&mut app, s, e)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneRange(resp, s, e) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::FocusWindow(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::FocusPane(pid) => { focus_pane_by_id(&mut app, pid); }
                CtrlReq::FocusPaneByIndex(idx) => { focus_pane_by_index(&mut app, idx); }
                CtrlReq::SessionInfo(resp) => {
                    let attached = if app.attached_clients > 0 { "(attached)" } else { "(detached)" };
                    let windows = app.windows.len();
                    let (w,h) = {
                        let win = &mut app.windows[app.active_idx];
                        let mut size = (0,0);
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { size = (p.last_cols as i32, p.last_rows as i32); }
                        size
                    };
                    let created = app.created_at.format("%a %b %e %H:%M:%S %Y");
                    let line = format!("{}: {} windows (created {}) [{}x{}] {}\n", app.session_name, windows, created, w, h, attached);
                    let _ = resp.send(line);
                }
                CtrlReq::ClientAttach => { app.attached_clients = app.attached_clients.saturating_add(1); }
                CtrlReq::ClientDetach => { app.attached_clients = app.attached_clients.saturating_sub(1); }
                CtrlReq::DumpLayout(resp) => {
                    let json = dump_layout_json(&mut app)?;
                    let _ = resp.send(json);
                }
                CtrlReq::SendText(s) => { send_text_to_active(&mut app, &s)?; }
                CtrlReq::SendKey(k) => { send_key_to_active(&mut app, &k)?; }
                CtrlReq::SendPaste(s) => { send_text_to_active(&mut app, &s)?; }
                CtrlReq::ZoomPane => { toggle_zoom(&mut app); }
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); }
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                CtrlReq::CopyYank => { let _ = yank_selection(&mut app); app.mode = Mode::Passthrough; }
                CtrlReq::ClientSize(w, h) => { 
                    app.last_window_area = Rect { x: 0, y: 0, width: w, height: h }; 
                    resize_all_panes(&mut app);
                }
                CtrlReq::FocusPaneCmd(pid) => { focus_pane_by_id(&mut app, pid); }
                CtrlReq::FocusWindowCmd(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::MouseDown(x,y) => { remote_mouse_down(&mut app, x, y); }
                CtrlReq::MouseDownRight(x,y) => { remote_mouse_button(&mut app, x, y, 2, true); }
                CtrlReq::MouseDownMiddle(x,y) => { remote_mouse_button(&mut app, x, y, 1, true); }
                CtrlReq::MouseDrag(x,y) => { remote_mouse_drag(&mut app, x, y); }
                CtrlReq::MouseUp(x,y) => { remote_mouse_up(&mut app, x, y); }
                CtrlReq::MouseUpRight(x,y) => { remote_mouse_button(&mut app, x, y, 2, false); }
                CtrlReq::MouseUpMiddle(x,y) => { remote_mouse_button(&mut app, x, y, 1, false); }
                CtrlReq::MouseMove(x,y) => { remote_mouse_motion(&mut app, x, y); }
                CtrlReq::ScrollUp(x, y) => { remote_scroll_up(&mut app, x, y); }
                CtrlReq::ScrollDown(x, y) => { remote_scroll_down(&mut app, x, y); }
                CtrlReq::NextWindow => { if !app.windows.is_empty() { app.active_idx = (app.active_idx + 1) % app.windows.len(); } }
                CtrlReq::PrevWindow => { if !app.windows.is_empty() { app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); } }
                CtrlReq::RenameWindow(name) => { let win = &mut app.windows[app.active_idx]; win.name = name; }
                CtrlReq::ListWindows(resp) => { let json = list_windows_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ListTree(resp) => { let json = list_tree_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ToggleSync => { app.sync_input = !app.sync_input; }
                CtrlReq::SetPaneTitle(title) => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { p.title = title; }
                }
                CtrlReq::KillServer | CtrlReq::KillSession => {
                    // Kill all child processes and exit
                    for win in app.windows.iter_mut() {
                        kill_all_children(&mut win.root);
                    }
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let keypath = format!("{}\\.psmux\\{}.key", home, app.port_file_base());
                    let _ = std::fs::remove_file(&keypath);
                    std::process::exit(0);
                }
                // For attach mode, we just ignore the new commands - they're handled by the server
                _ => {}
            }
        }

        let (all_empty, any_pruned) = reap_children(&mut app)?;
        if any_pruned {
            resize_all_panes(&mut app);
        }
        if all_empty {
            quit = true;
        }

        if quit { break; }
    }
    // teardown: kill all pane children
    for win in app.windows.iter_mut() {
        kill_all_children(&mut win.root);
    }
    Ok(())
}
