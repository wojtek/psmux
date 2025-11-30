use std::io::{self, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use std::net::TcpListener;
use std::io::Read as _;
use std::io::BufRead as _;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use portable_pty::{CommandBuilder, MasterPty, PtySize, PtySystemSelection};
use ratatui::{prelude::*, widgets::*};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use crossterm::terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute};
use crossterm::cursor::{EnableBlinking, DisableBlinking};
use crossterm::event::EnableMouseCapture;
use crossterm::event::DisableMouseCapture;
use ratatui::style::{Style, Modifier};
use unicode_width::UnicodeWidthStr;
use chrono::Local;
use std::env;
use crossterm::style::Print;
use serde::{Serialize, Deserialize};

struct Pane {
    master: Box<dyn MasterPty>,
    child: Box<dyn portable_pty::Child>,
    term: Arc<Mutex<vt100::Parser>>,
    last_rows: u16,
    last_cols: u16,
    id: usize,
    title: String,
}

#[derive(Clone, Copy)]
enum LayoutKind { Horizontal, Vertical }

enum Node {
    Leaf(Pane),
    Split { kind: LayoutKind, sizes: Vec<u16>, children: Vec<Node> },
}

struct Window {
    root: Node,
    active_path: Vec<usize>,
    name: String,
    id: usize,
}

enum Mode {
    Passthrough,
    Prefix { armed_at: Instant },
    CommandPrompt { input: String },
    WindowChooser { selected: usize },
    RenamePrompt { input: String },
    CopyMode,
    PaneChooser { opened_at: Instant },
}

#[derive(Clone, Copy)]
enum FocusDir { Left, Right, Up, Down }

struct AppState {
    windows: Vec<Window>,
    active_idx: usize,
    mode: Mode,
    escape_time_ms: u64,
    prefix_key: (KeyCode, KeyModifiers),
    drag: Option<DragState>,
    last_window_area: Rect,
    mouse_enabled: bool,
    paste_buffers: Vec<String>,
    status_left: String,
    status_right: String,
    copy_anchor: Option<(u16,u16)>,
    copy_pos: Option<(u16,u16)>,
    display_map: Vec<(usize, Vec<usize>)>,
    binds: Vec<Bind>,
    control_rx: Option<mpsc::Receiver<CtrlReq>>,
    control_port: Option<u16>,
    session_name: String,
    attached_clients: usize,
    created_at: chrono::DateTime<Local>,
    next_win_id: usize,
    next_pane_id: usize,
    zoom_saved: Option<Vec<(Vec<usize>, Vec<u16>)>>,
    sync_input: bool,
}

struct DragState {
    split_path: Vec<usize>,
    kind: LayoutKind,
    index: usize,
    start_x: u16,
    start_y: u16,
    left_initial: u16,
    _right_initial: u16,
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn get_program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "pmux".to_string())
        .to_lowercase()
        .replace(".exe", "")
}

fn print_help() {
    let prog = get_program_name();
    println!(r#"{prog} - PowerShell Terminal Multiplexer for Windows

USAGE:
    {prog} [COMMAND] [OPTIONS]

COMMANDS:
    (no command)        Start a new session or attach to existing one
    new-session         Create a new session
        -s <name>       Session name (default: "default")
        -d              Start detached (in background)
    attach, attach-session
                        Attach to an existing session
        -t <name>       Target session name
    ls, list-sessions   List all active sessions
    new-window          Create a new window in current session
    split-window        Split current pane
        -h              Split horizontally (side by side)
        -v              Split vertically (top/bottom, default)
    kill-pane           Close the current pane
    capture-pane        Capture the content of current pane
    server              Run as a server (internal use)
    help                Show this help message
    version             Show version information

OPTIONS:
    -h, --help          Show this help message
    -V, --version       Show version information

KEY BINDINGS (default prefix: Ctrl+B):
    prefix + c          Create new window
    prefix + n          Next window
    prefix + p          Previous window
    prefix + "          Split pane horizontally
    prefix + %          Split pane vertically
    prefix + o          Switch to next pane
    prefix + x          Kill current pane
    prefix + d          Detach from session
    prefix + [          Enter copy mode
    prefix + :          Enter command mode
    prefix + ,          Rename current window
    prefix + w          Window chooser
    prefix + q          Display pane numbers

ENVIRONMENT VARIABLES:
    PMUX_SESSION_NAME       Default session name
    PMUX_DEFAULT_SESSION    Fallback default session name
    PMUX_CURSOR_STYLE       Cursor style (block, underline, bar)
    PMUX_CURSOR_BLINK       Cursor blinking (true/false)

CONFIG FILES:
    ~/.pmux.conf            Main configuration file
    ~/.pmuxrc               Alternative configuration file
    ~/.pmux/pmuxrc          Session directory configuration

EXAMPLES:
    {prog}                          Start or attach to default session
    {prog} new-session -s work      Create a new session named "work"
    {prog} attach -t work           Attach to session "work"
    {prog} ls                       List all sessions
    {prog} split-window -h          Split current pane horizontally

For more information, visit: https://github.com/marlocarlo/pmux
"#, prog = prog);
}

fn print_version() {
    let prog = get_program_name();
    println!("{} {}", prog, VERSION);
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    
    // Handle help and version flags first
    if args.len() > 1 {
        match args[1].as_str() {
            "-h" | "--help" | "help" => {
                print_help();
                return Ok(());
            }
            "-V" | "--version" | "version" => {
                print_version();
                return Ok(());
            }
            _ => {}
        }
    }
    
    if args.len() > 1 {
        match args[1].as_str() {
            "ls" | "list-sessions" => {
                let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                let dir = format!("{}\\.pmux", home);
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        if let Some(name) = e.file_name().to_str() {
                            if let Some((base, ext)) = name.rsplit_once('.') {
                                if ext == "port" {
                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                        if let Ok(_p) = port_str.trim().parse::<u16>() {
                                            let addr = format!("127.0.0.1:{}", port_str.trim());
                                            if let Ok(mut s) = std::net::TcpStream::connect(addr) {
                                                let _ = std::io::Write::write_all(&mut s, b"session-info\n");
                                                let mut br = std::io::BufReader::new(s);
                                                let mut line = String::new();
                                                let _ = br.read_line(&mut line);
                                                if !line.trim().is_empty() { println!("{}", line.trim_end()); } else { println!("{}", base); }
                                            } else { /* stale: skip */ }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(());
            }
            "attach" | "attach-session" => {
                let name = args
                    .iter()
                    .position(|a| a == "-t")
                    .and_then(|i| args.get(i + 1))
                    .map(|s| s.clone())
                    .or_else(resolve_default_session_name)
                    .or_else(resolve_last_session_name)
                    .unwrap_or_else(|| "default".to_string());
                env::set_var("PMUX_SESSION_NAME", name);
                env::set_var("PMUX_REMOTE_ATTACH", "1");
            }
            "server" | "new-session" => {
                let name = args.iter().position(|a| a == "-s").and_then(|i| args.get(i+1)).map(|s| s.clone()).unwrap_or_else(|| "default".to_string());
                let detached = args.iter().any(|a| a == "-d");
                if detached {
                    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("pmux"));
                    let mut cmd = std::process::Command::new(exe);
                    cmd.arg("server").arg("-s").arg(&name);
                    let _child = cmd.spawn().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to spawn server: {e}")))?;
                    return Ok(());
                } else {
                    return run_server(name);
                }
            }
            "new-window" => { send_control("new-window\n".to_string())?; return Ok(()); }
            "split-window" => {
                let flag = if args.iter().any(|a| a == "-h") { "-h" } else { "-v" };
                send_control(format!("split-window {}\n", flag))?; return Ok(());
            }
            "kill-pane" => { send_control("kill-pane\n".to_string())?; return Ok(()); }
            "capture-pane" => {
                let resp = send_control_with_response("capture-pane\n".to_string())?; print!("{}", resp); return Ok(());
            }
            _ => {}
        }
    }
    if env::var("PMUX_ACTIVE").ok().as_deref() == Some("1") {
        eprintln!("pmux: nested sessions are not allowed");
        return Ok(());
    }
    env::set_var("PMUX_ACTIVE", "1");
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableBlinking, EnableMouseCapture)?;
    apply_cursor_style(&mut stdout)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = if env::var("PMUX_REMOTE_ATTACH").ok().as_deref() == Some("1") { run_remote(&mut terminal) } else { run(&mut terminal) };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableBlinking, DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_remote(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let name = env::var("PMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string());
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let path = format!("{}\\.pmux\\{}.port", home, name);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no session"))?;
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect(addr.clone())?;
    let _ = std::io::Write::write_all(&mut stream, b"client-attach\n");
    let last_path = format!("{}\\.pmux\\last_session", home);
    let _ = std::fs::write(&last_path, &name);
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
    loop {
        terminal.draw(|f| {
            let area = f.size();
            let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(1), Constraint::Length(1)].as_ref()).split(area);
            // update server with client size
            let mut cs = std::net::TcpStream::connect(addr.clone()).unwrap();
            let _ = std::io::Write::write_all(&mut cs, format!("client-size {} {}\n", chunks[0].width, chunks[0].height).as_bytes());
            // fetch layout json
            let mut s = std::net::TcpStream::connect(addr.clone()).unwrap();
            let _ = std::io::Write::write_all(&mut s, b"dump-layout\n");
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut s, &mut buf);
            let root: LayoutJson = serde_json::from_str(&buf).unwrap_or(LayoutJson::Leaf { id: 0, rows: 0, cols: 0, cursor_row: 0, cursor_col: 0, content: Vec::new() });

            fn render_json(f: &mut Frame, node: &LayoutJson, area: Rect) {
                match node {
                    LayoutJson::Leaf { id: _, rows: _, cols: _, cursor_row, cursor_col, content } => {
                        let pane_block = Block::default().borders(Borders::ALL);
                        let inner = pane_block.inner(area);
                        let mut lines: Vec<Line> = Vec::new();
                        for r in 0..inner.height.min(content.len() as u16) {
                            let mut spans: Vec<Span> = Vec::new();
                            let row = &content[r as usize];
                            for c in 0..inner.width.min(row.len() as u16) {
                                let cell = &row[c as usize];
                                let mut fg = map_color(&cell.fg);
                                let mut bg = map_color(&cell.bg);
                                if cell.inverse { std::mem::swap(&mut fg, &mut bg); }
                                let mut style = Style::default().fg(fg).bg(bg);
                                if cell.bold { style = style.add_modifier(Modifier::BOLD); }
                                if cell.italic { style = style.add_modifier(Modifier::ITALIC); }
                                if cell.underline { style = style.add_modifier(Modifier::UNDERLINED); }
                                spans.push(Span::styled(cell.text.clone(), style));
                            }
                            lines.push(Line::from(spans));
                        }
                        f.render_widget(pane_block, area);
                        f.render_widget(Clear, inner);
                        let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
                        f.render_widget(para, inner);
                        let cy = inner.y + (*cursor_row).min(inner.height.saturating_sub(1));
                        let cx = inner.x + (*cursor_col).min(inner.width.saturating_sub(1));
                        f.set_cursor(cx, cy);
                    }
                    LayoutJson::Split { kind, sizes, children } => {
                        let constraints: Vec<Constraint> = if sizes.len() == children.len() { sizes.iter().map(|p| Constraint::Percentage(*p)).collect() } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
                        let rects = if kind == "Horizontal" { Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area) } else { Layout::default().direction(Direction::Vertical).constraints(constraints).split(area) };
                        for (i, child) in children.iter().enumerate() { render_json(f, child, rects[i]); }
                    }
                }
            }

            render_json(f, &root, chunks[0]);
            if tree_chooser {
                let overlay = Block::default().borders(Borders::ALL).title("choose-tree");
                let oa = centered_rect(60, 30, chunks[0]);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let mut lines: Vec<Line> = Vec::new();
                for (i, (is_win, wid, pid, name)) in tree_entries.iter().enumerate() {
                    let marker = if *is_win { format!("@{}", wid) } else { format!("%{}", pid) };
                    let prefix = if *is_win { "".to_string() } else { "  ".to_string() };
                    let line = if i == tree_selected { Line::from(Span::styled(format!("{}{} {}", prefix, marker, name), Style::default().bg(Color::Yellow).fg(Color::Black))) } else { Line::from(format!("{}{} {}", prefix, marker, name)) };
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
                            let constraints: Vec<Constraint> = if sizes.len() == children.len() { sizes.iter().map(|p| Constraint::Percentage(*p)).collect() } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
                            let rects = if kind == "Horizontal" { Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area) } else { Layout::default().direction(Direction::Vertical).constraints(constraints).split(area) };
                            for (i, child) in children.iter().enumerate() { rec(child, rects[i], out); }
                        }
                    }
                }
                rec(&root, chunks[0], &mut rects);
                choices.clear();
                for (i,(pid,r)) in rects.iter().enumerate() { if i<10 { choices.push((i+1,*pid)); let bw=7u16; let bh=3u16; let bx=r.x + r.width.saturating_sub(bw)/2; let by=r.y + r.height.saturating_sub(bh)/2; let b=Rect{ x:bx, y:by, width:bw, height:bh }; let block=Block::default().borders(Borders::ALL).style(Style::default().bg(Color::Yellow).fg(Color::Black)); let inner=block.inner(b); let disp=if i+1==10 {0} else {i+1}; let line=Line::from(Span::styled(format!(" {} ",disp), Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD))); let para=Paragraph::new(line).alignment(Alignment::Center); f.render_widget(Clear,b); f.render_widget(block,b); f.render_widget(para,inner);} }
            }
            let status_bar = Paragraph::new(Line::from(vec![Span::raw(format!("session:{}", name))])).style(Style::default().bg(Color::Green).fg(Color::Black));
            f.render_widget(Clear, chunks[1]);
            f.render_widget(status_bar, chunks[1]);
            if pane_renaming {
                let overlay = Block::default().borders(Borders::ALL).title("set pane title");
                let oa = centered_rect(60, 3, chunks[0]);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(format!("title: {}", pane_title_buf));
                f.render_widget(para, overlay.inner(oa));
            }
        })?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? { Event::Key(key) if key.kind == KeyEventKind::Press => {
                if matches!(key.code, KeyCode::Char('q')) && key.modifiers.contains(KeyModifiers::CONTROL) { quit = true; }
                else if matches!(key.code, KeyCode::Char('b')) && key.modifiers.contains(KeyModifiers::CONTROL) { prefix_armed = true; }
                else if prefix_armed {
                    // tmux-like prefix mappings
                    match key.code {
                        KeyCode::Char('c') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"new-window\n"); }
                        KeyCode::Char('%') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"split-window -h\n"); }
                        KeyCode::Char('"') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"split-window -v\n"); }
                        KeyCode::Char('x') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"kill-pane\n"); }
                        KeyCode::Char('z') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"zoom-pane\n"); }
                        KeyCode::Char('[') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-enter\n"); }
                        KeyCode::Char('n') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"next-window\n"); }
                        KeyCode::Char('p') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"previous-window\n"); }
                        KeyCode::Char(',') => { renaming = true; rename_buf.clear(); }
                        KeyCode::Char('t') => { pane_renaming = true; pane_title_buf.clear(); }
                        KeyCode::Char('w') => { tree_chooser = true; tree_entries.clear(); tree_selected = 0; let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"list-tree\n"); let mut buf = String::new(); let _ = std::io::Read::read_to_string(&mut s, &mut buf); let infos: Vec<WinTree> = serde_json::from_str(&buf).unwrap_or(Vec::new()); for wi in infos.into_iter() { tree_entries.push((true, wi.id, 0, wi.name)); for pi in wi.panes.into_iter() { tree_entries.push((false, wi.id, pi.id, pi.title)); } } }
                        KeyCode::Char('s') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"toggle-sync\n"); }
                        KeyCode::Char('q') => { chooser = true; }
                        KeyCode::Left => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-move -1 0\n"); }
                        KeyCode::Right => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-move 1 0\n"); }
                        KeyCode::Up => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-move 0 -1\n"); }
                        KeyCode::Down => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-move 0 1\n"); }
                        KeyCode::Char('v') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-anchor\n"); }
                        KeyCode::Char('y') => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"copy-yank\n"); }
                        _ => {}
                    }
                    prefix_armed = false;
                } else {
                    match key.code {
                        KeyCode::Up if tree_chooser => { if tree_selected>0 { tree_selected-=1; } }
                        KeyCode::Down if tree_chooser => { if tree_selected+1 < tree_entries.len() { tree_selected+=1; } }
                        KeyCode::Enter if tree_chooser => { if let Some((is_win, wid, pid, _)) = tree_entries.get(tree_selected) { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); if *is_win { let _ = std::io::Write::write_all(&mut s, format!("focus-window {}\n", wid).as_bytes()); } else { let _ = std::io::Write::write_all(&mut s, format!("focus-pane {}\n", pid).as_bytes()); } tree_chooser=false; } }
                        KeyCode::Esc if tree_chooser => { tree_chooser=false; }
                        KeyCode::Char(c) if renaming && !key.modifiers.contains(KeyModifiers::CONTROL) => { rename_buf.push(c); }
                        KeyCode::Char(c) if pane_renaming && !key.modifiers.contains(KeyModifiers::CONTROL) => { pane_title_buf.push(c); }
                        KeyCode::Backspace if renaming => { let _ = rename_buf.pop(); }
                        KeyCode::Backspace if pane_renaming => { let _ = pane_title_buf.pop(); }
                        KeyCode::Enter if renaming => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, format!("rename-window {}\n", rename_buf).as_bytes()); renaming=false; }
                        KeyCode::Enter if pane_renaming => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, format!("set-pane-title {}\n", pane_title_buf).as_bytes()); pane_renaming=false; }
                        KeyCode::Esc if renaming => { renaming=false; }
                        KeyCode::Esc if pane_renaming => { pane_renaming=false; }
                        KeyCode::Char(d) if chooser && d.is_ascii_digit() => {
                            let raw = d.to_digit(10).unwrap() as usize;
                            let choice = if raw==0 {10} else {raw};
                            if let Some((_,pid)) = choices.iter().find(|(n,_)| *n==choice) { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, format!("focus-pane {}\n", pid).as_bytes()); chooser=false; }
                        }
                        KeyCode::Esc if chooser => { chooser=false; }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let mut s = std::net::TcpStream::connect(addr.clone()).unwrap();
                            let _ = std::io::Write::write_all(&mut s, format!("send-text {}\n", c).as_bytes());
                        }
                        KeyCode::Enter => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key enter\n"); }
                        KeyCode::Tab => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key tab\n"); }
                        KeyCode::Backspace => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key backspace\n"); }
                        KeyCode::Esc => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key esc\n"); }
                        KeyCode::Left => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key left\n"); }
                        KeyCode::Right => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key right\n"); }
                        KeyCode::Up => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key up\n"); }
                        KeyCode::Down => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"send-key down\n"); }
                        _ => {}
                    }
                }
            } Event::Mouse(me) => {
                use crossterm::event::{MouseEventKind,MouseButton};
                match me.kind {
                    MouseEventKind::Down(MouseButton::Left) => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, format!("mouse-down {} {}\n", me.column, me.row).as_bytes()); }
                    MouseEventKind::Drag(MouseButton::Left) => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, format!("mouse-drag {} {}\n", me.column, me.row).as_bytes()); }
                    MouseEventKind::Up(MouseButton::Left) => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, format!("mouse-up {} {}\n", me.column, me.row).as_bytes()); }
                    MouseEventKind::ScrollUp => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"scroll-up\n"); }
                    MouseEventKind::ScrollDown => { let mut s = std::net::TcpStream::connect(addr.clone()).unwrap(); let _ = std::io::Write::write_all(&mut s, b"scroll-down\n"); }
                    _ => {}
                }
            } _ => {} }
        }
        if reap_children_placeholder()? { /* no-op */ }
        if quit { break; }
    }
    let _ = std::net::TcpStream::connect(addr).and_then(|mut s| { std::io::Write::write_all(&mut s, b"client-detach\n"); Ok(()) });
    Ok(())
}

fn reap_children_placeholder() -> io::Result<bool> { Ok(false) }

fn send_control(line: String) -> io::Result<()> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let target = env::var("PMUX_TARGET_SESSION").ok().unwrap_or_else(|| "default".to_string());
    let path = format!("{}\\.pmux\\{}.port", home, target);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no session"))?;
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect(addr)?;
    let _ = write!(stream, "{}", line);
    Ok(())
}

fn send_control_with_response(line: String) -> io::Result<String> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let target = env::var("PMUX_TARGET_SESSION").ok().unwrap_or_else(|| "default".to_string());
    let path = format!("{}\\.pmux\\{}.port", home, target);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no session"))?;
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect(addr)?;
    let _ = write!(stream, "{}", line);
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut stream, &mut buf);
    Ok(buf)
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let pty_system = PtySystemSelection::default()
        .get()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;

    let mut app = AppState {
        windows: Vec::new(),
        active_idx: 0,
        mode: Mode::Passthrough,
        escape_time_ms: 500,
        prefix_key: (KeyCode::Char('b'), KeyModifiers::CONTROL),
        drag: None,
        last_window_area: Rect { x: 0, y: 0, width: 0, height: 0 },
        mouse_enabled: true,
        paste_buffers: Vec::new(),
        status_left: "pmux:#I".to_string(),
        status_right: "%H:%M".to_string(),
        copy_anchor: None,
        copy_pos: None,
        display_map: Vec::new(),
        binds: Vec::new(),
        control_rx: None,
        control_port: None,
        session_name: env::var("PMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string()),
        attached_clients: 1,
        created_at: Local::now(),
        next_win_id: 1,
        next_pane_id: 1,
        zoom_saved: None,
        sync_input: false,
    };

    load_config(&mut app);

    create_window(&*pty_system, &mut app)?;

    let (tx, rx) = mpsc::channel::<CtrlReq>();
    app.control_rx = Some(rx);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    app.control_port = Some(port);
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let dir = format!("{}\\.pmux", home);
    let _ = std::fs::create_dir_all(&dir);
    let regpath = format!("{}\\{}.port", dir, app.session_name);
    let _ = std::fs::write(&regpath, port.to_string());
    thread::spawn(move || {
        for conn in listener.incoming() {
            if let Ok(mut stream) = conn {
                let mut line = String::new();
                let mut r = io::BufReader::new(stream.try_clone().unwrap());
                let _ = r.read_line(&mut line);
                let mut parts = line.split_whitespace();
                let cmd = parts.next().unwrap_or("");
                // parse optional target specifier
                let mut args: Vec<&str> = parts.by_ref().collect();
                let mut target_win: Option<usize> = None;
                let mut target_pane: Option<usize> = None;
                let mut start_line: Option<u16> = None;
                let mut end_line: Option<u16> = None;
                let mut i = 0;
                while i < args.len() {
                    if args[i] == "-t" {
                        if let Some(v) = args.get(i+1) {
                            if v.starts_with('%') { if let Ok(pid) = v[1..].parse::<usize>() { target_pane = Some(pid); } }
                            else if v.starts_with('@') { if let Ok(wid) = v[1..].parse::<usize>() { target_win = Some(wid); } }
                        }
                        i += 2; continue;
                    } else if args[i] == "-S" {
                        if let Some(v) = args.get(i+1) { if let Ok(n) = v.parse::<u16>() { start_line = Some(n); } }
                        i += 2; continue;
                    } else if args[i] == "-E" {
                        if let Some(v) = args.get(i+1) { if let Ok(n) = v.parse::<u16>() { end_line = Some(n); } }
                        i += 2; continue;
                    }
                    i += 1;
                }
                if let Some(wid) = target_win { let _ = tx.send(CtrlReq::FocusWindow(wid)); }
                if let Some(pid) = target_pane { let _ = tx.send(CtrlReq::FocusPane(pid)); }
                match cmd {
                    "new-window" => { let _ = tx.send(CtrlReq::NewWindow); }
                    "split-window" => {
                        let kind = if args.iter().any(|a| *a == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                        let _ = tx.send(CtrlReq::SplitWindow(kind));
                    }
                    "kill-pane" => { let _ = tx.send(CtrlReq::KillPane); }
                    "capture-pane" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        if start_line.is_some() || end_line.is_some() { let _ = tx.send(CtrlReq::CapturePaneRange(rtx, start_line, end_line)); }
                        else { let _ = tx.send(CtrlReq::CapturePane(rtx)); }
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
            let area = f.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)].as_ref())
                .split(area);

            app.last_window_area = chunks[0];
            render_window(f, &mut app, chunks[0]);

            let _mode_str = match app.mode { Mode::Passthrough => "", Mode::Prefix { .. } => "PREFIX", Mode::CommandPrompt { .. } => ":", Mode::WindowChooser { .. } => "W", Mode::RenamePrompt { .. } => "REN", Mode::CopyMode => "CPY", Mode::PaneChooser { .. } => "PANE" };
            let time_str = Local::now().format("%H:%M").to_string();
            let mut windows_list = String::new();
            for (i, _) in app.windows.iter().enumerate() {
                if i == app.active_idx { windows_list.push_str(&format!(" #[{}]", i+1)); } else { windows_list.push_str(&format!(" {}", i+1)); }
            }
            let status_spans = parse_status(&app.status_left, &app, &time_str);
            let mut right_spans = parse_status(&app.status_right, &app, &time_str);
            let mut combined: Vec<Span<'static>> = status_spans;
            combined.push(Span::raw(" "));
            combined.append(&mut right_spans);
            let status_bar = Paragraph::new(Line::from(combined)).style(Style::default().bg(Color::Green).fg(Color::Black));
            f.render_widget(Clear, chunks[1]);
            f.render_widget(status_bar, chunks[1]);

            if let Mode::CommandPrompt { input } = &app.mode {
                let overlay = Paragraph::new(format!(":{}", input)).block(Block::default().borders(Borders::ALL).title("command"));
                let oa = centered_rect(80, 3, area);
                f.render_widget(Clear, oa);
                f.render_widget(overlay, oa);
            }

            if let Mode::WindowChooser { selected } = app.mode {
                let mut lines: Vec<Line> = Vec::new();
                for (i,w) in app.windows.iter().enumerate() {
                    let marker = if i == selected { ">" } else { " " };
                    lines.push(Line::from(format!("{} [{}] {}", marker, i+1, w.name)));
                }
                let overlay = Paragraph::new(Text::from(lines)).block(Block::default().borders(Borders::ALL).title("windows"));
                let oa = centered_rect(60, (app.windows.len() as u16 + 2).min(10), area);
                f.render_widget(Clear, oa);
                f.render_widget(overlay, oa);
            }

            if let Mode::RenamePrompt { input } = &app.mode {
                let overlay = Paragraph::new(format!("rename: {}", input)).block(Block::default().borders(Borders::ALL).title("rename window"));
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
        })?;

        if let Mode::PaneChooser { opened_at } = &app.mode {
            if opened_at.elapsed() > Duration::from_millis(1500) { app.mode = Mode::Passthrough; }
        }

        if event::poll(Duration::from_millis(20))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if handle_key(&mut app, key)? {
                        quit = true;
                    }
                }
                Event::Mouse(me) => {
                    let area = app.last_window_area;
                    handle_mouse(&mut app, me, area)?;
                }
                Event::Resize(cols, rows) => {
                    if last_resize.elapsed() > Duration::from_millis(50) {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = pane.master.resize(PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 });
                            let mut parser = pane.term.lock().unwrap();
                            parser.screen_mut().set_size(rows, cols);
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
                CtrlReq::NewWindow => {
                    let pty_system = PtySystemSelection::default().get().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
                    create_window(&*pty_system, &mut app)?;
                }
                CtrlReq::SplitWindow(k) => { let _ = split_active(&mut app, k); }
                CtrlReq::KillPane => { let _ = kill_active_pane(&mut app); }
                CtrlReq::CapturePane(resp) => {
                    if let Some(text) = capture_active_pane_text(&mut app)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneRange(resp, s, e) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::FocusWindow(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::FocusPane(pid) => { focus_pane_by_id(&mut app, pid); }
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
                CtrlReq::ZoomPane => { toggle_zoom(&mut app); }
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); }
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                CtrlReq::CopyYank => { let _ = yank_selection(&mut app); app.mode = Mode::Passthrough; }
                CtrlReq::ClientSize(w, h) => { app.last_window_area = Rect { x: 0, y: 0, width: w, height: h }; }
                CtrlReq::FocusPaneCmd(pid) => { focus_pane_by_id(&mut app, pid); }
                CtrlReq::FocusWindowCmd(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::MouseDown(x,y) => { remote_mouse_down(&mut app, x, y); }
                CtrlReq::MouseDrag(x,y) => { remote_mouse_drag(&mut app, x, y); }
                CtrlReq::MouseUp(_,_) => { app.drag = None; }
                CtrlReq::ScrollUp => { remote_scroll_up(&mut app); }
                CtrlReq::ScrollDown => { remote_scroll_down(&mut app); }
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
            }
        }

        if reap_children(&mut app)? {
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

fn create_window(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState) -> io::Result<()> {
    let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;

    let shell_cmd = detect_shell();
    let child = pair
        .slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;

    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 0)));
    let term_reader = term.clone();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    thread::spawn(move || {
        let mut local = [0u8; 8192];
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => {
                    let mut parser = term_reader.lock().unwrap();
                    parser.process(&local[..n]);
                }
                Ok(_) => thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    });

    let pane = Pane { master: pair.master, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id) };
    app.next_pane_id += 1;
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: format!("win {}", app.windows.len()+1), id: app.next_win_id });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    Ok(())
}

fn handle_key(app: &mut AppState, key: KeyEvent) -> io::Result<bool> {
    if matches!(key.code, KeyCode::Char('q')) && key.modifiers.contains(KeyModifiers::CONTROL) {
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
            forward_key_to_active(app, key)?;
            Ok(false)
        }
        Mode::Prefix { armed_at } => {
            let elapsed = armed_at.elapsed().as_millis() as u64;
            let handled = match key.code {
                KeyCode::Left => { move_focus(app, FocusDir::Left); true }
                KeyCode::Right => { move_focus(app, FocusDir::Right); true }
                KeyCode::Up => { move_focus(app, FocusDir::Up); true }
                KeyCode::Down => { move_focus(app, FocusDir::Down); true }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let idx = d.to_digit(10).unwrap() as usize;
                    if idx > 0 && idx <= app.windows.len() { app.active_idx = idx - 1; }
                    true
                }
                KeyCode::Char('c') => {
                    let pty_system = PtySystemSelection::default()
                        .get()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
                    create_window(&*pty_system, app)?;
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
                    kill_active_pane(app)?;
                    true
                }
                KeyCode::Char('d') => {
                    // detach: exit pmux cleanly
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
                _ => false,
            };

            if matches!(app.mode, Mode::Prefix { .. }) {
                if !handled && elapsed < app.escape_time_ms {
                    // Unrecognized after prefix: do not send '^B'; swallow and return
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
        Mode::CopyMode => {
            match key.code {
                KeyCode::Esc | KeyCode::Char(']') => { app.mode = Mode::Passthrough; app.copy_anchor = None; app.copy_pos = None; }
                KeyCode::Left => { move_copy_cursor(app, -1, 0); }
                KeyCode::Right => { move_copy_cursor(app, 1, 0); }
                KeyCode::Up => { move_copy_cursor(app, 0, -1); }
                KeyCode::Down => { move_copy_cursor(app, 0, 1); }
                KeyCode::Char('v') => { if let Some((r,c)) = current_prompt_pos(app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                KeyCode::Char('y') => { yank_selection(app)?; app.mode = Mode::Passthrough; }
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
    }
}

fn move_focus(app: &mut AppState, dir: FocusDir) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    // find active index
    let mut active_idx = None;
    for (i, (path, _)) in rects.iter().enumerate() { if *path == win.active_path { active_idx = Some(i); break; } }
    let Some(ai) = active_idx else { return; };
    let (_, arect) = &rects[ai];
    // pick nearest neighbor in direction
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

fn forward_key_to_active(app: &mut AppState, key: KeyEvent) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    let Some(active) = active_pane_mut(&mut win.root, &win.active_path) else { return Ok(()); };
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = write!(active.master, "{}", c);
        }
        KeyCode::Enter => { let _ = write!(active.master, "\r"); }
        KeyCode::Tab => { let _ = write!(active.master, "\t"); }
        KeyCode::Backspace => { let _ = write!(active.master, "\x08"); }
        KeyCode::Esc => { let _ = write!(active.master, "\x1b"); }
        KeyCode::Left => { let _ = write!(active.master, "\x1b[D"); }
        KeyCode::Right => { let _ = write!(active.master, "\x1b[C"); }
        KeyCode::Up => { let _ = write!(active.master, "\x1b[A"); }
        KeyCode::Down => { let _ = write!(active.master, "\x1b[B"); }
        _ => {}
    }
    Ok(())
}

fn split_active(app: &mut AppState, kind: LayoutKind) -> io::Result<()> {
    let pty_system = PtySystemSelection::default().get().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
    let size = PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    let shell_cmd = detect_shell();
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 0)));
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
    let new_leaf = Node::Leaf(Pane { master: pair.master, child, term, last_rows: size.rows, last_cols: size.cols, id: app.next_pane_id, title: format!("pane %{}", app.next_pane_id) });
    app.next_pane_id += 1;
    let win = &mut app.windows[app.active_idx];
    replace_leaf_with_split(&mut win.root, &win.active_path, kind, new_leaf);
    // focus the newly created right child of the split
    let mut new_path = win.active_path.clone();
    new_path.push(1);
    win.active_path = new_path;
    Ok(())
}

fn handle_mouse(app: &mut AppState, me: crossterm::event::MouseEvent, window_area: Rect) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    // compute leaf rects from split tree
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, window_area, &mut rects);
    // compute borders for splits
    let mut borders: Vec<(Vec<usize>, LayoutKind, usize, u16)> = Vec::new();
    compute_split_borders(&win.root, window_area, &mut borders);

    use crossterm::event::{MouseEventKind, MouseButton};
    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // focus pane
            for (path, area) in rects.iter() {
                if area.contains(ratatui::layout::Position { x: me.column, y: me.row }) { win.active_path = path.clone(); }
            }
            // check border resize start
            let tol = 1u16;
            for (path, kind, idx, pos) in borders.iter() {
                match kind {
                    LayoutKind::Horizontal => {
                        if me.column >= pos.saturating_sub(tol) && me.column <= pos + tol {
                            // record initial sizes from split node
                            if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) {
                                app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: me.column, start_y: me.row, left_initial: left, _right_initial: right });
                            }
                            break;
                        }
                    }
                    LayoutKind::Vertical => {
                        if me.row >= pos.saturating_sub(tol) && me.row <= pos + tol {
                            if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) {
                                app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: me.column, start_y: me.row, left_initial: left, _right_initial: right });
                            }
                            break;
                        }
                    }
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(d) = &app.drag {
                adjust_split_sizes(&mut win.root, d, me.column, me.row);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => { app.drag = None; }
        MouseEventKind::ScrollUp => {
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(active.master, "\x1b[A"); }
        }
        MouseEventKind::ScrollDown => {
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(active.master, "\x1b[B"); }
        }
        _ => {}
    }
    Ok(())
}

// TODO: implement split-border detection and per-node size adjustment

fn kill_active_pane(app: &mut AppState) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    kill_leaf(&mut win.root, &win.active_path);
    Ok(())
}

fn detect_shell() -> CommandBuilder {
    let pwsh = which::which("pwsh").ok().map(|p| p.to_string_lossy().into_owned());
    let cmd = which::which("cmd").ok().map(|p| p.to_string_lossy().into_owned());
    match pwsh.or(cmd) {
        Some(path) => CommandBuilder::new(path),
        None => CommandBuilder::new("pwsh.exe"),
    }
}

fn execute_command_prompt(app: &mut AppState) -> io::Result<()> {
    let cmdline = match &app.mode { Mode::CommandPrompt { input } => input.clone(), _ => String::new() };
    app.mode = Mode::Passthrough;
    let parts: Vec<&str> = cmdline.split_whitespace().collect();
    if parts.is_empty() { return Ok(()); }
    match parts[0] {
        "new-window" => {
            let pty_system = PtySystemSelection::default().get().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;
            create_window(&*pty_system, app)?;
        }
        "split-window" => {
            let kind = if parts.iter().any(|p| *p == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
            split_active(app, kind)?;
        }
        "kill-pane" => { kill_active_pane(app)?; }
        "capture-pane" => { capture_active_pane(app)?; }
        "save-buffer" => { if let Some(file) = parts.get(1) { save_latest_buffer(app, file)?; } }
        "list-sessions" => { println!("default"); }
        "attach-session" => { /* already attached */ }
        "next-window" => { app.active_idx = (app.active_idx + 1) % app.windows.len(); }
        "previous-window" => { app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); }
        "select-window" => {
            if let Some(tidx) = parts.iter().position(|p| *p == "-t").and_then(|i| parts.get(i+1)) { if let Ok(n) = tidx.parse::<usize>() { if n>0 && n<=app.windows.len() { app.active_idx = n-1; } } }
        }
        _ => {}
    }
    Ok(())
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
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

fn reap_children(app: &mut AppState) -> io::Result<bool> {
    // Transform each window's split tree by pruning exited leaves.
    for i in (0..app.windows.len()).rev() {
        let root = std::mem::replace(&mut app.windows[i].root, Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
        match prune_exited(root) {
            Some(new_root) => {
                app.windows[i].root = new_root;
                // ensure active_path remains valid; if not, pick first available leaf
                if !path_exists(&app.windows[i].root, &app.windows[i].active_path) {
                    app.windows[i].active_path = first_leaf_path(&app.windows[i].root);
                }
            }
            None => { app.windows.remove(i); }
        }
    }
    Ok(app.windows.is_empty())
}

fn vt_to_color(c: vt100::Color) -> Color {
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
            7 => Color::Gray,
            8 => Color::DarkGray,
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

fn apply_cursor_style<W: Write>(out: &mut W) -> io::Result<()> {
    let style = env::var("PMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string());
    let blink = env::var("PMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0";
    let code = match style.as_str() {
        "block" => if blink { 1 } else { 2 },
        "underline" => if blink { 3 } else { 4 },
        "bar" | "beam" => if blink { 5 } else { 6 },
        _ => if blink { 5 } else { 6 },
    };
    execute!(out, Print(format!("\x1b[{} q", code)))?;
    Ok(())
}

fn render_window(f: &mut Frame, app: &mut AppState, area: Rect) {
    let win = &mut app.windows[app.active_idx];
    render_node(f, &mut win.root, &win.active_path, &mut Vec::new(), area);
}

fn enter_copy_mode(app: &mut AppState) { app.mode = Mode::CopyMode; }

fn cycle_top_layout(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    // toggle parent of active path, else toggle root
    if !win.active_path.is_empty() {
        let parent_path = &win.active_path[..win.active_path.len()-1].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path.to_vec()) {
            *kind = match *kind { LayoutKind::Horizontal => LayoutKind::Vertical, LayoutKind::Vertical => LayoutKind::Horizontal };
            *sizes = vec![50,50];
        }
    } else {
        if let Node::Split { kind, sizes, .. } = &mut win.root { *kind = match *kind { LayoutKind::Horizontal => LayoutKind::Vertical, LayoutKind::Vertical => LayoutKind::Horizontal }; *sizes = vec![50,50]; }
    }
}

fn render_node(f: &mut Frame, node: &mut Node, active_path: &Vec<usize>, cur_path: &mut Vec<usize>, area: Rect) {
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
            let mut lines: Vec<Line> = Vec::with_capacity(target_rows as usize);
            for r in 0..target_rows {
                let mut spans: Vec<Span> = Vec::with_capacity(target_cols as usize);
                let mut c = 0;
                while c < target_cols {
                    if let Some(cell) = screen.cell(r, c) {
                        let mut fg = vt_to_color(cell.fgcolor());
                        let mut bg = vt_to_color(cell.bgcolor());
                        if cell.inverse() { std::mem::swap(&mut fg, &mut bg); }
                        let mut style = Style::default().fg(fg).bg(bg);
                        if cell.bold() { style = style.add_modifier(Modifier::BOLD); }
                        if cell.italic() { style = style.add_modifier(Modifier::ITALIC); }
                        if cell.underline() { style = style.add_modifier(Modifier::UNDERLINED); }
                        let text = cell.contents().to_string();
                        let w = UnicodeWidthStr::width(text.as_str()) as u16;
                        if w == 0 {
                            spans.push(Span::styled(" ", style));
                            c += 1;
                        } else if w >= 2 && c + 1 < target_cols {
                            spans.push(Span::styled(text, style));
                            spans.push(Span::styled(" ", style));
                            c += 2;
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
            let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
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

fn active_pane_mut<'a>(node: &'a mut Node, path: &Vec<usize>) -> Option<&'a mut Pane> {
    let mut cur = node;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => { cur = children.get_mut(idx)?; }
            Node::Leaf(_) => return None,
        }
    }
    match cur { Node::Leaf(p) => Some(p), _ => None }
}

fn replace_leaf_with_split(node: &mut Node, path: &Vec<usize>, kind: LayoutKind, new_leaf: Node) {
    if path.is_empty() {
        let old = std::mem::replace(node, Node::Split { kind, sizes: vec![50,50], children: vec![] });
        if let Node::Split { children, .. } = node { children.push(old); children.push(new_leaf); }
        return;
    }
    let mut cur = node;
    for (depth, &idx) in path.iter().enumerate() {
        match cur {
            Node::Split { children, .. } => {
                if depth == path.len()-1 {
                    let leaf = std::mem::replace(&mut children[idx], Node::Split { kind, sizes: vec![50,50], children: vec![] });
                    if let Node::Split { children: c, .. } = &mut children[idx] { c.push(leaf); c.push(new_leaf); }
                    return;
                } else { cur = &mut children[idx]; }
            }
            Node::Leaf(_) => return,
        }
    }
}

fn kill_leaf(node: &mut Node, path: &Vec<usize>) {
    *node = remove_node(std::mem::replace(node, Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] }), path);
}

fn remove_node(n: Node, path: &Vec<usize>) -> Node {
    match n {
        Node::Leaf(p) => {
            // if path points here, removing leaf yields an empty split collapse handled by parent; return leaf
            Node::Leaf(p)
        }
        Node::Split { kind, sizes, children } => {
            if path.is_empty() { return Node::Split { kind, sizes, children }; }
            let idx = path[0];
            let mut new_children: Vec<Node> = Vec::new();
            for (i, child) in children.into_iter().enumerate() {
                if i == idx {
                    if path.len() > 1 { new_children.push(remove_node(child, &path[1..].to_vec())); }
                    // else: drop this child (removed)
                } else { new_children.push(child); }
            }
            if new_children.len() == 1 { new_children.into_iter().next().unwrap() }
            else {
                // normalize sizes equally
                let mut eq = vec![100 / new_children.len() as u16; new_children.len()];
                let rem = 100 - eq.iter().sum::<u16>();
                if let Some(last) = eq.last_mut() { *last += rem; }
                Node::Split { kind, sizes: eq, children: new_children }
            }
        }
    }
}

fn compute_rects(node: &Node, area: Rect, out: &mut Vec<(Vec<usize>, Rect)>) {
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, Rect)>) {
        match node {
            Node::Leaf(_) => { out.push((path.clone(), area)); }
            Node::Split { kind, sizes, children } => {
                let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                    sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
                } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
                let rects = match *kind {
                    LayoutKind::Horizontal => Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area),
                    LayoutKind::Vertical => Layout::default().direction(Direction::Vertical).constraints(constraints).split(area),
                };
                for (i, child) in children.iter().enumerate() { path.push(i); rec(child, rects[i], path, out); path.pop(); }
            }
        }
    }
    let mut path = Vec::new();
    rec(node, area, &mut path, out);
}

fn kill_all_children(node: &mut Node) {
    match node {
        Node::Leaf(p) => { let _ = p.child.kill(); }
        Node::Split { children, .. } => { for child in children.iter_mut() { kill_all_children(child); } }
    }
}

fn compute_split_borders(node: &Node, area: Rect, out: &mut Vec<(Vec<usize>, LayoutKind, usize, u16)>) {
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, LayoutKind, usize, u16)>) {
        match node {
            Node::Leaf(_) => {}
            Node::Split { kind, sizes, children } => {
                let constraints: Vec<Constraint> = if sizes.len() == children.len() {
                    sizes.iter().map(|p| Constraint::Percentage(*p)).collect()
                } else { vec![Constraint::Percentage((100 / children.len() as u16) as u16); children.len()] };
                let rects = match *kind {
                    LayoutKind::Horizontal => Layout::default().direction(Direction::Horizontal).constraints(constraints).split(area),
                    LayoutKind::Vertical => Layout::default().direction(Direction::Vertical).constraints(constraints).split(area),
                };
                for i in 0..children.len()-1 {
                    let pos = match *kind {
                        LayoutKind::Horizontal => rects[i].x + rects[i].width,
                        LayoutKind::Vertical => rects[i].y + rects[i].height,
                    };
                    out.push((path.clone(), *kind, i, pos));
                }
                for (i, child) in children.iter().enumerate() { path.push(i); rec(child, rects[i], path, out); path.pop(); }
            }
        }
    }
    let mut path = Vec::new();
    rec(node, area, &mut path, out);
}

fn split_sizes_at<'a>(node: &'a Node, path: Vec<usize>, idx: usize) -> Option<(u16,u16)> {
    let mut cur = node;
    for &i in path.iter() {
        match cur { Node::Split { children, .. } => { cur = children.get(i)?; } _ => return None }
    }
    if let Node::Split { sizes, .. } = cur {
        if idx+1 < sizes.len() { Some((sizes[idx], sizes[idx+1])) } else { None }
    } else { None }
}

fn adjust_split_sizes(root: &mut Node, d: &DragState, x: u16, y: u16) {
    if let Some(Node::Split { sizes, .. }) = get_split_mut(root, &d.split_path) {
        let total = sizes[d.index] + sizes[d.index+1];
        let min_pct = 5u16;
        let delta: i16 = match d.kind {
            LayoutKind::Horizontal => (x as i32 - d.start_x as i32).clamp(-100, 100) as i16,
            LayoutKind::Vertical => (y as i32 - d.start_y as i32).clamp(-100, 100) as i16,
        };
        let left = (d.left_initial as i16 + delta).clamp(min_pct as i16, (total - min_pct) as i16) as u16;
        let right = total - left;
        sizes[d.index] = left;
        sizes[d.index+1] = right;
    }
}

fn get_split_mut<'a>(node: &'a mut Node, path: &Vec<usize>) -> Option<&'a mut Node> {
    let mut cur = node;
    for &idx in path.iter() {
        match cur { Node::Split { children, .. } => { cur = children.get_mut(idx)?; } _ => return None }
    }
    Some(cur)
}

fn prune_exited(n: Node) -> Option<Node> {
    match n {
        Node::Leaf(mut p) => {
            match p.child.try_wait() {
                Ok(Some(_)) => None,
                _ => Some(Node::Leaf(p)),
            }
        }
        Node::Split { kind, sizes: _sizes, children } => {
            let mut new_children: Vec<Node> = Vec::new();
            for child in children { if let Some(c) = prune_exited(child) { new_children.push(c); } }
            if new_children.is_empty() { None }
            else if new_children.len() == 1 { Some(new_children.remove(0)) }
            else {
                // equalize sizes
                let mut eq = vec![100 / new_children.len() as u16; new_children.len()];
                let rem = 100 - eq.iter().sum::<u16>();
                if let Some(last) = eq.last_mut() { *last += rem; }
                Some(Node::Split { kind, sizes: eq, children: new_children })
            }
        }
    }
}

fn expand_status(fmt: &str, app: &AppState, time_str: &str) -> String {
    let mut s = fmt.to_string();
    let window = &app.windows[app.active_idx];
    s = s.replace("#I", &(app.active_idx + 1).to_string());
    s = s.replace("#W", &window.name);
    s = s.replace("#S", "pmux");
    s = s.replace("%H:%M", time_str);
    s
}

fn load_config(app: &mut AppState) {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let path = format!("{}\\.pmux.conf", home);
    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') { continue; }
            if l.starts_with("set -g ") {
                let rest = &l[7..];
                if let Some((key, value)) = rest.split_once(' ') {
                    match key.trim() {
                        "status-left" => app.status_left = value.trim().to_string(),
                        "status-right" => app.status_right = value.trim().to_string(),
                        "mouse" => app.mouse_enabled = matches!(value.trim(), "on" | "true"),
                        "cursor-style" => env::set_var("PMUX_CURSOR_STYLE", value.trim()),
                        "cursor-blink" => env::set_var("PMUX_CURSOR_BLINK", if matches!(value.trim(), "on"|"true") { "1" } else { "0" }),
                        "prefix" => {
                            if value.trim().eq_ignore_ascii_case("C-b") { app.prefix_key = (KeyCode::Char('b'), KeyModifiers::CONTROL); }
                            if value.trim().eq_ignore_ascii_case("C-a") { app.prefix_key = (KeyCode::Char('a'), KeyModifiers::CONTROL); }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn parse_status(fmt: &str, app: &AppState, time_str: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cur_style = Style::default();
    let mut i = 0;
    while i < fmt.len() {
        if fmt.as_bytes()[i] == b'#' && i + 1 < fmt.len() && fmt.as_bytes()[i+1] == b'[' {
            // parse style token #[...]
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
        // regular text, expand placeholders
        let mut j = i;
        while j < fmt.len() && !(fmt.as_bytes()[j] == b'#' && j + 1 < fmt.len() && fmt.as_bytes()[j+1] == b'[') { j += 1; }
        let chunk = &fmt[i..j];
        let text = expand_status(chunk, app, time_str);
        spans.push(Span::styled(text, cur_style));
        i = j;
    }
    spans
}

fn map_color(name: &str) -> Color {
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

fn current_prompt_pos(app: &mut AppState) -> Option<(u16,u16)> {
    let win = &mut app.windows[app.active_idx];
    let p = active_pane_mut(&mut win.root, &win.active_path)?;
    let parser = p.term.lock().ok()?;
    let (r,c) = parser.screen().cursor_position();
    Some((r,c))
}

fn move_copy_cursor(app: &mut AppState, dx: i16, dy: i16) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    // track copy position externally (no direct cursor mutation)
    let (r,c) = parser.screen().cursor_position();
    let nr = (r as i16 + dy).max(0) as u16;
    let nc = (c as i16 + dx).max(0) as u16;
    app.copy_pos = Some((nr,nc));
}

fn yank_selection(app: &mut AppState) -> io::Result<()> {
    let (anchor, pos) = match (app.copy_anchor, app.copy_pos) { (Some(a), Some(p)) => (a,p), _ => return Ok(()) };
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(()) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let screen = parser.screen();
    let r0 = anchor.0.min(pos.0); let r1 = anchor.0.max(pos.0);
    let c0 = anchor.1.min(pos.1); let c1 = anchor.1.max(pos.1);
    let mut text = String::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            if let Some(cell) = screen.cell(r, c) { text.push_str(&cell.contents().to_string()); } else { text.push(' '); }
        }
        if r < r1 { text.push('\n'); }
    }
    app.paste_buffers.push(text);
    Ok(())
}

fn paste_latest(app: &mut AppState) -> io::Result<()> {
    if let Some(buf) = app.paste_buffers.last() {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(p.master, "{}", buf); }
    }
    Ok(())
}

fn path_exists(node: &Node, path: &Vec<usize>) -> bool {
    let mut cur = node;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => {
                if let Some(next) = children.get(idx) { cur = next; } else { return false; }
            }
            Node::Leaf(_) => return false,
        }
    }
    matches!(cur, Node::Leaf(_) | Node::Split { .. })
}

fn first_leaf_path(node: &Node) -> Vec<usize> {
    fn rec(n: &Node, path: &mut Vec<usize>) -> Option<Vec<usize>> {
        match n {
            Node::Leaf(_) => Some(path.clone()),
            Node::Split { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    path.push(i);
                    if let Some(p) = rec(child, path) { return Some(p); }
                    path.pop();
                }
                None
            }
        }
    }
    rec(node, &mut Vec::new()).unwrap_or_default()
}

fn capture_active_pane(app: &mut AppState) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(()) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let screen = parser.screen();
    let mut text = String::new();
    for r in 0..p.last_rows { for c in 0..p.last_cols { if let Some(cell) = screen.cell(r, c) { text.push_str(&cell.contents().to_string()); } else { text.push(' '); } } text.push('\n'); }
    app.paste_buffers.push(text);
    Ok(())
}

fn capture_active_pane_text(app: &mut AppState) -> io::Result<Option<String>> {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(None) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let screen = parser.screen();
    let mut text = String::new();
    for r in 0..p.last_rows { for c in 0..p.last_cols { if let Some(cell) = screen.cell(r, c) { text.push_str(&cell.contents().to_string()); } else { text.push(' '); } } text.push('\n'); }
    Ok(Some(text))
}

fn save_latest_buffer(app: &mut AppState, file: &str) -> io::Result<()> {
    if let Some(buf) = app.paste_buffers.last() { std::fs::write(file, buf)?; }
    Ok(())
}

#[derive(Clone)]
enum Action { DisplayPanes, MoveFocus(FocusDir) }

#[derive(Clone)]
struct Bind { key: (KeyCode, KeyModifiers), action: Action }
enum CtrlReq {
    NewWindow,
    SplitWindow(LayoutKind),
    KillPane,
    CapturePane(mpsc::Sender<String>),
    FocusWindow(usize),
    FocusPane(usize),
    SessionInfo(mpsc::Sender<String>),
    CapturePaneRange(mpsc::Sender<String>, Option<u16>, Option<u16>),
    ClientAttach,
    ClientDetach,
    DumpLayout(mpsc::Sender<String>),
    SendText(String),
    SendKey(String),
    ZoomPane,
    CopyEnter,
    CopyMove(i16, i16),
    CopyAnchor,
    CopyYank,
    ClientSize(u16, u16),
    FocusPaneCmd(usize),
    FocusWindowCmd(usize),
    MouseDown(u16,u16),
    MouseDrag(u16,u16),
    MouseUp(u16,u16),
    ScrollUp,
    ScrollDown,
    NextWindow,
    PrevWindow,
    RenameWindow(String),
    ListWindows(mpsc::Sender<String>),
    ListTree(mpsc::Sender<String>),
    ToggleSync,
    SetPaneTitle(String),
}

fn find_window_index_by_id(app: &AppState, wid: usize) -> Option<usize> {
    for (i, w) in app.windows.iter().enumerate() { if w.id == wid { return Some(i); } }
    None
}

fn focus_pane_by_id(app: &mut AppState, pid: usize) {
    let win = &mut app.windows[app.active_idx];
    fn rec(node: &Node, path: &mut Vec<usize>, found: &mut Option<Vec<usize>>, pid: usize) {
        match node {
            Node::Leaf(p) => { if p.id == pid { *found = Some(path.clone()); } }
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() { path.push(i); rec(c, path, found, pid); path.pop(); }
            }
        }
    }
    let mut found = None;
    rec(&win.root, &mut Vec::new(), &mut found, pid);
    if let Some(p) = found { win.active_path = p; }
}
fn run_server(session_name: String) -> io::Result<()> {
    let pty_system = PtySystemSelection::default()
        .get()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("pty system error: {e}")))?;

    let mut app = AppState {
        windows: Vec::new(),
        active_idx: 0,
        mode: Mode::Passthrough,
        escape_time_ms: 500,
        prefix_key: (KeyCode::Char('b'), KeyModifiers::CONTROL),
        drag: None,
        last_window_area: Rect { x: 0, y: 0, width: 0, height: 0 },
        mouse_enabled: true,
        paste_buffers: Vec::new(),
        status_left: "pmux:#I".to_string(),
        status_right: "%H:%M".to_string(),
        copy_anchor: None,
        copy_pos: None,
        display_map: Vec::new(),
        binds: Vec::new(),
        control_rx: None,
        control_port: None,
        session_name,
        attached_clients: 0,
        created_at: Local::now(),
        next_win_id: 1,
        next_pane_id: 1,
        zoom_saved: None,
        sync_input: false,
    };
    create_window(&*pty_system, &mut app)?;
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    app.control_rx = Some(rx);
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    app.control_port = Some(port);
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let dir = format!("{}\\.pmux", home);
    let _ = std::fs::create_dir_all(&dir);
    let regpath = format!("{}\\{}.port", dir, app.session_name);
    let _ = std::fs::write(&regpath, port.to_string());
    thread::spawn(move || {
        for conn in listener.incoming() {
            if let Ok(mut stream) = conn {
                let mut line = String::new();
                let mut r = io::BufReader::new(stream.try_clone().unwrap());
                let _ = r.read_line(&mut line);
                let mut parts = line.split_whitespace();
                let cmd = parts.next().unwrap_or("");
                let mut args: Vec<&str> = parts.by_ref().collect();
                let mut target_win: Option<usize> = None;
                let mut target_pane: Option<usize> = None;
                let mut i = 0;
                while i < args.len() {
                    if args[i] == "-t" {
                        if let Some(v) = args.get(i+1) {
                            if v.starts_with('%') { if let Ok(pid) = v[1..].parse::<usize>() { target_pane = Some(pid); } }
                            else if v.starts_with('@') { if let Ok(wid) = v[1..].parse::<usize>() { target_win = Some(wid); } }
                        }
                        i += 2; continue;
                    }
                    i += 1;
                }
                if let Some(wid) = target_win { let _ = tx.send(CtrlReq::FocusWindow(wid)); }
                if let Some(pid) = target_pane { let _ = tx.send(CtrlReq::FocusPane(pid)); }
                match cmd {
                    "new-window" => { let _ = tx.send(CtrlReq::NewWindow); }
                    "split-window" => {
                        let kind = if args.iter().any(|a| *a == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                        let _ = tx.send(CtrlReq::SplitWindow(kind));
                    }
                    "kill-pane" => { let _ = tx.send(CtrlReq::KillPane); }
                    "capture-pane" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::CapturePane(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(stream, "{}", text); }
                    }
                    "dump-layout" => {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::DumpLayout(rtx));
                        if let Ok(text) = rrx.recv() { let _ = write!(stream, "{}", text); }
                    }
                    "send-text" => {
                        if let Some(payload) = args.get(0) { let _ = tx.send(CtrlReq::SendText(payload.to_string())); }
                    }
                    "send-key" => {
                        if let Some(payload) = args.get(0) { let _ = tx.send(CtrlReq::SendKey(payload.to_string())); }
                    }
                    "zoom-pane" => { let _ = tx.send(CtrlReq::ZoomPane); }
                    "copy-enter" => { let _ = tx.send(CtrlReq::CopyEnter); }
                    "copy-move" => {
                        if args.len() >= 2 { if let (Ok(dx), Ok(dy)) = (args[0].parse::<i16>(), args[1].parse::<i16>()) { let _ = tx.send(CtrlReq::CopyMove(dx, dy)); } }
                    }
                    "copy-anchor" => { let _ = tx.send(CtrlReq::CopyAnchor); }
                    "copy-yank" => { let _ = tx.send(CtrlReq::CopyYank); }
                    "client-size" => {
                        if args.len() >= 2 { if let (Ok(w), Ok(h)) = (args[0].parse::<u16>(), args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::ClientSize(w, h)); } }
                    }
                    "focus-pane" => {
                        if let Some(pid) = args.get(0).and_then(|s| s.parse::<usize>().ok()) { let _ = tx.send(CtrlReq::FocusPaneCmd(pid)); }
                    }
                    "focus-window" => {
                        if let Some(wid) = args.get(0).and_then(|s| s.parse::<usize>().ok()) { let _ = tx.send(CtrlReq::FocusWindowCmd(wid)); }
                    }
                    "mouse-down" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDown(x,y)); } }
                    }
                    "mouse-drag" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDrag(x,y)); } }
                    }
                    "mouse-up" => {
                        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUp(x,y)); } }
                    }
                    "scroll-up" => { let _ = tx.send(CtrlReq::ScrollUp); }
                    "scroll-down" => { let _ = tx.send(CtrlReq::ScrollDown); }
                    "next-window" => { let _ = tx.send(CtrlReq::NextWindow); }
                    "previous-window" => { let _ = tx.send(CtrlReq::PrevWindow); }
                    "rename-window" => { if let Some(name) = args.get(0) { let _ = tx.send(CtrlReq::RenameWindow((*name).to_string())); } }
                    "list-windows" => { let (rtx, rrx) = mpsc::channel::<String>(); let _ = tx.send(CtrlReq::ListWindows(rtx)); if let Ok(text) = rrx.recv() { let _ = write!(stream, "{}", text); } }
                    "list-tree" => { let (rtx, rrx) = mpsc::channel::<String>(); let _ = tx.send(CtrlReq::ListTree(rtx)); if let Ok(text) = rrx.recv() { let _ = write!(stream, "{}", text); } }
                    "toggle-sync" => { let _ = tx.send(CtrlReq::ToggleSync); }
                    "set-pane-title" => { let title = args.join(" "); let _ = tx.send(CtrlReq::SetPaneTitle(title)); }
                    _ => {}
                }
            }
        }
    });
    loop {
        while let Some(req) = app.control_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match req {
                CtrlReq::NewWindow => { let _ = create_window(&*pty_system, &mut app); }
                CtrlReq::SplitWindow(k) => { let _ = split_active(&mut app, k); }
                CtrlReq::KillPane => { let _ = kill_active_pane(&mut app); }
                CtrlReq::CapturePane(resp) => {
                    if let Some(text) = capture_active_pane_text(&mut app)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneRange(resp, s, e) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::FocusWindow(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::FocusPane(pid) => { focus_pane_by_id(&mut app, pid); }
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
                CtrlReq::ZoomPane => { toggle_zoom(&mut app); }
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); }
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_pos = Some((r,c)); } }
                CtrlReq::CopyYank => { let _ = yank_selection(&mut app); app.mode = Mode::Passthrough; }
                CtrlReq::ClientSize(w, h) => { app.last_window_area = Rect { x: 0, y: 0, width: w, height: h }; }
                CtrlReq::FocusPaneCmd(pid) => { focus_pane_by_id(&mut app, pid); }
                CtrlReq::FocusWindowCmd(wid) => { if let Some(idx) = find_window_index_by_id(&app, wid) { app.active_idx = idx; } }
                CtrlReq::MouseDown(x,y) => { remote_mouse_down(&mut app, x, y); }
                CtrlReq::MouseDrag(x,y) => { remote_mouse_drag(&mut app, x, y); }
                CtrlReq::MouseUp(_,_) => { app.drag = None; }
                CtrlReq::ScrollUp => { remote_scroll_up(&mut app); }
                CtrlReq::ScrollDown => { remote_scroll_down(&mut app); }
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
            }
        }
        let _ = reap_children(&mut app)?;
        thread::sleep(Duration::from_millis(20));
    }
}

fn capture_active_pane_range(app: &mut AppState, s: Option<u16>, e: Option<u16>) -> io::Result<Option<String>> {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(None) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let screen = parser.screen();
    let start = s.unwrap_or(0).min(p.last_rows.saturating_sub(1));
    let end = e.unwrap_or(p.last_rows.saturating_sub(1)).min(p.last_rows.saturating_sub(1));
    let mut text = String::new();
    for r in start..=end { for c in 0..p.last_cols { if let Some(cell) = screen.cell(r, c) { text.push_str(&cell.contents().to_string()); } else { text.push(' '); } } text.push('\n'); }
    Ok(Some(text))
}
#[derive(Serialize, Deserialize)]
struct CellJson { text: String, fg: String, bg: String, bold: bool, italic: bool, underline: bool, inverse: bool }

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum LayoutJson {
    #[serde(rename = "split")]
    Split { kind: String, sizes: Vec<u16>, children: Vec<LayoutJson> },
    #[serde(rename = "leaf")]
    Leaf { id: usize, rows: u16, cols: u16, cursor_row: u16, cursor_col: u16, content: Vec<Vec<CellJson>> },
}

fn dump_layout_json(app: &mut AppState) -> io::Result<String> {
    fn build(node: &mut Node) -> LayoutJson {
        match node {
            Node::Split { kind, sizes, children } => {
                let k = match *kind { LayoutKind::Horizontal => "Horizontal".to_string(), LayoutKind::Vertical => "Vertical".to_string() };
                let mut ch: Vec<LayoutJson> = Vec::new();
                for c in children.iter_mut() { ch.push(build(c)); }
                LayoutJson::Split { kind: k, sizes: sizes.clone(), children: ch }
            }
            Node::Leaf(p) => {
                let parser = p.term.lock().unwrap();
                let screen = parser.screen();
                let (cr, cc) = screen.cursor_position();
                if let Some(t) = infer_title_from_prompt(&screen, p.last_rows, p.last_cols) { p.title = t; }
                let mut lines: Vec<Vec<CellJson>> = Vec::new();
                for r in 0..p.last_rows {
                    let mut row: Vec<CellJson> = Vec::new();
                    for c in 0..p.last_cols {
                        if let Some(cell) = screen.cell(r, c) {
                            let fg = color_to_name(cell.fgcolor());
                            let bg = color_to_name(cell.bgcolor());
                            let text = cell.contents().to_string();
                            row.push(CellJson { text, fg, bg, bold: cell.bold(), italic: cell.italic(), underline: cell.underline(), inverse: cell.inverse() });
                        } else {
                            row.push(CellJson { text: " ".to_string(), fg: "default".to_string(), bg: "default".to_string(), bold: false, italic: false, underline: false, inverse: false });
                        }
                    }
                    lines.push(row);
                }
                LayoutJson::Leaf { id: p.id, rows: p.last_rows, cols: p.last_cols, cursor_row: cr, cursor_col: cc, content: lines }
            }
        }
    }
    let win = &mut app.windows[app.active_idx];
    let root = build(&mut win.root);
    let s = serde_json::to_string(&root).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))?;
    Ok(s)
}

fn infer_title_from_prompt(screen: &vt100::Screen, rows: u16, cols: u16) -> Option<String> {
    let mut last: Option<String> = None;
    for r in (0..rows).rev() {
        let mut s = String::new();
        for c in 0..cols { if let Some(cell) = screen.cell(r, c) { s.push_str(&cell.contents().to_string()); } else { s.push(' '); } }
        let t = s.trim_end().to_string();
        if !t.trim().is_empty() { last = Some(t); break; }
    }
    let Some(line) = last else { return None };
    let trimmed = line.trim().to_string();
    if let Some(pos) = trimmed.rfind('>') {
        let before = trimmed[..pos].trim().to_string();
        if before.contains("\\") || before.contains("/") {
            let parts: Vec<&str> = before.trim_matches(|ch: char| ch == '"').split(['\\','/']).collect();
            if let Some(base) = parts.last() { return Some(base.to_string()); }
        }
        return Some(before);
    }
    if let Some(pos) = trimmed.rfind('$') { return Some(trimmed[..pos].trim().to_string()); }
    if let Some(pos) = trimmed.rfind('#') { return Some(trimmed[..pos].trim().to_string()); }
    Some(trimmed)
}

fn resolve_last_session_name() -> Option<String> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()?;
    let dir = format!("{}\\.pmux", home);
    let last = std::fs::read_to_string(format!("{}\\last_session", dir)).ok();
    if let Some(name) = last {
        let name = name.trim().to_string();
        let p = format!("{}\\{}.port", dir, name);
        if std::path::Path::new(&p).exists() { return Some(name); }
    }
    let mut picks: Vec<(String, std::time::SystemTime)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if let Some(fname) = e.file_name().to_str() {
                if let Some((base, ext)) = fname.rsplit_once('.') {
                    if ext == "port" { if let Ok(md) = e.metadata() { picks.push((base.to_string(), md.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH))); } }
                }
            }
        }
    }
    picks.sort_by_key(|(_, t)| *t);
    picks.last().map(|(n, _)| n.clone())
}

fn resolve_default_session_name() -> Option<String> {
    if let Ok(name) = env::var("PMUX_DEFAULT_SESSION") {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()?;
        let p = format!("{}\\.pmux\\{}.port", home, name);
        if std::path::Path::new(&p).exists() { return Some(name); }
    }
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()?;
    let candidates = [format!("{}\\.pmuxrc", home), format!("{}\\.pmux\\pmuxrc", home)];
    for cfg in candidates.iter() {
        if let Ok(text) = std::fs::read_to_string(cfg) {
            let line = text.lines().find(|l| !l.trim().is_empty())?;
            let name = if let Some(rest) = line.strip_prefix("default-session ") { rest.trim().to_string() } else { line.trim().to_string() };
            let p = format!("{}\\.pmux\\{}.port", home, name);
            if std::path::Path::new(&p).exists() { return Some(name); }
        }
    }
    None
}

#[derive(Serialize, Deserialize)]
struct WinInfo { id: usize, name: String, active: bool }

#[derive(Serialize, Deserialize)]
struct PaneInfo { id: usize, title: String }

#[derive(Serialize, Deserialize)]
struct WinTree { id: usize, name: String, active: bool, panes: Vec<PaneInfo> }

fn list_windows_json(app: &AppState) -> io::Result<String> {
    let mut v: Vec<WinInfo> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() { v.push(WinInfo { id: w.id, name: w.name.clone(), active: i == app.active_idx }); }
    let s = serde_json::to_string(&v).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))?;
    Ok(s)
}

fn list_tree_json(app: &AppState) -> io::Result<String> {
    fn collect_panes(node: &Node, out: &mut Vec<PaneInfo>) {
        match node {
            Node::Leaf(p) => { out.push(PaneInfo { id: p.id, title: p.title.clone() }); }
            Node::Split { children, .. } => { for c in children.iter() { collect_panes(c, out); } }
        }
    }
    let mut v: Vec<WinTree> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() {
        let mut panes = Vec::new();
        collect_panes(&w.root, &mut panes);
        v.push(WinTree { id: w.id, name: w.name.clone(), active: i == app.active_idx, panes });
    }
    let s = serde_json::to_string(&v).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))?;
    Ok(s)
}

fn color_to_name(c: vt100::Color) -> String {
    match c {
        vt100::Color::Default => "default".to_string(),
        vt100::Color::Idx(i) => format!("idx:{}", i),
        vt100::Color::Rgb(r,g,b) => format!("rgb:{},{},{}", r,g,b),
    }
}

fn send_text_to_active(app: &mut AppState, text: &str) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(p.master, "{}", text); }
    Ok(())
}

fn send_key_to_active(app: &mut AppState, k: &str) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        match k {
            "enter" => { let _ = write!(p.master, "\r"); }
            "tab" => { let _ = write!(p.master, "\t"); }
            "backspace" => { let _ = write!(p.master, "\x08"); }
            "esc" => { let _ = write!(p.master, "\x1b"); }
            "left" => { let _ = write!(p.master, "\x1b[D"); }
            "right" => { let _ = write!(p.master, "\x1b[C"); }
            "up" => { let _ = write!(p.master, "\x1b[A"); }
            "down" => { let _ = write!(p.master, "\x1b[B"); }
            _ => {}
        }
    }
    Ok(())
}

fn toggle_zoom(app: &mut AppState) {
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

fn remote_mouse_down(app: &mut AppState, x: u16, y: u16) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    for (path, area) in rects.iter() { if area.contains(ratatui::layout::Position { x, y }) { win.active_path = path.clone(); } }
    let mut borders: Vec<(Vec<usize>, LayoutKind, usize, u16)> = Vec::new();
    compute_split_borders(&win.root, app.last_window_area, &mut borders);
    let tol = 1u16;
    for (path, kind, idx, pos) in borders.iter() {
        match kind {
            LayoutKind::Horizontal => {
                if x >= pos.saturating_sub(tol) && x <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: x, start_y: y, left_initial: left, _right_initial: right }); } break; }
            }
            LayoutKind::Vertical => {
                if y >= pos.saturating_sub(tol) && y <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: x, start_y: y, left_initial: left, _right_initial: right }); } break; }
            }
        }
    }
}

fn remote_mouse_drag(app: &mut AppState, x: u16, y: u16) { let win = &mut app.windows[app.active_idx]; if let Some(d) = &app.drag { adjust_split_sizes(&mut win.root, d, x, y); } }

fn remote_scroll_up(app: &mut AppState) { let win = &mut app.windows[app.active_idx]; if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(p.master, "\x1b[A"); } }
fn remote_scroll_down(app: &mut AppState) { let win = &mut app.windows[app.active_idx]; if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(p.master, "\x1b[B"); } }
