/// Static catalog of all supported tmux options for customize-mode.

pub struct OptionDef {
    pub name: &'static str,
    pub scope: &'static str,
    pub option_type: &'static str,
    pub default: &'static str,
    pub description: &'static str,
}

pub static OPTION_CATALOG: &[OptionDef] = &[
    // ── Server options ──
    OptionDef { name: "escape-time", scope: "server", option_type: "number", default: "500", description: "Time in ms to wait for escape sequence" },
    OptionDef { name: "focus-events", scope: "server", option_type: "boolean", default: "off", description: "Send focus events to applications" },
    OptionDef { name: "bold-is-bright", scope: "server", option_type: "boolean", default: "on", description: "Rewrite crossterm's 256-indexed basic colors to standard SGR so the terminal applies bold-is-bright (issue #425); off keeps explicit 256-indexed low colors byte-accurate" },
    OptionDef { name: "history-limit", scope: "server", option_type: "number", default: "2000", description: "Maximum scrollback lines per pane" },
    OptionDef { name: "alternate-screen", scope: "server", option_type: "boolean", default: "on", description: "Honour DEC 47/1049 alt-screen mode (off = TUI output goes to scrollback, #88)" },
    OptionDef { name: "set-clipboard", scope: "server", option_type: "choice", default: "on", description: "OSC 52 clipboard integration" },
    OptionDef { name: "default-shell", scope: "server", option_type: "string", default: "", description: "Default shell for new panes" },
    OptionDef { name: "default-terminal", scope: "server", option_type: "string", default: "xterm-256color", description: "TERM value for new panes" },
    OptionDef { name: "copy-command", scope: "server", option_type: "string", default: "", description: "External copy command (pipe selection)" },
    OptionDef { name: "exit-empty", scope: "server", option_type: "boolean", default: "on", description: "Exit server when no sessions remain" },
    // ── Session options ──
    OptionDef { name: "prefix", scope: "session", option_type: "string", default: "C-b", description: "Primary prefix key" },
    OptionDef { name: "prefix2", scope: "session", option_type: "string", default: "none", description: "Secondary prefix key" },
    OptionDef { name: "base-index", scope: "session", option_type: "number", default: "0", description: "Starting index for windows" },
    OptionDef { name: "pane-base-index", scope: "session", option_type: "number", default: "0", description: "Starting index for panes" },
    OptionDef { name: "display-time", scope: "session", option_type: "number", default: "750", description: "Duration of messages in ms" },
    OptionDef { name: "display-panes-time", scope: "session", option_type: "number", default: "1000", description: "Duration of pane numbers display in ms" },
    OptionDef { name: "repeat-time", scope: "session", option_type: "number", default: "500", description: "Repeat timeout for prefix keys in ms" },
    OptionDef { name: "mouse", scope: "session", option_type: "boolean", default: "on", description: "Enable mouse support" },
    OptionDef { name: "scroll-enter-copy-mode", scope: "session", option_type: "boolean", default: "on", description: "Enter copy mode on mouse scroll when the child did not request mouse events" },
    OptionDef { name: "pwsh-mouse-selection", scope: "session", option_type: "boolean", default: "off", description: "Windows 11 PowerShell-style drag selection (pane-aware, right-click to copy, word/line multi-click)" },
    OptionDef { name: "mouse-selection", scope: "session", option_type: "boolean", default: "on", description: "Enable psmux's client-side drag-selection overlay. Set to off so apps inside a pane (opencode, etc.) can implement their own mouse selection without psmux drawing on top." },
    OptionDef { name: "paste-detection", scope: "session", option_type: "boolean", default: "on", description: "Detect Ctrl+V paste from console host and send as bracketed paste (disable to let Ctrl+V reach child apps)" },
    OptionDef { name: "mode-keys", scope: "session", option_type: "choice", default: "emacs", description: "Key bindings in copy mode (vi/emacs)" },
    OptionDef { name: "copy-mode-line-numbers", scope: "window", option_type: "choice", default: "off", description: "Line number mode in copy mode (off/default/absolute/relative/hybrid)" },
    OptionDef { name: "copy-mode-line-number-style", scope: "window", option_type: "string", default: "fg=brightblack", description: "Style for copy-mode line numbers" },
    OptionDef { name: "copy-mode-current-line-number-style", scope: "window", option_type: "string", default: "fg=yellow,bold", description: "Style for the current copy-mode line number" },
    OptionDef { name: "status", scope: "session", option_type: "boolean", default: "on", description: "Show/hide the status bar" },
    OptionDef { name: "status-position", scope: "session", option_type: "choice", default: "bottom", description: "Status bar position (top/bottom)" },
    OptionDef { name: "status-interval", scope: "session", option_type: "number", default: "15", description: "Status bar refresh interval in seconds" },
    OptionDef { name: "status-justify", scope: "session", option_type: "choice", default: "left", description: "Window list alignment (left/centre/right)" },
    OptionDef { name: "status-left", scope: "session", option_type: "string", default: "[#S] ", description: "Left side of the status bar" },
    OptionDef { name: "status-right", scope: "session", option_type: "string", default: "#{?window_bigger,[#{window_offset_x}#,#{window_offset_y}] ,}\"#{=21:pane_title}\" %H:%M %d-%b-%y", description: "Right side of the status bar" },
    OptionDef { name: "status-left-length", scope: "session", option_type: "number", default: "10", description: "Max width of left status section" },
    OptionDef { name: "status-right-length", scope: "session", option_type: "number", default: "40", description: "Max width of right status section" },
    OptionDef { name: "status-style", scope: "session", option_type: "string", default: "bg=green,fg=black", description: "Status bar style" },
    OptionDef { name: "status-left-style", scope: "session", option_type: "string", default: "default", description: "Left status section style" },
    OptionDef { name: "status-right-style", scope: "session", option_type: "string", default: "default", description: "Right status section style" },
    OptionDef { name: "message-style", scope: "session", option_type: "string", default: "bg=yellow,fg=black", description: "Command prompt / message style" },
    OptionDef { name: "message-command-style", scope: "session", option_type: "string", default: "bg=black,fg=yellow", description: "Command prompt editing style" },
    OptionDef { name: "mode-style", scope: "session", option_type: "string", default: "bg=yellow,fg=black", description: "Copy mode selection style" },
    OptionDef { name: "bell-action", scope: "session", option_type: "choice", default: "any", description: "Bell handling (any/none/current/other)" },
    OptionDef { name: "visual-bell", scope: "session", option_type: "boolean", default: "off", description: "Show visual indicator on bell" },
    OptionDef { name: "activity-action", scope: "session", option_type: "choice", default: "other", description: "Activity alert action" },
    OptionDef { name: "silence-action", scope: "session", option_type: "choice", default: "other", description: "Silence alert action" },
    OptionDef { name: "monitor-silence", scope: "session", option_type: "number", default: "0", description: "Seconds of silence before alert (0=off)" },
    OptionDef { name: "destroy-unattached", scope: "session", option_type: "boolean", default: "off", description: "Destroy session when last client detaches" },
    OptionDef { name: "renumber-windows", scope: "session", option_type: "boolean", default: "off", description: "Renumber windows on close" },
    OptionDef { name: "set-titles", scope: "session", option_type: "boolean", default: "off", description: "Set terminal title" },
    OptionDef { name: "set-titles-string", scope: "session", option_type: "string", default: "#S:#I:#W", description: "Terminal title format string" },
    OptionDef { name: "word-separators", scope: "session", option_type: "string", default: " -_@", description: "Characters treated as word boundaries" },
    OptionDef { name: "allow-passthrough", scope: "session", option_type: "choice", default: "off", description: "Allow passthrough escape sequences" },
    OptionDef { name: "allow-rename", scope: "session", option_type: "boolean", default: "on", description: "Allow programs to rename windows" },
    OptionDef { name: "allow-set-title", scope: "session", option_type: "boolean", default: "off", description: "Allow programs to set pane title via escape sequences" },
    OptionDef { name: "update-environment", scope: "session", option_type: "string", default: "DISPLAY KRB5CCNAME SSH_ASKPASS SSH_AUTH_SOCK SSH_AGENT_PID SSH_CONNECTION WINDOWID XAUTHORITY", description: "Environment variables to update on attach" },
    OptionDef { name: "synchronize-panes", scope: "session", option_type: "boolean", default: "off", description: "Send input to all panes simultaneously" },
    // ── psmux extensions (session scope) ──
    OptionDef { name: "prediction-dimming", scope: "session", option_type: "boolean", default: "off", description: "Dim PSReadLine prediction text" },
    OptionDef { name: "allow-predictions", scope: "session", option_type: "boolean", default: "off", description: "Allow PSReadLine predictions" },
    OptionDef { name: "warm", scope: "session", option_type: "boolean", default: "on", description: "Pre-spawn warm shell for fast window creation" },
    OptionDef { name: "cursor-style", scope: "session", option_type: "choice", default: "bar", description: "Cursor style (bar/block/underline)" },
    OptionDef { name: "cursor-blink", scope: "session", option_type: "boolean", default: "on", description: "Blink the cursor" },
    OptionDef { name: "claude-code-fix-tty", scope: "session", option_type: "boolean", default: "on", description: "Fix TTY for Claude Code sessions" },
    OptionDef { name: "claude-code-force-interactive", scope: "session", option_type: "boolean", default: "on", description: "Force interactive mode for Claude Code" },
    // ── Window options ──
    OptionDef { name: "automatic-rename", scope: "window", option_type: "boolean", default: "on", description: "Auto-rename windows based on running command" },
    OptionDef { name: "monitor-activity", scope: "window", option_type: "boolean", default: "off", description: "Monitor for activity in window" },
    OptionDef { name: "remain-on-exit", scope: "window", option_type: "boolean", default: "off", description: "Keep pane open after command exits" },
    OptionDef { name: "aggressive-resize", scope: "window", option_type: "boolean", default: "off", description: "Resize window to smallest attached client" },
    OptionDef { name: "main-pane-width", scope: "window", option_type: "number", default: "0", description: "Width of main pane in main-* layouts (0 sizes it automatically)" },
    OptionDef { name: "main-pane-height", scope: "window", option_type: "number", default: "0", description: "Height of main pane in main-* layouts (0 sizes it automatically)" },
    OptionDef { name: "window-size", scope: "window", option_type: "choice", default: "latest", description: "Window sizing strategy" },
    OptionDef { name: "window-status-format", scope: "window", option_type: "string", default: "#I:#W#{?window_flags,#{window_flags}, }", description: "Window status bar format" },
    OptionDef { name: "window-status-current-format", scope: "window", option_type: "string", default: "#I:#W#{?window_flags,#{window_flags}, }", description: "Active window status bar format" },
    OptionDef { name: "window-status-separator", scope: "window", option_type: "string", default: " ", description: "Separator between window entries" },
    OptionDef { name: "window-status-style", scope: "window", option_type: "string", default: "default", description: "Inactive window style" },
    OptionDef { name: "window-status-current-style", scope: "window", option_type: "string", default: "default", description: "Active window style" },
    OptionDef { name: "window-status-activity-style", scope: "window", option_type: "string", default: "reverse", description: "Window style on activity alert" },
    OptionDef { name: "window-status-bell-style", scope: "window", option_type: "string", default: "reverse", description: "Window style on bell alert" },
    OptionDef { name: "window-status-last-style", scope: "window", option_type: "string", default: "default", description: "Previously active window style" },
    // ── Pane options ──
    OptionDef { name: "pane-border-style", scope: "pane", option_type: "string", default: "default", description: "Inactive pane border style" },
    OptionDef { name: "pane-active-border-style", scope: "pane", option_type: "string", default: "fg=green", description: "Active pane border style" },
    OptionDef { name: "pane-border-lines", scope: "pane", option_type: "choice", default: "single", description: "Pane border line style (single/double/heavy/simple/number/spaces/none)" },
];

/// Build the flattened option list for CustomizeMode using live values from AppState.
pub fn build_option_list(app: &crate::types::AppState) -> Vec<(String, String, String)> {
    use crate::server::options::get_option_value;
    OPTION_CATALOG.iter().map(|def| {
        let value = get_option_value(app, def.name);
        (def.name.to_string(), value, def.scope.to_string())
    }).collect()
}

/// Look up the default value for a given option name.
pub fn default_for(name: &str) -> Option<&'static str> {
    OPTION_CATALOG.iter().find(|d| d.name == name).map(|d| d.default)
}

#[cfg(test)]
#[path = "../../tests-rs/test_option_default_parity.rs"]
mod tests_option_default_parity;
