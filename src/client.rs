use std::io::{self, Write, BufRead, BufReader};
use std::time::{Duration, Instant};
use std::env;

use crossterm::event::{self, Event, KeyCode, KeyModifiers, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::layout::LayoutJson;
use crate::util::*;
use crate::session::*;
use crate::rendering::*;
use crate::config::parse_key_string;

pub fn run_remote(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let name = env::var("PSMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string());
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let path = format!("{}\\.psmux\\{}.port", home, name);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no session"))?;
    let addr = format!("127.0.0.1:{}", port);
    let session_key = read_session_key(&name).unwrap_or_default();
    let last_path = format!("{}\\.psmux\\last_session", home);
    let _ = std::fs::write(&last_path, &name);

    // ── Open persistent TCP connection ───────────────────────────────────
    let stream = std::net::TcpStream::connect(&addr)?;
    stream.set_nodelay(true)?; // Disable Nagle's algorithm for low latency
    let mut writer = stream.try_clone()?;
    writer.set_nodelay(true)?;
    let mut reader = BufReader::new(stream);

    // AUTH handshake
    let _ = writer.write_all(format!("AUTH {}\n", session_key).as_bytes());
    let _ = writer.flush();
    let mut auth_line = String::new();
    reader.read_line(&mut auth_line)?;
    if !auth_line.trim().starts_with("OK") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "auth failed"));
    }

    // Enter persistent mode + attach
    let _ = writer.write_all(b"PERSISTENT\n");
    let _ = writer.write_all(b"client-attach\n");
    let _ = writer.flush();

    // Set read timeout for dump-state responses (generous but bounded)
    let _ = reader.get_ref().set_read_timeout(Some(Duration::from_millis(2000)));

    let mut quit = false;
    let mut prefix_armed = false;
    let mut renaming = false;
    let mut rename_buf = String::new();
    let mut pane_renaming = false;
    let mut pane_title_buf = String::new();
    let mut chooser = false;
    let mut choices: Vec<(usize, usize)> = Vec::new();
    let mut tree_chooser = false;
    let mut tree_entries: Vec<(bool, usize, usize, String)> = Vec::new();
    let mut tree_selected: usize = 0;
    let mut session_chooser = false;
    let mut session_entries: Vec<(String, String)> = Vec::new();
    let mut session_selected: usize = 0;
    let current_session = name.clone();
    let mut last_sent_size: (u16, u16) = (0, 0);
    let mut last_event_time = Instant::now();
    let mut last_tree: Vec<WinTree> = Vec::new();
    // Default prefix is Ctrl+B, updated dynamically from server config
    let mut prefix_key: (KeyCode, KeyModifiers) = (KeyCode::Char('b'), KeyModifiers::CONTROL);
    // Precompute the raw control character for the default prefix
    let mut prefix_raw_char: Option<char> = Some('\x02');

    #[derive(serde::Deserialize, Default)]
    struct WinStatus { id: usize, name: String, active: bool }

    #[derive(serde::Deserialize)]
    struct DumpState {
        layout: LayoutJson,
        windows: Vec<WinStatus>,
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        tree: Vec<WinTree>,
    }

    loop {
        // ── STEP 1: Poll events with adaptive timeout ────────────────────
        // Fast polling when typing (1ms), relaxed when idle (16ms ≈ 60fps)
        let since_last = last_event_time.elapsed().as_millis();
        let poll_ms = if since_last < 50 { 1 } else if since_last < 200 { 5 } else { 16 };

        let mut cmd_batch: Vec<String> = Vec::new();
        if event::poll(Duration::from_millis(poll_ms))? {
            last_event_time = Instant::now();
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                        let is_ctrl_q = (matches!(key.code, KeyCode::Char('q')) && key.modifiers.contains(KeyModifiers::CONTROL))
                            || matches!(key.code, KeyCode::Char('\x11'));
                        // Dynamic prefix key check (default: Ctrl+B, configurable via .psmux.conf)
                        let is_prefix = (key.code, key.modifiers) == prefix_key
                            || prefix_raw_char.map_or(false, |c| matches!(key.code, KeyCode::Char(ch) if ch == c));

                        if is_ctrl_q { quit = true; }
                        else if is_prefix { prefix_armed = true; }
                        else if prefix_armed {
                            match key.code {
                                KeyCode::Char('c') => { cmd_batch.push("new-window\n".into()); }
                                KeyCode::Char('%') => { cmd_batch.push("split-window -h\n".into()); }
                                KeyCode::Char('"') => { cmd_batch.push("split-window -v\n".into()); }
                                KeyCode::Char('x') => { cmd_batch.push("kill-pane\n".into()); }
                                KeyCode::Char('&') => { cmd_batch.push("kill-window\n".into()); }
                                KeyCode::Char('z') => { cmd_batch.push("zoom-pane\n".into()); }
                                KeyCode::Char('[') | KeyCode::Char('{') => { cmd_batch.push("copy-enter\n".into()); }
                                KeyCode::Char('n') => { cmd_batch.push("next-window\n".into()); }
                                KeyCode::Char('p') => { cmd_batch.push("previous-window\n".into()); }
                                KeyCode::Char(d) if d.is_ascii_digit() => {
                                    let idx = d.to_digit(10).unwrap() as usize;
                                    cmd_batch.push(format!("select-window {}\n", idx));
                                }
                                KeyCode::Char('o') => { cmd_batch.push("select-pane -t :.+\n".into()); }
                                KeyCode::Up => { cmd_batch.push("select-pane -U\n".into()); }
                                KeyCode::Down => { cmd_batch.push("select-pane -D\n".into()); }
                                KeyCode::Left => { cmd_batch.push("select-pane -L\n".into()); }
                                KeyCode::Right => { cmd_batch.push("select-pane -R\n".into()); }
                                KeyCode::Char('d') => { quit = true; }
                                KeyCode::Char(',') => { renaming = true; rename_buf.clear(); }
                                KeyCode::Char('t') => { pane_renaming = true; pane_title_buf.clear(); }
                                KeyCode::Char('w') => {
                                    tree_chooser = true;
                                    tree_entries.clear();
                                    tree_selected = 0;
                                    // Build tree entries from the last dump-state (no separate TCP needed)
                                    for wi in &last_tree {
                                        tree_entries.push((true, wi.id, 0, wi.name.clone()));
                                        for pi in &wi.panes {
                                            tree_entries.push((false, wi.id, pi.id, pi.title.clone()));
                                        }
                                    }
                                }
                                KeyCode::Char('s') => {
                                    session_chooser = true;
                                    session_entries.clear();
                                    session_selected = 0;
                                    let dir = format!("{}\\.psmux", home);
                                    if let Ok(entries) = std::fs::read_dir(&dir) {
                                        for e in entries.flatten() {
                                            if let Some(fname) = e.file_name().to_str() {
                                                if let Some((base, ext)) = fname.rsplit_once('.') {
                                                    if ext == "port" {
                                                        if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                                            if let Ok(p) = port_str.trim().parse::<u16>() {
                                                                let sess_addr = format!("127.0.0.1:{}", p);
                                                                let sess_key = read_session_key(base).unwrap_or_default();
                                                                let info = if let Ok(mut ss) = std::net::TcpStream::connect_timeout(
                                                                    &sess_addr.parse().unwrap(), Duration::from_millis(25)
                                                                ) {
                                                                    let _ = ss.set_read_timeout(Some(Duration::from_millis(25)));
                                                                    let _ = write!(ss, "AUTH {}\n", sess_key);
                                                                    let _ = ss.write_all(b"session-info\n");
                                                                    let mut br = BufReader::new(ss);
                                                                    let mut al = String::new();
                                                                    let _ = br.read_line(&mut al);
                                                                    let mut line = String::new();
                                                                    if br.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                                                                        line.trim().to_string()
                                                                    } else {
                                                                        format!("{}: (no info)", base)
                                                                    }
                                                                } else {
                                                                    format!("{}: (not responding)", base)
                                                                };
                                                                session_entries.push((base.to_string(), info));
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if session_entries.is_empty() {
                                        session_entries.push((current_session.clone(), format!("{}: (current)", current_session)));
                                    }
                                    for (i, (sname, _)) in session_entries.iter().enumerate() {
                                        if sname == &current_session { session_selected = i; break; }
                                    }
                                }
                                KeyCode::Char('q') => { chooser = true; }
                                KeyCode::Char('v') => { cmd_batch.push("copy-anchor\n".into()); }
                                KeyCode::Char('y') => { cmd_batch.push("copy-yank\n".into()); }
                                _ => {}
                            }
                            prefix_armed = false;
                        } else {
                            match key.code {
                                KeyCode::Up if session_chooser => { if session_selected > 0 { session_selected -= 1; } }
                                KeyCode::Down if session_chooser => { if session_selected + 1 < session_entries.len() { session_selected += 1; } }
                                KeyCode::Enter if session_chooser => {
                                    if let Some((sname, _)) = session_entries.get(session_selected) {
                                        if sname != &current_session {
                                            cmd_batch.push("client-detach\n".into());
                                            env::set_var("PSMUX_SWITCH_TO", sname);
                                            quit = true;
                                        }
                                        session_chooser = false;
                                    }
                                }
                                KeyCode::Esc if session_chooser => { session_chooser = false; }
                                KeyCode::Up if tree_chooser => { if tree_selected > 0 { tree_selected -= 1; } }
                                KeyCode::Down if tree_chooser => { if tree_selected + 1 < tree_entries.len() { tree_selected += 1; } }
                                KeyCode::Enter if tree_chooser => {
                                    if let Some((is_win, wid, pid, _)) = tree_entries.get(tree_selected) {
                                        if *is_win { cmd_batch.push(format!("focus-window {}\n", wid)); }
                                        else { cmd_batch.push(format!("focus-pane {}\n", pid)); }
                                        tree_chooser = false;
                                    }
                                }
                                KeyCode::Esc if tree_chooser => { tree_chooser = false; }
                                KeyCode::Char(c) if renaming && !key.modifiers.contains(KeyModifiers::CONTROL) => { rename_buf.push(c); }
                                KeyCode::Char(c) if pane_renaming && !key.modifiers.contains(KeyModifiers::CONTROL) => { pane_title_buf.push(c); }
                                KeyCode::Backspace if renaming => { let _ = rename_buf.pop(); }
                                KeyCode::Backspace if pane_renaming => { let _ = pane_title_buf.pop(); }
                                KeyCode::Enter if renaming => { cmd_batch.push(format!("rename-window {}\n", rename_buf)); renaming = false; }
                                KeyCode::Enter if pane_renaming => { cmd_batch.push(format!("set-pane-title {}\n", pane_title_buf)); pane_renaming = false; }
                                KeyCode::Esc if renaming => { renaming = false; }
                                KeyCode::Esc if pane_renaming => { pane_renaming = false; }
                                KeyCode::Char(d) if chooser && d.is_ascii_digit() => {
                                    let raw = d.to_digit(10).unwrap() as usize;
                                    let choice = if raw == 0 { 10 } else { raw };
                                    if let Some((_, pid)) = choices.iter().find(|(n, _)| *n == choice) {
                                        cmd_batch.push(format!("focus-pane {}\n", pid));
                                        chooser = false;
                                    }
                                }
                                KeyCode::Esc if chooser => { chooser = false; }
                                KeyCode::Char(' ') => { cmd_batch.push("send-key space\n".into()); }
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::ALT) => {
                                    cmd_batch.push(format!("send-key C-M-{}\n", c.to_ascii_lowercase()));
                                }
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
                                    cmd_batch.push(format!("send-key M-{}\n", c));
                                }
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    cmd_batch.push(format!("send-key C-{}\n", c.to_ascii_lowercase()));
                                }
                                KeyCode::Char(c) if (c as u8) >= 0x01 && (c as u8) <= 0x1A => {
                                    let ctrl_letter = ((c as u8) + b'a' - 1) as char;
                                    cmd_batch.push(format!("send-key C-{}\n", ctrl_letter));
                                }
                                KeyCode::Char(c) => {
                                    let escaped = match c {
                                        '"' => "\\\"".to_string(),
                                        '\\' => "\\\\".to_string(),
                                        _ => c.to_string(),
                                    };
                                    cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                                }
                                KeyCode::Enter => { cmd_batch.push("send-key enter\n".into()); }
                                KeyCode::Tab => { cmd_batch.push("send-key tab\n".into()); }
                                KeyCode::Backspace => { cmd_batch.push("send-key backspace\n".into()); }
                                KeyCode::Delete => { cmd_batch.push("send-key delete\n".into()); }
                                KeyCode::Esc => { cmd_batch.push("send-key esc\n".into()); }
                                KeyCode::Left => { cmd_batch.push("send-key left\n".into()); }
                                KeyCode::Right => { cmd_batch.push("send-key right\n".into()); }
                                KeyCode::Up => { cmd_batch.push("send-key up\n".into()); }
                                KeyCode::Down => { cmd_batch.push("send-key down\n".into()); }
                                KeyCode::PageUp => { cmd_batch.push("send-key pageup\n".into()); }
                                KeyCode::PageDown => { cmd_batch.push("send-key pagedown\n".into()); }
                                KeyCode::Home => { cmd_batch.push("send-key home\n".into()); }
                                KeyCode::End => { cmd_batch.push("send-key end\n".into()); }
                                _ => {}
                            }
                        }
                    }
                    Event::Paste(data) => {
                        let encoded = base64_encode(&data);
                        cmd_batch.push(format!("send-paste {}\n", encoded));
                    }
                    Event::Mouse(me) => {
                        use crossterm::event::{MouseEventKind, MouseButton};
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => { cmd_batch.push(format!("mouse-down {} {}\n", me.column, me.row)); }
                            MouseEventKind::Drag(MouseButton::Left) => { cmd_batch.push(format!("mouse-drag {} {}\n", me.column, me.row)); }
                            MouseEventKind::Up(MouseButton::Left) => { cmd_batch.push(format!("mouse-up {} {}\n", me.column, me.row)); }
                            MouseEventKind::ScrollUp => { cmd_batch.push(format!("scroll-up {} {}\n", me.column, me.row)); }
                            MouseEventKind::ScrollDown => { cmd_batch.push(format!("scroll-down {} {}\n", me.column, me.row)); }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                if quit || !event::poll(Duration::from_millis(0))? { break; }
            }
        }
        if quit { break; }

        // ── STEP 2: Send commands + dump-state on persistent connection ──
        // Send client-size if changed
        {
            let ts = terminal.size()?;
            let new_size = (ts.width, ts.height.saturating_sub(1));
            if new_size != last_sent_size {
                last_sent_size = new_size;
                if writer.write_all(format!("client-size {} {}\n", new_size.0, new_size.1).as_bytes()).is_err() {
                    break; // Connection lost
                }
            }
        }

        // Send all batched commands (fire-and-forget)
        for cmd in &cmd_batch {
            if writer.write_all(cmd.as_bytes()).is_err() {
                break; // Connection lost
            }
        }

        // Send dump-state and flush (server responds with one line of JSON)
        if writer.write_all(b"dump-state\n").is_err() { break; }
        if writer.flush().is_err() { break; }

        // Read one line of JSON response
        let mut buf = String::new();
        match reader.read_line(&mut buf) {
            Ok(0) => break, // EOF - server disconnected
            Err(_) => break, // Error
            Ok(_) => {}
        }
        let state: DumpState = match serde_json::from_str(&buf) {
            Ok(s) => s,
            Err(_) => continue, // Skip frame on parse error
        };

        let root = state.layout;
        let windows = state.windows;
        last_tree = state.tree;

        // Update prefix key from server config (if provided)
        if let Some(ref prefix_str) = state.prefix {
            if let Some((kc, km)) = parse_key_string(prefix_str) {
                if (kc, km) != prefix_key {
                    prefix_key = (kc, km);
                    // Compute raw control character for Ctrl+<letter> prefix
                    prefix_raw_char = if km.contains(KeyModifiers::CONTROL) {
                        if let KeyCode::Char(c) = kc {
                            Some((c as u8 & 0x1f) as char)
                        } else { None }
                    } else { None };
                }
            }
        }

        // ── STEP 3: Render ───────────────────────────────────────────────
        terminal.draw(|f| {
            let area = f.size();
            let chunks = Layout::default().direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)].as_ref()).split(area);

            fn render_json(f: &mut Frame, node: &LayoutJson, area: Rect, dim_preds: bool) {
                match node {
                    LayoutJson::Leaf {
                        id: _,
                        rows: _,
                        cols: _,
                        cursor_row,
                        cursor_col,
                        active,
                        copy_mode,
                        scroll_offset,
                        sel_start_row,
                        sel_start_col,
                        sel_end_row,
                        sel_end_col,
                        content,
                    } => {
                        let pane_block = if *copy_mode && *active {
                            Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Yellow)).title("[copy mode]")
                        } else if *active {
                            Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Green))
                        } else {
                            Block::default().borders(Borders::ALL)
                        };
                        let inner = pane_block.inner(area);
                        let mut lines: Vec<Line> = Vec::new();
                        for r in 0..inner.height.min(content.len() as u16) {
                            let mut spans: Vec<Span> = Vec::new();
                            let row = &content[r as usize];
                            let max_c = inner.width.min(row.len() as u16);
                            let mut c: u16 = 0;
                            while c < max_c {
                                let cell = &row[c as usize];
                                let mut fg = map_color(&cell.fg);
                                let mut bg = map_color(&cell.bg);
                                if cell.inverse { std::mem::swap(&mut fg, &mut bg); }
                                let in_selection = if *copy_mode && *active {
                                    if let (Some(sr), Some(sc), Some(er), Some(ec)) = (sel_start_row, sel_start_col, sel_end_row, sel_end_col) {
                                        r >= *sr && r <= *er && c >= *sc && c <= *ec
                                    } else { false }
                                } else { false };
                                if *active && dim_preds && (r > *cursor_row || (r == *cursor_row && c >= *cursor_col)) {
                                    fg = dim_color(fg);
                                }
                                let mut style = Style::default().fg(fg).bg(bg);
                                if in_selection {
                                    style = style.fg(Color::Black).bg(Color::LightYellow);
                                }
                                if cell.dim { style = style.add_modifier(Modifier::DIM); }
                                if cell.bold { style = style.add_modifier(Modifier::BOLD); }
                                if cell.italic { style = style.add_modifier(Modifier::ITALIC); }
                                if cell.underline { style = style.add_modifier(Modifier::UNDERLINED); }
                                let text = if cell.text.is_empty() { " ".to_string() } else { cell.text.clone() };
                                let char_width = unicode_width::UnicodeWidthStr::width(text.as_str()) as u16;
                                spans.push(Span::styled(text, style));
                                if char_width >= 2 {
                                    c += 2; // skip continuation cell after wide character
                                } else {
                                    c += 1;
                                }
                            }
                            lines.push(Line::from(spans));
                        }
                        f.render_widget(pane_block, area);
                        f.render_widget(Clear, inner);
                        let para = Paragraph::new(Text::from(lines));
                        f.render_widget(para, inner);

                        if *copy_mode && *active && *scroll_offset > 0 {
                            let indicator = format!("[{}/{}]", scroll_offset, scroll_offset);
                            let indicator_width = indicator.len() as u16;
                            if area.width > indicator_width + 2 {
                                let indicator_x = area.x + area.width - indicator_width - 1;
                                let indicator_area = Rect::new(indicator_x, area.y, indicator_width, 1);
                                let indicator_span = Span::styled(indicator, Style::default().fg(Color::Black).bg(Color::Yellow));
                                f.render_widget(Paragraph::new(Line::from(indicator_span)), indicator_area);
                            }
                        }

                        if *active && !*copy_mode {
                            let cy = inner.y + (*cursor_row).min(inner.height.saturating_sub(1));
                            let cx = inner.x + (*cursor_col).min(inner.width.saturating_sub(1));
                            f.set_cursor(cx, cy);
                        }
                    }
                    LayoutJson::Split { kind, sizes, children } => {
                        let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                            sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
                        } else {
                            vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()]
                        };
                        let rects = if kind == "Horizontal" {
                            Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area)
                        } else {
                            Layout::default().direction(Direction::Vertical).constraints(constraints).split(area)
                        };
                        for (i, child) in children.iter().enumerate() { render_json(f, child, rects[i], dim_preds); }
                    }
                }
            }

            let dim_preds = dim_predictions_enabled();
            render_json(f, &root, chunks[0], dim_preds);

            if session_chooser {
                let overlay = Block::default().borders(Borders::ALL).title("choose-session");
                let oa = centered_rect(70, 20, chunks[0]);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let mut lines: Vec<Line> = Vec::new();
                for (i, (sname, info)) in session_entries.iter().enumerate() {
                    let marker = if sname == &current_session { "*" } else { " " };
                    let line = if i == session_selected {
                        Line::from(Span::styled(format!("{} {}", marker, info), Style::default().bg(Color::Yellow).fg(Color::Black)))
                    } else {
                        Line::from(format!("{} {}", marker, info))
                    };
                    lines.push(line);
                }
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, overlay.inner(oa));
            }
            if tree_chooser {
                let overlay = Block::default().borders(Borders::ALL).title("choose-tree");
                let oa = centered_rect(60, 30, chunks[0]);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let mut lines: Vec<Line> = Vec::new();
                for (i, (is_win, wid, pid, name)) in tree_entries.iter().enumerate() {
                    let marker = if *is_win { format!("@{}", wid) } else { format!("%{}", pid) };
                    let prefix = if *is_win { "".to_string() } else { "  ".to_string() };
                    let line = if i == tree_selected {
                        Line::from(Span::styled(format!("{}{} {}", prefix, marker, name), Style::default().bg(Color::Yellow).fg(Color::Black)))
                    } else {
                        Line::from(format!("{}{} {}", prefix, marker, name))
                    };
                    lines.push(line);
                }
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, overlay.inner(oa));
            }
            if chooser {
                let mut rects: Vec<(usize, Rect)> = Vec::new();
                fn rec(node: &LayoutJson, area: Rect, out: &mut Vec<(usize, Rect)>) {
                    match node {
                        LayoutJson::Leaf { id, .. } => { out.push((*id, area)); }
                        LayoutJson::Split { kind, sizes, children } => {
                            let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                                sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
                            } else {
                                vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()]
                            };
                            let rects = if kind == "Horizontal" {
                                Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area)
                            } else {
                                Layout::default().direction(Direction::Vertical).constraints(constraints).split(area)
                            };
                            for (i, child) in children.iter().enumerate() { rec(child, rects[i], out); }
                        }
                    }
                }
                rec(&root, chunks[0], &mut rects);
                choices.clear();
                for (i, (pid, r)) in rects.iter().enumerate() {
                    if i < 10 {
                        choices.push((i + 1, *pid));
                        let bw = 7u16; let bh = 3u16;
                        let bx = r.x + r.width.saturating_sub(bw) / 2;
                        let by = r.y + r.height.saturating_sub(bh) / 2;
                        let b = Rect { x: bx, y: by, width: bw, height: bh };
                        let block = Block::default().borders(Borders::ALL).style(Style::default().bg(Color::Yellow).fg(Color::Black));
                        let inner = block.inner(b);
                        let disp = if i + 1 == 10 { 0 } else { i + 1 };
                        let para = Paragraph::new(Line::from(Span::styled(
                            format!(" {} ", disp),
                            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ))).alignment(Alignment::Center);
                        f.render_widget(Clear, b);
                        f.render_widget(block, b);
                        f.render_widget(para, inner);
                    }
                }
            }
            let mut status_spans: Vec<Span> = vec![
                Span::styled(format!("[{}] ", name), Style::default().fg(Color::Black).bg(Color::Green)),
            ];
            for (i, w) in windows.iter().enumerate() {
                if w.active {
                    status_spans.push(Span::styled(
                        format!("{}: {} ", i, w.name),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    status_spans.push(Span::styled(
                        format!("{}: {} ", i, w.name),
                        Style::default().fg(Color::Black).bg(Color::Green),
                    ));
                }
            }
            let status_bar = Paragraph::new(Line::from(status_spans)).style(Style::default().bg(Color::Green).fg(Color::Black));
            f.render_widget(Clear, chunks[1]);
            f.render_widget(status_bar, chunks[1]);
            if renaming {
                let overlay = Block::default().borders(Borders::ALL).title("rename window");
                let oa = centered_rect(60, 3, chunks[0]);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(format!("name: {}", rename_buf));
                f.render_widget(para, overlay.inner(oa));
            }
            if pane_renaming {
                let overlay = Block::default().borders(Borders::ALL).title("set pane title");
                let oa = centered_rect(60, 3, chunks[0]);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(format!("title: {}", pane_title_buf));
                f.render_widget(para, overlay.inner(oa));
            }
        })?;
    }

    // Clean disconnect on persistent connection
    let _ = writer.write_all(b"client-detach\n");
    let _ = writer.flush();
    Ok(())
}
