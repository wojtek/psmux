```
╔═══════════════════════════════════════════════════════════╗
║   ██████╗ ███████╗███╗   ███╗██╗   ██╗██╗  ██╗            ║
║   ██╔══██╗██╔════╝████╗ ████║██║   ██║╚██╗██╔╝            ║
║   ██████╔╝███████╗██╔████╔██║██║   ██║ ╚███╔╝             ║
║   ██╔═══╝ ╚════██║██║╚██╔╝██║██║   ██║ ██╔██╗             ║
║   ██║     ███████║██║ ╚═╝ ██║╚██████╔╝██╔╝ ██╗            ║
║   ╚═╝     ╚══════╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═╝            ║
║     Born in PowerShell. Made in Rust. 🦀                 ║
║          Terminal Multiplexer for Windows                 ║
╚═══════════════════════════════════════════════════════════╝
```

<p align="center">
  <strong>The native Windows tmux. Born in PowerShell, made in Rust.</strong><br/>
  Full mouse support · tmux themes · tmux config · 76 commands · blazing fast
</p>

<p align="center">
  <a href="#installation">Install</a> ·
  <a href="#usage">Usage</a> ·
  <a href="#mouse-over-ssh">SSH Mouse</a> ·
  <a href="docs/keybindings.md">Keys</a> ·
  <a href="docs/configuration.md">Config</a> ·
  <a href="#performance">Performance</a> ·
  <a href="#tmux-compatibility">Compatibility</a> ·
  <a href="docs/scripting.md">Scripting</a> ·
  <a href="docs/faq.md">FAQ</a>
</p>

---

# psmux

**The real tmux for Windows.** Not a port, not a wrapper, not a workaround.

psmux is a **native Windows terminal multiplexer** built from the ground up in Rust. It uses Windows ConPTY directly, speaks the tmux command language, reads your `.tmux.conf`, and supports tmux themes. All without WSL, Cygwin, or MSYS2.

> 💡 **Tip:** psmux ships with `tmux` and `pmux` aliases. Just type `tmux` and it works!

## Installation

### Using WinGet

```powershell
winget install psmux
```

### Using Cargo

```powershell
cargo install psmux
```

This installs `psmux`, `pmux`, and `tmux` binaries to your Cargo bin directory.

### Using Scoop

```powershell
scoop bucket add psmux https://github.com/marlocarlo/scoop-psmux
scoop install psmux
```

### Using Chocolatey

```powershell
choco install psmux
```

### From GitHub Releases

Download the latest `.zip` from [GitHub Releases](https://github.com/marlocarlo/psmux/releases) and add to your PATH.

### From Source

```powershell
git clone https://github.com/marlocarlo/psmux.git
cd psmux
cargo build --release
```

Built binaries:

```text
target\release\psmux.exe
target\release\pmux.exe
target\release\tmux.exe
```

### Requirements

- Windows 10 or Windows 11
- **PowerShell 7+** (recommended) or cmd.exe
  - Download PowerShell: `winget install --id Microsoft.PowerShell`
  - Or visit: https://aka.ms/powershell

## Why psmux?

If you've used tmux on Linux/macOS and wished you had something like it on Windows, **this is it**.

| | psmux | Windows Terminal tabs | WSL + tmux |
|---|:---:|:---:|:---:|
| Session persist (detach/reattach) | ✅ | ❌ | ⚠️ WSL only |
| Synchronized panes | ✅ | ❌ | ✅ |
| tmux keybindings | ✅ | ❌ | ✅ |
| Reads `.tmux.conf` | ✅ | ❌ | ✅ |
| tmux theme support | ✅ | ❌ | ✅ |
| Native Windows shells | ✅ | ✅ | ❌ |
| Full mouse support | ✅ | ✅ | ⚠️ Partial |
| Zero dependencies | ✅ | ✅ | ❌ (needs WSL) |
| Scriptable (76 commands) | ✅ | ❌ | ✅ |

![psmux in action - monitoring system info](psmux_sysinfo.gif)

### Highlights

- 🦠 **Made in Rust** : opt-level 3, full LTO, single codegen unit. Maximum performance.
- 🖱️ **Full mouse support** : click panes, drag-resize borders, scroll, click tabs, select text, right-click copy
- 🎨 **tmux theme support** : 16 named colors + 256 indexed + 24-bit true color (`#RRGGBB`), 14 style options
- 📋 **Reads your `.tmux.conf`** : drop-in config compatibility, zero learning curve
- ⚡ **Blazing fast startup** : sub-100ms session creation, near-zero overhead over shell startup
- 🔌 **76 tmux-compatible commands** : `bind-key`, `set-option`, `if-shell`, `run-shell`, hooks, and more
- 🪟 **Windows-native** : ConPTY, Win32 API, works with PowerShell, cmd, bash, WSL, nushell
- 📦 **Single binary, no dependencies** : install via `cargo`, `winget`, `scoop`, or `choco`

## Features

### Terminal Multiplexing
- Split panes horizontally (`Prefix + %`) and vertically (`Prefix + "`)
- Multiple windows with clickable status-bar tabs
- Session management: detach (`Prefix + d`) and reattach from anywhere
- 5 layouts: even-horizontal, even-vertical, main-horizontal, main-vertical, tiled

### Full Mouse Support
- **Click** any pane to focus it, input goes to the right shell
- **Drag** pane borders to resize splits interactively
- **Click** status-bar tabs to switch windows
- **Scroll wheel** in any pane, scrolls that pane's output
- **Drag-select** text to copy to clipboard
- **Right-click** to paste or copy selection
- **VT mouse forwarding** : apps like vim, htop, and midnight commander get full mouse events
- **3-layer mouse injection** : VT protocol, VT bridge (for WSL/SSH), and native Win32 MOUSE_EVENT
- **Mouse over SSH** : works from any OS client when server runs Windows 11 build 22523+ (see [Mouse Over SSH](#mouse-over-ssh))

### tmux Theme & Style Support
- **14 customizable style options** : status bar, pane borders, messages, copy-mode highlights, popups
- **Full color spectrum** : 16 named colors, 256 indexed (`colour0`–`colour255`), 24-bit true color (`#RRGGBB`)
- **Text attributes** : bold, dim, italic, underline, blink, reverse, strikethrough, and more
- **Status bar** : fully customizable left/right content with format variables
- **Window tab styling** : separate styles for active, inactive, activity, bell, and last-used tabs
- Compatible with existing tmux theme configs

### Copy Mode (Vim Keybindings)
- **53 vi-style key bindings** : motions, selections, search, text objects
- Visual, line, and **rectangle selection** modes (`v`, `V`, `Ctrl+v`)
- `/` and `?` search with `n`/`N` navigation
- `f`/`F`/`t`/`T` character find, `%` bracket matching, `{`/`}` paragraph jump
- Named registers (`"a`–`"z`), count prefixes, word/WORD variants
- Mouse drag-select copies to Windows clipboard on release

### Format Engine
- **126+ tmux-compatible format variables** across sessions, windows, panes, cursor, client, and server
- Conditionals (`#{?cond,true,false}`), comparisons, boolean logic
- Regex substitution (`#{s/pat/rep/:var}`), string manipulation
- Loop iteration (`#{W:fmt}`, `#{P:fmt}`, `#{S:fmt}`) over windows, panes, sessions
- Truncation, padding, basename, dirname, strftime, shell quoting

### Scripting & Automation
- **76 tmux-compatible commands** : everything you need for automation
- `send-keys`, `capture-pane`, `pipe-pane` for CI/CD and DevOps workflows
- `if-shell` and `run-shell` for conditional config logic
- **15+ event hooks** : `after-new-window`, `after-split-window`, `client-attached`, etc.
- Paste buffers, named registers, `display-message` with format variables

### Multi-Shell Support
- **PowerShell 7** (default), PowerShell 5, cmd.exe
- **Git Bash**, WSL, nushell, and any Windows executable
- Sets `TERM=xterm-256color`, `COLORTERM=truecolor` automatically
- Sets `TMUX` and `TMUX_PANE` env vars for tmux-aware tool compatibility

![psmux windows and panes](psmux_windows.gif)

## Performance

psmux is built for speed. The Rust release binary is compiled with **opt-level 3**, **full LTO**, and **single codegen unit**. Every cycle counts.

| Metric | psmux | Notes |
|--------|-------|-------|
| **Session creation** | **< 100ms** | Time for `new-session -d` to return |
| **New window** | **< 80ms** | Overhead on top of shell startup |
| **New pane (split)** | **< 80ms** | Same as window, cached shell resolution |
| **Startup to prompt** | **~shell launch time** | psmux adds near-zero overhead; bottleneck is your shell |
| **15+ windows** | ✅ Stable | Stress-tested with 15+ rapid windows, 18+ panes, 5 concurrent sessions |
| **Rapid fire creates** | ✅ No hangs | Burst-create windows/panes without delays or orphaned processes |

### How it's fast

- **Lazy pane resize** : only the active window's panes are resized. Background windows resize on-demand when switched to, avoiding O(n) ConPTY syscalls
- **Cached shell resolution** : `which` PATH lookups are cached with `OnceLock`, not repeated per spawn
- **10ms polling** : client-server discovery uses tight 10ms polling for sub-100ms session attach
- **Early port-file write** : server writes its discovery file *before* spawning the first shell, so the client connects instantly
- **8KB reader buffers** : small buffer size minimizes mutex contention across pane reader threads

> **Note:** The primary startup bottleneck is your shell (PowerShell 7 takes ~400-1000ms to display a prompt). psmux itself adds < 100ms of overhead. For faster shells like `cmd.exe` or `nushell`, total startup is near-instant.

## tmux Compatibility

psmux is the most tmux-compatible terminal multiplexer on Windows:

| Feature | Support |
|---------|---------|
| Commands | **76** tmux commands implemented |
| Format variables | **126+** variables with full modifier support |
| Config file | Reads `~/.tmux.conf` directly |
| Key bindings | `bind-key`/`unbind-key` with key tables |
| Hooks | 15+ event hooks (`after-new-window`, etc.) |
| Status bar | Full format engine with conditionals and loops |
| Themes | 14 style options, 24-bit color, text attributes |
| Layouts | 5 layouts (even-h, even-v, main-h, main-v, tiled) |
| Copy mode | 53 vim keybindings, search, registers |
| Targets | `session:window.pane`, `%id`, `@id` syntax |
| `if-shell` / `run-shell` | ✅ Conditional config logic |
| Paste buffers | ✅ Full buffer management |

**Your existing `.tmux.conf` works.** psmux reads it automatically. Just install and go.

## Usage

Use `psmux`, `pmux`, or `tmux`, they're identical:

```powershell
# Start a new session
psmux
pmux
tmux

# Start a named session
psmux new-session -s work
tmux new-session -s work

# List sessions
psmux ls
tmux ls

# Attach to a session
psmux attach -t work
tmux attach -t work

# Show help
psmux --help
tmux --help
```

## Mouse Over SSH

When you SSH into a Windows machine running psmux, mouse support depends on the **server's** Windows build — not the client OS.

| Server Windows version | Mouse over SSH | Notes |
|---|:---:|---|
| Windows 11 build 22523+ (22H2+) | ✅ Works natively | Any SSH client from any OS |
| Windows 11 before build 22523 | ❌ No mouse | ConPTY limitation |
| Windows 10 (any build) | ❌ No mouse (workaround available) | ConPTY limitation |

### Windows 11 (build 22523+) — nothing to do

Just SSH in normally and run psmux. Mouse works from any client:

```bash
ssh user@windowshost   # Linux, macOS, WSL, Windows — all work
```

### Windows 10 — client-side workaround

ConPTY on Windows 10 consumes mouse-enable escape sequences before they reach sshd, so the SSH client never learns to send mouse data. The workaround is to enable mouse reporting directly on the **local** terminal before launching SSH:

**If you have psmux installed locally (Windows client):**
```powershell
psmux ssh user@win10host
```

**From Linux / macOS / WSL:**
```bash
# Using the helper script from the psmux repo
chmod +x psmux-ssh.sh
./psmux-ssh.sh user@win10host

# Or one-liner (no cleanup on exit):
printf '\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h'; ssh user@win10host
```

The wrapper writes DECSET mouse-enable sequences directly to the local terminal, which then forwards mouse events through the SSH connection. psmux's VT parser on the remote side decodes them.

> **Keyboard always works** regardless of OS version. The mouse limitation is specific to Windows 10's ConPTY.

## Key Bindings

Default prefix: `Ctrl+b` (same as tmux). Full reference: **[docs/keybindings.md](docs/keybindings.md)**

| Key | Action |
|-----|--------|
| `Prefix + c` | Create new window |
| `Prefix + %` / `"` | Split pane horizontally / vertically |
| `Prefix + Arrow` | Navigate between panes |
| `Prefix + d` | Detach from session |
| `Prefix + z` | Toggle pane zoom |
| `Prefix + [` | Enter copy/scroll mode (53 vim keybindings) |
| `Ctrl+q` | Quit |

## Scripting & Automation

76 tmux-compatible commands. Full reference: **[docs/scripting.md](docs/scripting.md)**

```powershell
psmux split-window -h                        # Split pane
psmux send-keys -t work:1.0 "ls" Enter       # Send keys to target
psmux capture-pane                            # Capture pane content
psmux set-hook -g after-new-window "display-message created"
```

## Configuration

Full reference: **[docs/configuration.md](docs/configuration.md)**

psmux reads `~/.psmux.conf`, `~/.psmuxrc`, `~/.tmux.conf`, or `~/.config/psmux/psmux.conf` (first found). Config syntax is **tmux-compatible** — most `.tmux.conf` lines work as-is.

```tmux
set -g prefix C-a
set -g mouse on
set -g status-style "bg=green,fg=black"
set -g default-shell pwsh
bind-key -T prefix h split-window -h
```

## License

MIT

---

## About psmux

**psmux** (PowerShell Multiplexer) is a terminal multiplexer **born in PowerShell, made in Rust**, built from scratch for Windows. Not a tmux port — a native Windows application that speaks fluent tmux.

### Star History

If psmux helps your Windows workflow, consider giving it a ⭐ on GitHub. It helps others find it!

### Contributing

Contributions are welcome! Whether it's:
- 🐛 Bug reports and feature requests via [GitHub Issues](https://github.com/marlocarlo/psmux/issues)
- 💻 Pull requests for fixes and features
- 📖 Documentation improvements
- 🧪 Test scripts and compatibility reports

### FAQ

See **[docs/faq.md](docs/faq.md)** for frequently asked questions.

---

<p align="center">
  Made with ❤️ for PowerShell using Rust 🦀
</p>
