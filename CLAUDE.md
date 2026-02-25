# Project Instructions

## Build & Deploy
- After every successful `cargo build --release`, copy the binary to **both** locations:
  - `~/.local/bin/psmux.exe`
  - `E:\wojtek_elysium5.6_tsunami\agentx\bin\psmux.exe`
- "Elysium" refers to the workspace at `E:\wojtek_elysium5.6_tsunami\`

## Upstream Sync Procedure

This is a fork of `marlocarlo/psmux`. The upstream remote is named `marlocarlo`.
Our fork changes live as separate commits on top of upstream (one commit per change).
When asked to update/sync with upstream:

1. `git fetch marlocarlo`
2. `git rebase marlocarlo/master` — rebase our commits on top of new upstream
3. If conflicts, resolve per-commit (each commit is one logical change)
4. If rebase is too messy, fallback: `git reset --hard marlocarlo/master` then reapply
   all fork changes listed in the **Fork Changes** section below as separate commits
5. Fix any FFI signature clashes (e.g. `*mut c_void` vs `isize` for HANDLE types)
6. `cargo build --release` — must compile clean
7. Deploy (copy binary to both locations above)
8. `git push origin master --force-with-lease`

## Fork Changes

This is the living list of our changes on top of upstream. When adding a new fork
change, add it here. When syncing with upstream, reapply everything listed here.

### Named Pipe IPC (`src/pipe.rs` + transport layer)
Replace all TCP (`TcpStream`/`TcpListener`) communication with Windows named pipes.
- Create/update `src/pipe.rs` — named pipe module using raw Win32 FFI (no new crate deps)
  - `PipeStream` struct with `Read`, `Write`, `Drop`, `try_clone()` (via `DuplicateHandle`)
  - `create_server_pipe()`, `wait_for_connection()`, `disconnect_pipe()`
  - `connect_to_pipe()`, `pipe_exists()`, `sanitize_session_name()`, `pipe_name_for_session()`
  - SDDL: `D:(A;;GA;;;WD)(A;;GA;;;AN)S:(ML;;NW;;;LW)` — allows Session 0 (SSH) access
  - Pipe name format: `\\.\pipe\psmux-{name}`
- Add `mod pipe;` to `src/main.rs`
- Replace TCP in: `src/server/mod.rs`, `src/server/connection.rs`, `src/session.rs`, `src/client.rs`, `src/main.rs`, `src/app.rs`
- Remove `control_port: Option<u16>` from `AppState` in `src/types.rs`
- Update `src/commands.rs`: `send_control_to_port` → `send_control_to_session`
- Update `src/pane.rs` and `src/window_ops.rs`: remove `control_port` from `set_tmux_env`
- Session discovery changes from `.port` files + TCP connect to `.key` files + `pipe_exists()`
- Keep the AUTH protocol and `.key` files unchanged

**Why:** SSH sessions on Windows run in Session 0 (isolated). Named pipes with SDDL
security descriptors (low integrity SACL) allow cross-session access, so SSH users
can control psmux sessions created on the desktop.

### send-keys argument fix (`src/server/connection.rs`)
In the send-keys handler, replace the broken `!a.starts_with('-')` filter with a
flag-then-keys state machine using `parsing_flags` bool. This allows sending arguments
that start with `-` (e.g., `send-keys -- --help Enter`).

### Backspace fix (`src/input.rs`)
Change `KeyCode::Backspace` from `b"\x08"` (word-delete) to `b"\x7f"` (char-delete).
There may be two locations — one in normal mode and one in passthrough mode.

## Architecture Notes
- Fork of marlocarlo/psmux — Windows terminal multiplexer (tmux clone)
- Uses Windows named pipes (not TCP) for IPC — see `src/pipe.rs`
- Auth protocol via `.key` files in `~/.psmux/`
- Session discovery: `.key` files + `pipe_exists()` check
- No new crate dependencies for pipes — raw FFI matching `platform.rs` style
- The `port_file_base()` method on AppState was kept (not renamed) — returns session name with optional socket_name prefix, used for .key file naming and pipe name generation
