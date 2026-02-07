use crate::types::{ParsedTarget, VERSION};

pub fn get_program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "psmux".to_string())
        .to_lowercase()
        .replace(".exe", "")
}

pub fn print_help() {
    let prog = get_program_name();
    println!(r#"{prog} - Terminal multiplexer for Windows (tmux alternative)

USAGE:
    {prog} [COMMAND] [OPTIONS]

COMMANDS:
    (no command)        Start a new session or attach to existing one
    new-session         Create a new session
        -s <name>       Session name (default: "default")
        -d              Start detached (in background)        -- <cmd> [args] Run a specific command instead of the default shell    attach, attach-session
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
    PSMUX_SESSION_NAME       Default session name
    PSMUX_DEFAULT_SESSION    Fallback default session name
    PSMUX_CURSOR_STYLE       Cursor style (block, underline, bar)
    PSMUX_CURSOR_BLINK       Cursor blinking (true/false)

CONFIG FILES:
    %USERPROFILE%\.psmux.conf     Main configuration file
    %USERPROFILE%\.psmuxrc        Alternative configuration file

EXAMPLES:
    {prog}                          Start or attach to default session
    {prog} new-session -s work      Create a new session named "work"
    {prog} new-session -s dev -- pwsh -NoExit -Command "git status"
                                    Create session running a specific command
    {prog} attach -t work           Attach to session "work"
    {prog} ls                       List all sessions
    {prog} split-window -h          Split current pane horizontally

NOTE: psmux includes 'tmux' and 'pmux' aliases - use any command you prefer!

For more information: https://github.com/marlocarlo/psmux
"#, prog = prog);
}

pub fn print_version() {
    let prog = get_program_name();
    println!("{} {}", prog, VERSION);
}

pub fn print_commands() {
    println!(r#"Available commands:
  attach-session (attach)   - Attach to a session
  bind-key (bind)           - Bind a key to a command
  break-pane                - Break a pane into a new window
  capture-pane              - Capture the contents of a pane
  choose-tree               - Choose a session, window or pane from a tree
  clear-history (clearhist) - Clear pane scrollback history
  confirm-before (confirm)  - Run command after confirmation
  copy-mode                 - Enter copy mode
  delete-buffer             - Delete a paste buffer
  detach-client (detach)    - Detach from the current session
  display-menu (menu)       - Display a menu
  display-message           - Display a message in the status line
  display-panes             - Display pane numbers
  display-popup (popup)     - Display a popup window
  find-window (findw)       - Search for a window by name
  has-session               - Check if a session exists
  if-shell (if)             - Conditional command execution
  join-pane                 - Join a pane to a window
  kill-pane                 - Kill a pane
  kill-server               - Kill the psmux server
  kill-session              - Kill a session
  kill-window               - Kill a window
  last-pane                 - Select the previously active pane
  last-window               - Select the previously active window
  link-window (linkw)       - Link a window to another session
  list-buffers (lsb)        - List paste buffers
  list-clients (lsc)        - List connected clients
  list-commands (lscm)      - List commands
  list-keys (lsk)           - List key bindings
  list-panes (lsp)          - List panes in a window
  list-sessions (ls)        - List sessions
  list-windows (lsw)        - List windows in a session
  load-buffer (loadb)       - Load buffer from file
  lock-client (lockc)       - Lock the client
  move-pane (movep)         - Move a pane to another window
  move-window (movew)       - Move a window to a different index
  new-session (new)         - Create a new session
  new-window (neww)         - Create a new window
  next-layout (nextl)       - Cycle to next layout
  next-window (next)        - Move to the next window
  paste-buffer              - Paste from a buffer
  pipe-pane (pipep)         - Pipe pane output to a command
  previous-window (prev)    - Move to the previous window
  refresh-client (refresh)  - Refresh client display
  rename-session            - Rename a session
  rename-window (renamew)   - Rename a window
  resize-pane (resizep)     - Resize a pane
  respawn-pane              - Respawn a pane
  rotate-window (rotatew)   - Rotate panes in a window
  run-shell (run)           - Run a shell command
  save-buffer (saveb)       - Save buffer to file
  select-layout (selectl)   - Apply a layout preset
  select-pane (selectp)     - Select a pane
  select-window (selectw)   - Select a window
  send-keys                 - Send keys to a pane
  set-buffer (setb)         - Set a paste buffer
  set-environment (setenv)  - Set an environment variable
  set-hook                  - Set a hook command
  set-option (set)          - Set a session or window option
  show-buffer (showb)       - Display the contents of a paste buffer
  show-environment (showenv)- Show environment variables
  show-hooks                - Show defined hooks
  show-options (show)       - Show session or window options
  source-file (source)      - Execute commands from a file
  split-window (splitw)     - Split a window into panes
  start-server              - Start the psmux server
  suspend-client (suspendc) - Suspend the client
  swap-pane (swapp)         - Swap two panes
  swap-window (swapw)       - Swap two windows
  switch-client (switchc)   - Switch to another session
  unbind-key (unbind)       - Unbind a key
  unlink-window (unlinkw)   - Unlink a window
  wait-for (wait)           - Wait for a signal
  zoom-pane (zoom)          - Toggle pane zoom
"#);
}

/// Parse a tmux-style target specification
pub fn parse_target(target: &str) -> ParsedTarget {
    let mut result = ParsedTarget::default();
    
    if target.starts_with('%') {
        if let Ok(pid) = target[1..].parse::<usize>() {
            result.pane = Some(pid);
            result.pane_is_id = true;
        }
        return result;
    }
    if target.starts_with('@') {
        if let Ok(wid) = target[1..].parse::<usize>() {
            result.window = Some(wid);
            result.window_is_id = true;
        }
        return result;
    }
    
    let (session_part, window_pane_part) = if let Some(colon_pos) = target.find(':') {
        let session = if colon_pos == 0 { None } else { Some(target[..colon_pos].to_string()) };
        (session, Some(&target[colon_pos + 1..]))
    } else if target.starts_with('.') {
        (None, Some(target))
    } else {
        // A bare string without ':' or '.' is always a session name, even if numeric.
        // Window/pane specifiers require explicit syntax like ":0" or ".1"
        (Some(target.to_string()), None)
    };
    
    result.session = session_part;
    
    if let Some(wp) = window_pane_part {
        if wp.starts_with('%') {
            if let Ok(pid) = wp[1..].parse::<usize>() {
                result.pane = Some(pid);
                result.pane_is_id = true;
            }
        } else if wp.starts_with('@') {
            if let Ok(wid) = wp[1..].parse::<usize>() {
                result.window = Some(wid);
                result.window_is_id = true;
            }
        } else if let Some(dot_pos) = wp.find('.') {
            if dot_pos > 0 {
                if let Ok(w) = wp[..dot_pos].parse::<usize>() {
                    result.window = Some(w);
                }
            }
            if let Ok(p) = wp[dot_pos + 1..].parse::<usize>() {
                result.pane = Some(p);
            }
        } else {
            if let Ok(w) = wp.parse::<usize>() {
                result.window = Some(w);
            }
        }
    }
    
    result
}

/// Extract the session name from a target string (for port file lookup)
pub fn extract_session_from_target(target: &str) -> String {
    let parsed = parse_target(target);
    parsed.session.unwrap_or_else(|| "default".to_string())
}
