use std::io::{self, Write};
use std::time::Duration;
use std::env;

/// Clean up any stale port files (where server is not actually running)
pub fn cleanup_stale_port_files() {
    let home = match env::var("USERPROFILE").or_else(|_| env::var("HOME")) {
        Ok(h) => h,
        Err(_) => return,
    };
    let psmux_dir = format!("{}\\.psmux", home);
    if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "port").unwrap_or(false) {
                if let Ok(port_str) = std::fs::read_to_string(&path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let addr = format!("127.0.0.1:{}", port);
                        if std::net::TcpStream::connect_timeout(
                            &addr.parse().unwrap(),
                            Duration::from_millis(50)
                        ).is_err() {
                            let _ = std::fs::remove_file(&path);
                        }
                    } else {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

/// Read the session key from the key file
pub fn read_session_key(session: &str) -> io::Result<String> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let keypath = format!("{}\\.psmux\\{}.key", home, session);
    std::fs::read_to_string(&keypath).map(|s| s.trim().to_string())
}

/// Send an authenticated command to a server
pub fn send_auth_cmd(addr: &str, key: &str, cmd: &[u8]) -> io::Result<()> {
    let sock_addr: std::net::SocketAddr = addr.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if let Ok(mut s) = std::net::TcpStream::connect_timeout(&sock_addr, Duration::from_millis(50)) {
        let _ = write!(s, "AUTH {}\n", key);
        let _ = std::io::Write::write_all(&mut s, cmd);
        let _ = s.flush();
    }
    Ok(())
}

/// Send an authenticated command and get response
pub fn send_auth_cmd_response(addr: &str, key: &str, cmd: &[u8]) -> io::Result<String> {
    let mut s = std::net::TcpStream::connect(addr)?;
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = write!(s, "AUTH {}\n", key);
    let _ = std::io::Write::write_all(&mut s, cmd);
    let _ = s.flush();
    let mut br = std::io::BufReader::new(&mut s);
    let mut auth_line = String::new();
    let _ = std::io::BufRead::read_line(&mut br, &mut auth_line);
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut br, &mut buf);
    Ok(buf)
}

pub fn send_control(line: String) -> io::Result<()> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let target = env::var("PSMUX_TARGET_SESSION").ok().unwrap_or_else(|| "default".to_string());
    let full_target = env::var("PSMUX_TARGET_FULL").ok();
    let path = format!("{}\\.psmux\\{}.port", home, target);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, format!("no server running on session '{}'", target)))?.clone();
    let session_key = read_session_key(&target).unwrap_or_default();
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect(addr)?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = write!(stream, "AUTH {}\n", session_key);
    if let Some(ref ft) = full_target {
        let _ = write!(stream, "TARGET {}\n", ft);
    }
    let _ = write!(stream, "{}", line);
    let _ = stream.flush();
    // Read the "OK" response to drain the receive buffer before closing.
    // This prevents Windows from sending RST (due to unread data) which
    // could cause the server to lose the command.
    let mut buf = [0u8; 64];
    let _ = std::io::Read::read(&mut stream, &mut buf);
    Ok(())
}

pub fn send_control_with_response(line: String) -> io::Result<String> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let target = env::var("PSMUX_TARGET_SESSION").ok().unwrap_or_else(|| "default".to_string());
    let full_target = env::var("PSMUX_TARGET_FULL").ok();
    let path = format!("{}\\.psmux\\{}.port", home, target);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, format!("no server running on session '{}'", target)))?.clone();
    let session_key = read_session_key(&target).unwrap_or_default();
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect(&addr)?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
    let _ = write!(stream, "AUTH {}\n", session_key);
    if let Some(ref ft) = full_target {
        let _ = write!(stream, "TARGET {}\n", ft);
    }
    let _ = write!(stream, "{}", line);
    let _ = stream.flush();
    let mut buf = Vec::new();
    let mut temp = [0u8; 4096];
    loop {
        match std::io::Read::read(&mut stream, &mut temp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&temp[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => break,
            Err(_) => break,
        }
    }
    let result = String::from_utf8_lossy(&buf).to_string();
    // Strip the "OK\n" AUTH response prefix if present
    let result = if result.starts_with("OK\n") {
        result[3..].to_string()
    } else if result.starts_with("OK\r\n") {
        result[4..].to_string()
    } else {
        result
    };
    Ok(result)
}

/// Send a control message to a specific port
pub fn send_control_to_port(port: u16, msg: &str) -> io::Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
        let _ = stream.write_all(msg.as_bytes());
    }
    Ok(())
}

pub fn resolve_last_session_name() -> Option<String> {
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()?;
    let dir = format!("{}\\.psmux", home);
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

pub fn resolve_default_session_name() -> Option<String> {
    if let Ok(name) = env::var("PSMUX_DEFAULT_SESSION") {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()?;
        let p = format!("{}\\.psmux\\{}.port", home, name);
        if std::path::Path::new(&p).exists() { return Some(name); }
    }
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()?;
    let candidates = [format!("{}\\.psmuxrc", home), format!("{}\\.psmux\\pmuxrc", home)];
    for cfg in candidates.iter() {
        if let Ok(text) = std::fs::read_to_string(cfg) {
            let line = text.lines().find(|l| !l.trim().is_empty())?;
            let name = if let Some(rest) = line.strip_prefix("default-session ") { rest.trim().to_string() } else { line.trim().to_string() };
            let p = format!("{}\\.psmux\\{}.port", home, name);
            if std::path::Path::new(&p).exists() { return Some(name); }
        }
    }
    None
}

pub fn reap_children_placeholder() -> io::Result<bool> { Ok(false) }

/// A tree entry used by choose-tree: either a session header or a window under a session.
#[derive(Clone, Debug)]
pub struct TreeEntry {
    pub session_name: String,
    pub session_port: u16,
    pub is_session_header: bool,
    pub window_index: Option<usize>,
    pub window_name: String,
    pub window_panes: usize,
    pub window_size: String,
    pub is_current_session: bool,
    pub is_active_window: bool,
}

/// List all running sessions and their windows for choose-tree display.
/// Queries each running server via its TCP port for window list info.
pub fn list_all_sessions_tree(current_session: &str, current_windows: &[(String, usize, String, bool)]) -> Vec<TreeEntry> {
    let home = match env::var("USERPROFILE").or_else(|_| env::var("HOME")) {
        Ok(h) => h,
        Err(_) => return vec![],
    };
    let psmux_dir = format!("{}\\.psmux", home);
    let mut sessions: Vec<(String, u16, std::time::SystemTime)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "port").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(port_str) = std::fs::read_to_string(&path) {
                        if let Ok(port) = port_str.trim().parse::<u16>() {
                            let mtime = entry.metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            sessions.push((stem.to_string(), port, mtime));
                        }
                    }
                }
            }
        }
    }

    sessions.sort_by_key(|(name, _, _)| name.clone());

    let mut tree = Vec::new();
    for (name, port, _) in &sessions {
        let is_current = name == current_session;
        // Session header
        tree.push(TreeEntry {
            session_name: name.clone(),
            session_port: *port,
            is_session_header: true,
            window_index: None,
            window_name: String::new(),
            window_panes: 0,
            window_size: String::new(),
            is_current_session: is_current,
            is_active_window: false,
        });

        if is_current {
            // Use local data for the current session (fast, no IPC)
            for (i, (wname, panes, size, is_active)) in current_windows.iter().enumerate() {
                tree.push(TreeEntry {
                    session_name: name.clone(),
                    session_port: *port,
                    is_session_header: false,
                    window_index: Some(i),
                    window_name: wname.clone(),
                    window_panes: *panes,
                    window_size: size.clone(),
                    is_current_session: true,
                    is_active_window: *is_active,
                });
            }
        } else {
            // Query remote session for its window list
            let key = read_session_key(name).unwrap_or_default();
            let addr = format!("127.0.0.1:{}", port);
            if let Ok(resp) = send_auth_cmd_response(&addr, &key, b"list-windows -F \"#{window_index}:#{window_name}:#{window_panes}:#{window_width}x#{window_height}:#{window_active}\"\n") {
                for line in resp.lines() {
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    let parts: Vec<&str> = line.splitn(5, ':').collect();
                    if parts.len() >= 5 {
                        let wi = parts[0].parse::<usize>().unwrap_or(0);
                        let wn = parts[1].to_string();
                        let wp = parts[2].parse::<usize>().unwrap_or(1);
                        let ws = parts[3].to_string();
                        let wa = parts[4] == "1";
                        tree.push(TreeEntry {
                            session_name: name.clone(),
                            session_port: *port,
                            is_session_header: false,
                            window_index: Some(wi),
                            window_name: wn,
                            window_panes: wp,
                            window_size: ws,
                            is_current_session: false,
                            is_active_window: wa,
                        });
                    }
                }
            }
        }
    }
    tree
}
