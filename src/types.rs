use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers};
use portable_pty::MasterPty;
use ratatui::prelude::Rect;
use chrono::Local;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Pane {
    pub master: Box<dyn MasterPty>,
    pub child: Box<dyn portable_pty::Child>,
    pub term: Arc<Mutex<vt100::Parser>>,
    pub last_rows: u16,
    pub last_cols: u16,
    pub id: usize,
    pub title: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LayoutKind { Horizontal, Vertical }

pub enum Node {
    Leaf(Pane),
    Split { kind: LayoutKind, sizes: Vec<u16>, children: Vec<Node> },
}

pub struct Window {
    pub root: Node,
    pub active_path: Vec<usize>,
    pub name: String,
    pub id: usize,
}

/// A menu item for display-menu
#[derive(Clone)]
pub struct MenuItem {
    pub name: String,
    pub key: Option<char>,
    pub command: String,
    pub is_separator: bool,
}

/// A parsed menu structure
#[derive(Clone)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub x: Option<i16>,
    pub y: Option<i16>,
}

/// Hook definition - command to run on certain events
#[derive(Clone)]
pub struct Hook {
    pub name: String,
    pub command: String,
}

/// Pipe pane state - process piping pane output
pub struct PipePaneState {
    pub pane_id: usize,
    pub process: Option<std::process::Child>,
    pub stdin: bool,
    pub stdout: bool,
}

/// Wait-for channel state
pub struct WaitChannel {
    pub locked: bool,
    pub waiters: Vec<mpsc::Sender<()>>,
}

pub enum Mode {
    Passthrough,
    Prefix { armed_at: Instant },
    CommandPrompt { input: String },
    WindowChooser { selected: usize },
    RenamePrompt { input: String },
    CopyMode,
    PaneChooser { opened_at: Instant },
    /// Interactive menu mode
    MenuMode { menu: Menu },
    /// Popup window running a command
    PopupMode { 
        command: String, 
        output: String, 
        process: Option<std::process::Child>,
        width: u16,
        height: u16,
        close_on_exit: bool,
    },
    /// Confirmation prompt before command
    ConfirmMode { 
        prompt: String, 
        command: String,
        input: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum FocusDir { Left, Right, Up, Down }

pub struct AppState {
    pub windows: Vec<Window>,
    pub active_idx: usize,
    pub mode: Mode,
    pub escape_time_ms: u64,
    pub prefix_key: (KeyCode, KeyModifiers),
    pub prediction_dimming: bool,
    pub drag: Option<DragState>,
    pub last_window_area: Rect,
    pub mouse_enabled: bool,
    pub paste_buffers: Vec<String>,
    pub status_left: String,
    pub status_right: String,
    pub window_base_index: usize,
    pub copy_anchor: Option<(u16,u16)>,
    pub copy_pos: Option<(u16,u16)>,
    pub copy_scroll_offset: usize,
    pub display_map: Vec<(usize, Vec<usize>)>,
    pub binds: Vec<Bind>,
    pub control_rx: Option<mpsc::Receiver<CtrlReq>>,
    pub control_port: Option<u16>,
    pub session_name: String,
    pub attached_clients: usize,
    pub created_at: chrono::DateTime<Local>,
    pub next_win_id: usize,
    pub next_pane_id: usize,
    pub zoom_saved: Option<Vec<(Vec<usize>, Vec<u16>)>>,
    pub sync_input: bool,
    /// Hooks: map of hook name to list of commands
    pub hooks: std::collections::HashMap<String, Vec<String>>,
    /// Wait-for channels: map of channel name to list of waiting senders
    pub wait_channels: std::collections::HashMap<String, WaitChannel>,
    /// Pipe pane processes
    pub pipe_panes: Vec<PipePaneState>,
    /// Last active window index (for last-window command)
    pub last_window_idx: usize,
    /// Last active pane path (for last-pane command)
    pub last_pane_path: Vec<usize>,
    /// Tab positions on status bar: (window_index, x_start, x_end)
    pub tab_positions: Vec<(usize, u16, u16)>,
}

pub struct DragState {
    pub split_path: Vec<usize>,
    pub kind: LayoutKind,
    pub index: usize,
    pub start_x: u16,
    pub start_y: u16,
    pub left_initial: u16,
    pub _right_initial: u16,
    /// Total pixel dimension of the parent split area along the split axis.
    pub total_pixels: u16,
}

#[derive(Clone)]
pub enum Action { 
    DisplayPanes, 
    MoveFocus(FocusDir),
    /// Execute an arbitrary tmux-style command string
    Command(String),
    /// Common actions with direct handling
    NewWindow,
    SplitHorizontal,
    SplitVertical,
    KillPane,
    NextWindow,
    PrevWindow,
    CopyMode,
    Paste,
    Detach,
    RenameWindow,
    WindowChooser,
    ZoomPane,
}

#[derive(Clone)]
pub struct Bind { pub key: (KeyCode, KeyModifiers), pub action: Action }

pub enum CtrlReq {
    NewWindow(Option<String>),
    SplitWindow(LayoutKind, Option<String>),
    KillPane,
    CapturePane(mpsc::Sender<String>),
    FocusWindow(usize),
    FocusPane(usize),
    FocusPaneByIndex(usize),
    SessionInfo(mpsc::Sender<String>),
    CapturePaneRange(mpsc::Sender<String>, Option<u16>, Option<u16>),
    ClientAttach,
    ClientDetach,
    DumpLayout(mpsc::Sender<String>),
    DumpState(mpsc::Sender<String>),
    SendText(String),
    SendKey(String),
    SendPaste(String),
    ZoomPane,
    CopyEnter,
    CopyMove(i16, i16),
    CopyAnchor,
    CopyYank,
    ClientSize(u16, u16),
    FocusPaneCmd(usize),
    FocusWindowCmd(usize),
    MouseDown(u16,u16),
    MouseDownRight(u16,u16),
    MouseDownMiddle(u16,u16),
    MouseDrag(u16,u16),
    MouseUp(u16,u16),
    MouseUpRight(u16,u16),
    MouseUpMiddle(u16,u16),
    MouseMove(u16,u16),
    ScrollUp(u16, u16),
    ScrollDown(u16, u16),
    NextWindow,
    PrevWindow,
    RenameWindow(String),
    ListWindows(mpsc::Sender<String>),
    ListTree(mpsc::Sender<String>),
    ToggleSync,
    SetPaneTitle(String),
    SendKeys(String, bool),
    SelectPane(String),
    SelectWindow(usize),
    ListPanes(mpsc::Sender<String>),
    KillWindow,
    KillSession,
    HasSession(mpsc::Sender<bool>),
    RenameSession(String),
    SwapPane(String),
    ResizePane(String, u16),
    SetBuffer(String),
    ListBuffers(mpsc::Sender<String>),
    ShowBuffer(mpsc::Sender<String>),
    DeleteBuffer,
    DisplayMessage(mpsc::Sender<String>, String),
    LastWindow,
    LastPane,
    RotateWindow(bool),
    DisplayPanes,
    BreakPane,
    JoinPane(usize),
    RespawnPane,
    BindKey(String, String),
    UnbindKey(String),
    ListKeys(mpsc::Sender<String>),
    SetOption(String, String),
    ShowOptions(mpsc::Sender<String>),
    SourceFile(String),
    MoveWindow(Option<usize>),
    SwapWindow(usize),
    LinkWindow(String),
    UnlinkWindow,
    FindWindow(mpsc::Sender<String>, String),
    MovePane(usize),
    PipePane(String, bool, bool),
    SelectLayout(String),
    NextLayout,
    ListClients(mpsc::Sender<String>),
    SwitchClient(String),
    LockClient,
    RefreshClient,
    SuspendClient,
    CopyModePageUp,
    ClearHistory,
    SaveBuffer(String),
    LoadBuffer(String),
    SetEnvironment(String, String),
    ShowEnvironment(mpsc::Sender<String>),
    SetHook(String, String),
    ShowHooks(mpsc::Sender<String>),
    RemoveHook(String),
    KillServer,
    WaitFor(String, WaitForOp),
    DisplayMenu(String, Option<i16>, Option<i16>),
    DisplayPopup(String, u16, u16, bool),
    ConfirmBefore(String, String),
}

/// Wait-for operation types
#[derive(Clone, Copy)]
pub enum WaitForOp {
    Wait,
    Lock,
    Signal,
    Unlock,
}

/// Parsed target specification from -t argument.
#[derive(Debug, Clone, Default)]
pub struct ParsedTarget {
    pub session: Option<String>,
    pub window: Option<usize>,
    pub pane: Option<usize>,
    pub pane_is_id: bool,
    pub window_is_id: bool,
}
