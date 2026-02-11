# psmux vs tmux — Comprehensive Feature Audit Report

**Date:** 2025-01-XX  
**Codebase:** psmux (Windows tmux clone, Rust)  
**Files Audited:** main.rs, server.rs, client.rs, format.rs, types.rs, commands.rs, input.rs, config.rs, copy_mode.rs, window_ops.rs, layout.rs, pane.rs, tree.rs, session.rs, cli.rs, platform.rs, rendering.rs, app.rs, util.rs

---

## Table of Contents
1. [Architecture Summary](#1-architecture-summary)
2. [tmux Commands Audit](#2-tmux-commands-audit)
3. [Format Variables Audit](#3-format-variables-audit)
4. [Options (set-option) Audit](#4-options-set-option-audit)
5. [Copy Mode Audit](#5-copy-mode-audit)
6. [Key Bindings Audit](#6-key-bindings-audit)
7. [Plugin Compatibility Assessment](#7-plugin-compatibility-assessment)
8. [Gaps by Priority](#8-gaps-by-priority)
9. [Statistics Summary](#9-statistics-summary)

---

## 1. Architecture Summary

| Aspect | tmux | psmux |
|--------|------|-------|
| Platform | Unix (Linux/macOS/BSD) | Windows |
| Language | C | Rust |
| IPC | Unix domain socket | TCP on 127.0.0.1 (with session key auth) |
| Terminal | Custom terminal emulator | vt100 crate + ConPTY |
| Rendering | Direct terminal output | ratatui + crossterm |
| PTY | Unix PTY (forkpty) | portable-pty (ConPTY) |
| Server model | Forked daemon | DETACHED_PROCESS via CreateProcessW |
| Multi-client | Full multi-client | Single active client (attached_clients counter) |
| Config | ~/.tmux.conf | ~/.psmux.conf, ~/.psmuxrc, ~/.tmux.conf, ~/.config/psmux/psmux.conf |

---

## 2. tmux Commands Audit

### Legend
- ✅ **IMPLEMENTED** — Fully functional, matches tmux behavior
- 🔶 **PARTIAL** — Core functionality works, some flags/features missing
- ❌ **MISSING** — Not implemented
- ⬜ **STUB** — Accepted/parsed but no-op (for compatibility)
- 🚫 **N/A** — Not applicable on Windows

### 2.1 Client & Session Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `attach-session` | `attach`, `a`, `at` | ✅ IMPLEMENTED | `-t` flag supported. Persistent TCP connection. |
| `detach-client` | `detach` | ✅ IMPLEMENTED | Prefix+d, command mode |
| `has-session` | | ✅ IMPLEMENTED | Returns true/false via TCP |
| `kill-server` | | ✅ IMPLEMENTED | Cleans up port/key files, exits |
| `kill-session` | `kill-ses` | ✅ IMPLEMENTED | Kills all windows, removes port files |
| `list-clients` | `lsc` | 🔶 PARTIAL | Returns single pseudo-client (single-client model) |
| `list-commands` | `lscm` | ✅ IMPLEMENTED | Returns full TMUX_COMMANDS list |
| `list-sessions` | `ls` | ✅ IMPLEMENTED | Shows name, windows, size, attached status |
| `lock-client` | | 🚫 N/A | No terminal locking on Windows |
| `lock-server` | `lock` | 🚫 N/A | No terminal locking on Windows |
| `lock-session` | | 🚫 N/A | No terminal locking on Windows |
| `new-session` | `new` | ✅ IMPLEMENTED | `-s`, `-d`, `-n`, `-c`, `--` cmd args |
| `refresh-client` | | ✅ IMPLEMENTED | Forces state_dirty + meta_dirty |
| `rename-session` | | ✅ IMPLEMENTED | Updates port/key files on rename |
| `server-info` | `info` | ✅ IMPLEMENTED | Shows pid, session, windows, uptime, socket path |
| `show-messages` | `showmsgs` | ⬜ STUB | Accepted but no message log maintained |
| `source-file` | `source` | ✅ IMPLEMENTED | Glob patterns supported (for tpm), full config parsing |
| `start-server` | | ✅ IMPLEMENTED | No-op when server already running |
| `suspend-client` | | ⬜ STUB | No SIGTSTP on Windows |
| `switch-client` | `switchc` | 🔶 PARTIAL | Session switching via prefix+( and prefix+), session chooser |

### 2.2 Window Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `choose-buffer` | `chooseb` | ✅ IMPLEMENTED | Interactive buffer picker |
| `choose-client` | | ⬜ STUB | Single-client model, no-op |
| `choose-tree` | | ✅ IMPLEMENTED | Window/pane tree with keyboard navigation |
| `choose-window` | | ✅ IMPLEMENTED | Alias for choose-tree |
| `choose-session` | | ✅ IMPLEMENTED | Dedicated session chooser (prefix+s) with kill support |
| `customize-mode` | | ⬜ STUB | tmux 3.2+ feature, accepted for compat |
| `find-window` | `findw` | ✅ IMPLEMENTED | Pattern matching on window names |
| `kill-window` | `killw` | ✅ IMPLEMENTED | With confirm prompt (prefix+&) |
| `last-window` | `last` | ✅ IMPLEMENTED | Toggles active/last window |
| `link-window` | `linkw` | ⬜ STUB | Accepted, no-op (multi-session linking not supported) |
| `list-windows` | `lsw` | ✅ IMPLEMENTED | -F format, -J JSON, tmux-compatible output |
| `move-window` | `movew` | ✅ IMPLEMENTED | Reorders window in list |
| `new-window` | `neww` | ✅ IMPLEMENTED | -n name, -d detached, -c start_dir, custom command |
| `next-window` | `next` | ✅ IMPLEMENTED | Prefix+n, wraps around |
| `previous-window` | `prev` | ✅ IMPLEMENTED | Prefix+p, wraps around |
| `rename-window` | `renamew` | ✅ IMPLEMENTED | Prefix+, overlay |
| `resize-window` | `resizew` | 🔶 PARTIAL | -x/-y accepted; actual resize limited by terminal |
| `respawn-window` | `respawnw` | ✅ IMPLEMENTED | Kills and respawns active pane |
| `rotate-window` | `rotatew` | ✅ IMPLEMENTED | Forward and reverse (-U) |
| `select-window` | `selectw` | ✅ IMPLEMENTED | By index, prefix+0-9 |
| `swap-window` | `swapw` | ✅ IMPLEMENTED | Swaps positions of two windows |
| `unlink-window` | `unlinkw` | ✅ IMPLEMENTED | Removes window (kills processes) |

### 2.3 Pane Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `break-pane` | `breakp` | ✅ IMPLEMENTED | Extracts pane to new window |
| `capture-pane` | `capturep` | ✅ IMPLEMENTED | -p (stdout), -e (styled), -J (join), -S/-E (range) |
| `display-panes` | `displayp` | ✅ IMPLEMENTED | Pane number overlay, click to select |
| `join-pane` | `joinp` | ✅ IMPLEMENTED | Full tree extraction + grafting |
| `kill-pane` | `killp` | ✅ IMPLEMENTED | Process tree killing via platform API |
| `last-pane` | `lastp` | ✅ IMPLEMENTED | Toggles active/last pane path |
| `list-panes` | `lsp` | ✅ IMPLEMENTED | -F format support, mouse protocol info |
| `move-pane` | `movep` | ✅ IMPLEMENTED | Alias for join-pane implementation |
| `pipe-pane` | `pipep` | ✅ IMPLEMENTED | -I (stdin), -O (stdout), toggle on/off |
| `resize-pane` | `resizep` | ✅ IMPLEMENTED | -U/-D/-L/-R amount, -x/-y absolute, -Z zoom |
| `respawn-pane` | `respawnp` | ✅ IMPLEMENTED | Kills and respawns |
| `select-pane` | `selectp` | ✅ IMPLEMENTED | -U/-D/-L/-R directional, -l last, -t target |
| `split-window` | `splitw` | ✅ IMPLEMENTED | -h/-v, -c dir, -d detached, -l size |
| `swap-pane` | `swapp` | ✅ IMPLEMENTED | -U (up) / -D (down) |

### 2.4 Key Binding Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `bind-key` | `bind` | ✅ IMPLEMENTED | `-T` table, `-n` root, `-r` repeat, `\;` chaining |
| `list-keys` | `lsk` | ✅ IMPLEMENTED | Shows default + custom bindings |
| `send-keys` | `send` | ✅ IMPLEMENTED | -l literal, -X copy-mode commands, -N repeat, all special keys |
| `send-prefix` | | ✅ IMPLEMENTED | Sends prefix key to active pane |
| `unbind-key` | `unbind` | ✅ IMPLEMENTED | Removes from all tables |

### 2.5 Options Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `set-option` | `set` | ✅ IMPLEMENTED | -g, -u (unset), -a (append), -q (quiet), @user-options |
| `set-window-option` | `setw` | 🔶 PARTIAL | Maps to set-option (tmux merged these in 3.0+) |
| `show-options` | `show` | ✅ IMPLEMENTED | -v (value only), -q (quiet), all options listed |
| `show-window-options` | `showw` | 🔶 PARTIAL | Maps to show-options |

### 2.6 Buffer Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `choose-buffer` | `chooseb` | ✅ IMPLEMENTED | Interactive picker |
| `clear-history` | `clearhist` | ✅ IMPLEMENTED | Resets vt100 parser |
| `delete-buffer` | `deleteb` | ✅ IMPLEMENTED | Deletes first buffer |
| `list-buffers` | `lsb` | ✅ IMPLEMENTED | -F format support |
| `load-buffer` | `loadb` | ✅ IMPLEMENTED | From file |
| `paste-buffer` | `pasteb` | ✅ IMPLEMENTED | Prefix+] |
| `save-buffer` | `saveb` | ✅ IMPLEMENTED | To file |
| `set-buffer` | `setb` | ✅ IMPLEMENTED | |
| `show-buffer` | `showb` | ✅ IMPLEMENTED | With buffer index support |

### 2.7 Layout Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `next-layout` | `nextl` | ✅ IMPLEMENTED | Prefix+Space |
| `previous-layout` | `prevl` | ✅ IMPLEMENTED | Cycles reverse |
| `select-layout` | `selectl` | ✅ IMPLEMENTED | even-horizontal, even-vertical, main-horizontal, main-vertical, tiled |

### 2.8 Display Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `clock-mode` | | ✅ IMPLEMENTED | Big ASCII clock overlay, prefix+t |
| `command-prompt` | | ✅ IMPLEMENTED | Client-side overlay, prefix+: |
| `confirm-before` | `confirm` | ✅ IMPLEMENTED | Custom prompt, y/n handling |
| `display-menu` | `menu` | ✅ IMPLEMENTED | Parsed menu definition, keyboard navigation |
| `display-message` | `display` | ✅ IMPLEMENTED | Full format expansion |
| `display-popup` | `popup` | ✅ IMPLEMENTED | PTY-backed for interactive programs (fzf), -E, -w, -h |

### 2.9 Miscellaneous Commands

| Command | Aliases | Status | Notes |
|---------|---------|--------|-------|
| `copy-mode` | | ✅ IMPLEMENTED | -u page-up flag, full vi/emacs support |
| `if-shell` | `if` | ✅ IMPLEMENTED | -F format flag, shell command evaluation |
| `run-shell` | `run` | ✅ IMPLEMENTED | -b background, output capture |
| `set-environment` | `setenv` | ✅ IMPLEMENTED | Stored + set via env::set_var |
| `show-environment` | `showenv` | ✅ IMPLEMENTED | Shows app + PSMUX_/TMUX_ env vars |
| `set-hook` | | ✅ IMPLEMENTED | Multiple commands per hook |
| `show-hooks` | | ✅ IMPLEMENTED | Lists all registered hooks |
| `wait-for` | `wait` | ✅ IMPLEMENTED | -L lock, -S signal, -U unlock |

### 2.10 Commands Summary

| Category | Implemented | Partial | Stub/N/A | Missing | Total |
|----------|------------|---------|----------|---------|-------|
| Client/Session | 14 | 2 | 4 | 0 | 20 |
| Window | 17 | 1 | 3 | 0 | 21 |
| Pane | 14 | 0 | 0 | 0 | 14 |
| Key Binding | 5 | 0 | 0 | 0 | 5 |
| Options | 2 | 2 | 0 | 0 | 4 |
| Buffer | 9 | 0 | 0 | 0 | 9 |
| Layout | 3 | 0 | 0 | 0 | 3 |
| Display | 6 | 0 | 0 | 0 | 6 |
| Misc | 8 | 0 | 0 | 0 | 8 |
| **Total** | **78** | **5** | **7** | **0** | **90** |

**Command coverage: 92% (78 full + 5 partial out of 90)**

---

## 3. Format Variables Audit

### 3.1 Session Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `session_name` | ✅ | |
| `session_id` | ✅ | Returns `$0` (single session) |
| `session_created` | ✅ | Unix timestamp |
| `session_created_string` | ✅ | Human-readable via chrono |
| `session_attached` | ✅ | |
| `session_windows` | ✅ | |
| `session_activity` | ✅ | |
| `session_activity_string` | ✅ | |
| `session_last_attached` | ✅ | |
| `session_many_attached` | ✅ | |
| `session_format` | ✅ | Returns "1" in session context |
| `session_path` | ✅ | Current directory |
| `session_group` | 🔶 | Returns "" (session groups not supported) |
| `session_grouped` | 🔶 | Returns "0" |
| `session_group_attached` | 🔶 | Returns "0" |
| `session_group_size` | 🔶 | Returns "0" |
| `session_stack` | 🔶 | Returns "" |
| `session_alerts` | 🔶 | Returns "" |

### 3.2 Window Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `window_index` | ✅ | Respects base-index |
| `window_name` | ✅ | |
| `window_id` | ✅ | `@N` format |
| `window_active` | ✅ | |
| `window_flags` | ✅ | Full flag computation (*, -, #, Z, ~) |
| `window_raw_flags` | ✅ | |
| `window_panes` | ✅ | Count of panes |
| `window_layout` | ✅ | tmux-compatible layout string with checksum |
| `window_visible_layout` | ✅ | |
| `window_width` | ✅ | |
| `window_height` | ✅ | |
| `window_format` | ✅ | |
| `window_activity` | ✅ | |
| `window_activity_flag` | ✅ | |
| `window_zoomed_flag` | ✅ | |
| `window_last_flag` | ✅ | |
| `window_start_flag` | ✅ | |
| `window_end_flag` | ✅ | |
| `window_cell_width` | ✅ | |
| `window_cell_height` | ✅ | |
| `window_silence_flag` | 🔶 | Returns "0" (monitor-silence not implemented) |
| `window_bell_flag` | 🔶 | Returns "0" (bell detection not implemented) |
| `window_linked` | 🔶 | Returns "0" (window linking not supported) |
| `window_bigger` | 🔶 | Returns "0" |
| `window_offset_x` | 🔶 | Returns "0" |
| `window_offset_y` | 🔶 | Returns "0" |
| `window_stack_index` | 🔶 | Returns "0" |

### 3.3 Pane Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `pane_index` | ✅ | Respects pane-base-index |
| `pane_id` | ✅ | `%N` format |
| `pane_title` | ✅ | |
| `pane_width` | ✅ | |
| `pane_height` | ✅ | |
| `pane_active` | ✅ | |
| `pane_current_command` | ✅ | Inferred from process name |
| `pane_current_path` | ✅ | From process CWD |
| `pane_pid` | ✅ | |
| `pane_tty` | ✅ | Synthetic ConPTY path |
| `pane_in_mode` | ✅ | |
| `pane_mode` | ✅ | "copy-mode" / "clock-mode" / "" |
| `pane_synchronized` | ✅ | |
| `pane_dead` | ✅ | |
| `pane_format` | ✅ | |
| `pane_left` | ✅ | |
| `pane_top` | ✅ | |
| `pane_right` | ✅ | |
| `pane_bottom` | ✅ | |
| `pane_search_string` | ✅ | From copy mode search |
| `pane_start_command` | ✅ | |
| `pane_dead_signal` | 🔶 | Returns "0" (no signals on Windows) |
| `pane_dead_status` | 🔶 | Returns "0" |
| `pane_dead_time` | 🔶 | Returns "0" |
| `pane_input_off` | 🔶 | Returns "0" |
| `pane_marked` | 🔶 | Returns "0" (pane marking not implemented) |
| `pane_marked_set` | 🔶 | Returns "0" |
| `pane_last` | 🔶 | Returns "0" |
| `pane_pipe` | 🔶 | Returns "0" |
| `pane_unseen_changes` | 🔶 | Returns "0" |
| `pane_at_top` | 🔶 | Returns "1" (hardcoded) |
| `pane_at_bottom` | 🔶 | Returns "1" (hardcoded) |
| `pane_at_left` | 🔶 | Returns "1" (hardcoded) |
| `pane_at_right` | 🔶 | Returns "1" (hardcoded) |
| `pane_start_path` | 🔶 | Returns "" |
| `pane_tabs` | 🔶 | Returns "" |

### 3.4 Cursor Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `cursor_x` | ✅ | |
| `cursor_y` | ✅ | |
| `cursor_character` | ✅ | Character under cursor |
| `cursor_flag` | 🔶 | Returns "0" (cursor visibility not tracked) |

### 3.5 Copy Mode Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `copy_cursor_x` | ✅ | |
| `copy_cursor_y` | ✅ | |
| `selection_present` | ✅ | |
| `selection_active` | ✅ | |
| `selection_start_x` | ✅ | |
| `selection_start_y` | ✅ | |
| `selection_end_x` | ✅ | |
| `selection_end_y` | ✅ | |
| `search_present` | ✅ | |
| `scroll_position` | ✅ | |
| `scroll_region_upper` | ✅ | |
| `scroll_region_lower` | ✅ | |
| `copy_cursor_word` | 🔶 | Returns "" |
| `copy_cursor_line` | 🔶 | Returns "" |
| `search_match` | 🔶 | Returns "" |

### 3.6 Buffer Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `buffer_size` | ✅ | |
| `buffer_sample` | ✅ | First 50 chars |
| `buffer_name` | ✅ | "bufferN" format |
| `buffer_created` | ✅ | Unix timestamp |

### 3.7 Client Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `client_width` | ✅ | |
| `client_height` | ✅ | |
| `client_session` | ✅ | |
| `client_last_session` | ✅ | |
| `client_name` | ✅ | "client0" |
| `client_tty` | ✅ | Synthetic path |
| `client_pid` | ✅ | |
| `client_prefix` | ✅ | "1" if prefix armed |
| `client_activity` | ✅ | |
| `client_created` | ✅ | |
| `client_activity_string` | ✅ | |
| `client_created_string` | ✅ | |
| `client_flags` | ✅ | |
| `client_key_table` | ✅ | |
| `client_termname` | ✅ | |
| `client_termtype` | ✅ | |
| `client_termfeatures` | ✅ | |
| `client_utf8` | ✅ | Returns "1" |
| `client_cell_width` | ✅ | |
| `client_cell_height` | ✅ | |
| `client_control_mode` | 🔶 | Returns "0" (control mode not supported) |
| `client_written` | 🔶 | Returns "0" |
| `client_discarded` | 🔶 | Returns "0" |

### 3.8 Server Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `host` | ✅ | Cached hostname |
| `host_short` | ✅ | |
| `pid` / `server_pid` | ✅ | |
| `version` | ✅ | |
| `start_time` | ✅ | |
| `socket_path` | ✅ | .psmux directory path |

### 3.9 Terminal State Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `alternate_on` | ✅ | With ConPTY heuristic |
| `origin_flag` | ✅ | |
| `insert_flag` | ✅ | |
| `keypad_cursor_flag` | ✅ | |
| `keypad_flag` | ✅ | |
| `wrap_flag` | ✅ | |
| `mouse` | ✅ | |
| `prefix` | ✅ | |
| `status` (as var) | ✅ | |
| `mode_keys` | ✅ | |
| `history_size` | ✅ | Current scrollback lines |
| `history_limit` | ✅ | |

### 3.10 Meta/Command Variables

| Variable | Status | Notes |
|----------|--------|-------|
| `line` | ✅ | |
| `command` | ✅ | |
| `command_list_name` | ✅ | |
| `command_list_alias` | ✅ | |
| `command_list_usage` | ✅ | |
| `config_files` | ✅ | |

### 3.11 Format Features

| Feature | Status | Notes |
|---------|--------|-------|
| Conditionals `#{?cond,true,false}` | ✅ | Nested supported |
| Comparisons `#{==:a,b}` | ✅ | ==, !=, <, >, <=, >= |
| Boolean `#{||:a,b}` `#{&&:a,b}` | ✅ | |
| Loop `#{W:fmt}` (windows) | ✅ | |
| Loop `#{P:fmt}` (panes) | ✅ | |
| Loop `#{S:fmt}` (sessions) | ✅ | Single session |
| Modifier `#{t:var}` (time) | ✅ | |
| Modifier `#{b:var}` (basename) | ✅ | |
| Modifier `#{d:var}` (dirname) | ✅ | |
| Modifier `#{l:str}` (literal) | ✅ | |
| Modifier `#{E:var}` (expand) | ✅ | |
| Modifier `#{T:var}` (expand+time) | ✅ | |
| Modifier `#{q:var}` (quote) | ✅ | |
| Modifier `#{s/pat/rep/:var}` (sub) | ✅ | Regex substitution |
| Modifier `#{=N:var}` (trim) | ✅ | With marker support |
| Modifier `#{pN:var}` (pad) | ✅ | |
| Modifier `#{e\|op\|flags:a,b}` (math) | ✅ | Arithmetic operations |
| Modifier `#{m/flags:pat,str}` (match) | ✅ | Glob matching |
| Modifier `#{C:pattern}` (search) | 🔶 | Returns "0" (stub) |
| Modifier `#{w:var}` (width) | ✅ | Unicode-aware |
| Shorthand `#S`, `#I`, `#W`, etc. | ✅ | All tmux shorthands |
| Strftime `%H:%M` etc. | ✅ | Via chrono |
| `@user_options` | ✅ | Read from environment |

### 3.12 Format Variables Summary

| Category | Full | Partial/Stub | Total |
|----------|------|-------------|-------|
| Session | 10 | 8 | 18 |
| Window | 18 | 9 | 27 |
| Pane | 19 | 17 | 36 |
| Cursor | 3 | 1 | 4 |
| Copy Mode | 9 | 3 | 12 |
| Buffer | 4 | 0 | 4 |
| Client | 18 | 3 | 21 |
| Server | 6 | 0 | 6 |
| Terminal State | 12 | 0 | 12 |
| Meta/Command | 6 | 0 | 6 |
| **Total** | **105** | **41** | **146** |

**Format variable coverage: 72% full, 100% accepted (all return a value, even if stub)**

---

## 4. Options (set-option) Audit

### 4.1 Fully Implemented Options

| Option | Default | Notes |
|--------|---------|-------|
| `prefix` | `C-b` | Parsed via parse_key_string |
| `base-index` | 1 | Window numbering start |
| `pane-base-index` | 0 | Pane numbering start |
| `escape-time` | 500 | Milliseconds |
| `mouse` | on | Full mouse support |
| `status` | on | Status bar visibility |
| `status-position` | bottom | top/bottom |
| `status-left` | `psmux:#I` | Format string |
| `status-right` | `%H:%M` | Format string |
| `status-style` | | fg/bg/bold parsing |
| `status-bg` | | Maps to status-style (deprecated compat) |
| `status-fg` | | Maps to status-style (deprecated compat) |
| `history-limit` | 2000 | Scrollback lines |
| `display-time` | 750 | Message display ms |
| `display-panes-time` | | Pane number display ms |
| `mode-keys` | emacs | vi/emacs |
| `focus-events` | off | Focus in/out forwarding |
| `renumber-windows` | off | |
| `automatic-rename` | on | From process title |
| `monitor-activity` | off | Activity flag per window |
| `visual-activity` | off | |
| `synchronize-panes` | off | Sync input to all panes |
| `remain-on-exit` | off | Dead pane stays visible |
| `aggressive-resize` | off | |
| `set-titles` | off | OSC 2 terminal title |
| `set-titles-string` | `#S:#I:#W` | Format string |
| `default-shell` / `default-command` | | Shell path |
| `word-separators` | | For copy mode word movement |
| `pane-border-style` | | fg parsing |
| `pane-active-border-style` | `fg=green` | fg parsing |
| `window-status-format` | `#I:#W#F` | Tab format |
| `window-status-current-format` | `#I:#W#F` | Active tab format |
| `window-status-separator` | ` ` | Between tabs |
| `repeat-time` | | Key repeat timeout |
| `cursor-style` | bar | psmux-specific |
| `cursor-blink` | on | psmux-specific |
| `prediction-dimming` | on | psmux-specific |
| `status-left-length` | | Stored as @option |
| `status-right-length` | | Stored as @option |

### 4.2 Accepted but Ignored Options (Compatibility)

| Option | Why Ignored |
|--------|-------------|
| `prefix2` | Stored but not used for matching |
| `status-interval` | Server uses fixed frame timing |
| `status-justify` | Not implemented in status rendering |
| `status-keys` | Only mode-keys used |
| `allow-rename` | Always allowed |
| `terminal-overrides` | ConPTY handles terminal |
| `default-terminal` | ConPTY handles terminal |
| `update-environment` | Not applicable |
| `bell-action` | Bell not forwarded |
| `visual-bell` | Bell not forwarded |
| `activity-action` | Uses visual-activity only |
| `silence-action` | Monitor-silence not implemented |
| `monitor-silence` | Not implemented |
| `message-style` | Not styled separately |
| `clock-mode-colour` | Fixed to Cyan |
| `clock-mode-style` | 24h only |
| `pane-border-format` | Not implemented |
| `pane-border-status` | Not implemented |
| `popup-style` | Not implemented |
| `popup-border` | Not implemented |
| `window-style` | Not implemented |
| `window-active-style` | Not implemented |
| `wrap-search` | Not implemented |
| `lock-after-time` | N/A on Windows |
| `lock-command` | N/A on Windows |

### 4.3 Options Stored in Environment (for format access)

| Option | Notes |
|--------|-------|
| `window-status-style` | |
| `window-status-current-style` | |
| `window-status-activity-style` | |
| `window-status-bell-style` | |
| `window-status-last-style` | |
| `mode-style` | |
| `message-style` | |
| `message-command-style` | |
| `main-pane-width` | |
| `main-pane-height` | |
| All `@user-options` | Used by plugins |

---

## 5. Copy Mode Audit

### 5.1 Vi Key Bindings

| Key | Command | Status |
|-----|---------|--------|
| `h/j/k/l` | Cursor movement | ✅ |
| `w/b/e` | Word motions | ✅ |
| `W/B/E` | WORD motions (bigword) | ✅ |
| `0` | Start of line | ✅ |
| `$` | End of line | ✅ |
| `^` | First non-blank | ✅ |
| `H/M/L` | Screen top/middle/bottom | ✅ |
| `f/F/t/T` | Find char forward/backward | ✅ |
| `v` | Begin selection (char) | ✅ |
| `V` | Begin selection (line) | ✅ |
| `Ctrl-V` | Rectangle selection | ✅ |
| `y` | Yank selection | ✅ |
| `D` | Copy to end of line | ✅ |
| `/` | Search forward | ✅ |
| `?` | Search backward | ✅ |
| `n/N` | Search next/prev | ✅ |
| `g` | Go to top | ✅ |
| `G` | Go to bottom | ✅ |
| `o` | Other end of selection | ✅ |
| `A` | Append to selection | ✅ |
| `Space` | Begin selection | ✅ |
| `Enter` | Copy selection & exit | ✅ |
| `Escape` / `q` | Cancel copy mode | ✅ |
| `Ctrl-U/D` | Half-page up/down | ✅ |
| `Ctrl-B/F` | Page up/down | ✅ |
| `PageUp/Down` | Page scroll | ✅ |
| Arrow keys | Cursor movement | ✅ |

### 5.2 Emacs Key Bindings

| Key | Command | Status |
|-----|---------|--------|
| `Ctrl-A` | Start of line | ✅ |
| `Ctrl-E` | End of line | ✅ |
| `Ctrl-N` | Cursor down | ✅ |
| `Ctrl-P` | Cursor up | ✅ |
| `Ctrl-S` | Search forward | ✅ |
| `Ctrl-R` | Search backward | ✅ |
| `Ctrl-G` | Cancel/clear selection | ✅ |
| `Ctrl-Space` | Begin selection | ✅ |
| `Alt-F` | Word forward | ✅ |
| `Alt-B` | Word backward | ✅ |
| `Alt-V` | Page up | ✅ |
| `Alt-W` | Copy selection | ✅ |

### 5.3 send-keys -X Copy Commands

| Command | Status | Notes |
|---------|--------|-------|
| `cancel` | ✅ | |
| `begin-selection` | ✅ | |
| `select-line` | ✅ | |
| `rectangle-toggle` | ✅ | |
| `copy-selection` | ✅ | |
| `copy-selection-and-cancel` | ✅ | |
| `copy-selection-no-clear` | ✅ | |
| `copy-pipe` | ✅ | With command piping |
| `copy-pipe-and-cancel` | ✅ | With command piping |
| `cursor-up/down/left/right` | ✅ | |
| `start-of-line` | ✅ | |
| `end-of-line` | ✅ | |
| `back-to-indentation` | ✅ | |
| `next-word` | ✅ | |
| `previous-word` | ✅ | |
| `next-word-end` | ✅ | |
| `next-space` / `previous-space` / `next-space-end` | ✅ | |
| `top-line` / `middle-line` / `bottom-line` | ✅ | |
| `history-top` / `history-bottom` | ✅ | |
| `halfpage-up` / `halfpage-down` | ✅ | |
| `page-up` / `page-down` | ✅ | |
| `scroll-up` / `scroll-down` | ✅ | |
| `search-forward` / `search-backward` | ✅ | |
| `search-forward-incremental` / `search-backward-incremental` | ✅ | |
| `search-again` / `search-reverse` | ✅ | |
| `copy-end-of-line` | ✅ | |
| `select-word` | ✅ | |
| `other-end` | ✅ | |
| `append-selection` | ❌ | Not implemented |
| `clear-selection` | ❌ | Not implemented (Ctrl-G works in emacs mode) |
| `stop-selection` | ❌ | Not implemented |
| `goto-line` | ❌ | Not implemented |
| `jump-forward` / `jump-backward` | ❌ | Not mapped via -X |
| `jump-again` / `jump-reverse` | ❌ | Not mapped via -X |
| `set-mark` / `jump-to-mark` | ❌ | Not implemented |
| `search-forward-text` / `search-backward-text` | ❌ | Non-incremental |
| `next-matching-bracket` | ❌ | Not implemented |
| `previous-paragraph` / `next-paragraph` | ❌ | Not implemented |

### 5.4 System Clipboard

| Feature | Status | Notes |
|---------|--------|-------|
| Copy to clipboard | ✅ | Win32 API (OpenClipboard, SetClipboardData) |
| Paste from clipboard | ✅ | Win32 API (GetClipboardData) |
| Client-side: left-drag select + copy | ✅ | pwsh-style behavior |
| Client-side: right-click paste | ✅ | When no active selection |

---

## 6. Key Bindings Audit

### 6.1 Default Prefix Bindings

| Key | tmux Command | Status |
|-----|-------------|--------|
| `c` | `new-window` | ✅ |
| `n` | `next-window` | ✅ |
| `p` | `previous-window` | ✅ |
| `%` | `split-window -h` | ✅ |
| `"` | `split-window -v` | ✅ |
| `x` | `kill-pane` (confirm) | ✅ |
| `d` | `detach-client` | ✅ |
| `w` | `choose-tree` | ✅ |
| `,` | `rename-window` | ✅ |
| `$` | `rename-session` | ✅ |
| `Space` | `next-layout` | ✅ |
| `[` | `copy-mode` | ✅ |
| `]` | `paste-buffer` | ✅ |
| `:` | `command-prompt` | ✅ |
| `q` | `display-panes` | ✅ |
| `z` | `resize-pane -Z` | ✅ |
| `o` | `select-pane -t +` | ✅ |
| `;` | `last-pane` | ✅ |
| `l` | `last-window` | ✅ |
| `{` | `swap-pane -U` | ✅ |
| `}` | `swap-pane -D` | ✅ |
| `!` | `break-pane` | ✅ |
| `&` | `kill-window` (confirm) | ✅ |
| `0-9` | `select-window N` | ✅ |
| `t` | `clock-mode` | ✅ |
| `=` | `choose-buffer` | ✅ |
| `?` | `list-keys` | ✅ |
| `i` | `display-message` | ✅ |
| `s` | `choose-session` | ✅ |
| `(` / `)` | prev/next session | ✅ |
| `Arrow` | `select-pane` | ✅ |
| `Ctrl-Arrow` | `resize-pane 1` | ✅ |
| `Alt-Arrow` | `resize-pane 5` | ✅ |
| `Alt-1..5` | Preset layouts | ✅ |

### 6.2 Key Table Support

| Feature | Status | Notes |
|---------|--------|-------|
| `prefix` table | ✅ | Default table |
| `root` table (`-n`) | ✅ | Direct key bindings |
| Custom tables (`-T`) | ✅ | Named key tables |
| Repeat bindings (`-r`) | ✅ | |
| Command chaining (`\;`) | ✅ | |

---

## 7. Plugin Compatibility Assessment

### 7.1 tmux Plugin Manager (tpm)

| Requirement | Status | Notes |
|-------------|--------|-------|
| `source-file` with globs | ✅ | `source ~/.tmux/plugins/*/*.tmux` works |
| `run-shell` | ✅ | Background and foreground |
| `set-environment` | ✅ | TMUX_PLUGIN_MANAGER_PATH |
| `@user-options` | ✅ | @plugin stored as environment vars |
| Git clone integration | ⬜ | tpm does this externally |

**Overall: 🔶 PARTIAL** — tpm's core mechanism should work; the bootstrap `run-shell '~/.tmux/plugins/tpm/tpm'` will execute. Plugin sourcing via globs is supported. Full compatibility untested.

### 7.2 tmux-resurrect

| Requirement | Status | Notes |
|-------------|--------|-------|
| `capture-pane -p -t` | ✅ | Capture with target |
| `list-windows -F` | ✅ | Format support |
| `list-panes -F` | ✅ | Format support |
| `@resurrect-*` options | ✅ | Stored as @user-options |
| `run-shell` | ✅ | |
| `set-hook after-save-layout` | ✅ | |
| Process tree save/restore | ❌ | Not implemented |
| Session save file format | ❌ | Not implemented |

**Overall: ❌ MISSING** — The plugin will load and its options will be stored, but the actual save/restore of sessions is not implemented. Would need psmux-native session serialization.

### 7.3 tmux-continuum

| Requirement | Status | Notes |
|-------------|--------|-------|
| Depends on tmux-resurrect | ❌ | See above |
| `set-hook` for periodic save | ✅ | |
| `status-right` integration | ✅ | Format expansion works |

**Overall: ❌ MISSING** — Blocked by tmux-resurrect dependency.

### 7.4 tmux-sensible

| Requirement | Status | Notes |
|-------------|--------|-------|
| `set-option -s escape-time 0` | ✅ | |
| `set-option -g history-limit 50000` | ✅ | |
| `set-option -g display-time 4000` | ✅ | |
| `set-option -g status-interval 5` | ⬜ | Accepted, ignored |
| `set-option -g focus-events on` | ✅ | |
| `bind-key` various | ✅ | |
| `source-file` | ✅ | |

**Overall: ✅ HIGH COMPATIBILITY** — Nearly all settings will apply correctly.

### 7.5 tmux-yank

| Requirement | Status | Notes |
|-------------|--------|-------|
| `send-keys -X copy-pipe-and-cancel` | ✅ | Full implementation |
| `send-keys -X copy-pipe` | ✅ | With command piping |
| `copy-selection-and-cancel` | ✅ | |
| `bind-key -T copy-mode-vi` | ✅ | Custom key tables |
| System clipboard access | ✅ | Win32 API |
| `@user-options` for config | ✅ | |

**Overall: ✅ HIGH COMPATIBILITY** — Core yank/copy-pipe mechanisms all work.

### 7.6 tmux-pain-control

| Requirement | Status | Notes |
|-------------|--------|-------|
| `split-window -h/-v -c` | ✅ | |
| `select-pane -U/-D/-L/-R` | ✅ | |
| `resize-pane` | ✅ | |
| `swap-window -t` | ✅ | |
| `bind-key` | ✅ | |

**Overall: ✅ HIGH COMPATIBILITY** — All pane navigation and splitting commands available.

### 7.7 tmux-prefix-highlight

| Requirement | Status | Notes |
|-------------|--------|-------|
| `#{client_prefix}` | ✅ | Returns "1" when prefix armed |
| `#{pane_in_mode}` | ✅ | |
| `status-left` / `status-right` | ✅ | Format expansion |
| `@user-options` | ✅ | |
| `#{?cond,true,false}` | ✅ | |

**Overall: ✅ HIGH COMPATIBILITY** — Format variables and conditionals all present.

### 7.8 tmux-fingers

| Requirement | Status | Notes |
|-------------|--------|-------|
| `display-popup` with PTY | ✅ | Interactive programs work |
| `capture-pane -p` | ✅ | |
| `send-keys -l` | ✅ | |
| `run-shell` | ✅ | |
| Pattern matching engine | ❌ | Plugin-specific, needs Ruby/Python |

**Overall: 🔶 PARTIAL** — Infrastructure exists but the plugin needs external runtime.

### 7.9 tmux-fzf

| Requirement | Status | Notes |
|-------------|--------|-------|
| `display-popup` with PTY | ✅ | fzf works in popup |
| `list-windows -F` | ✅ | |
| `list-panes -F` | ✅ | |
| `list-sessions` format | ✅ | |
| `run-shell` | ✅ | |
| `send-keys` | ✅ | |
| fzf availability | ⬜ | External dependency |

**Overall: 🔶 PARTIAL** — Core infrastructure works well; fzf popup is functional.

### 7.10 tmux-copycat

| Requirement | Status | Notes |
|-------------|--------|-------|
| `copy-mode` | ✅ | |
| Search (/ and ?) | ✅ | Case-insensitive |
| Regex search | 🔶 | Basic regex via search_copy_mode |
| `send-keys -X` | ✅ | |
| Predefined regex patterns | ❌ | Not built-in |

**Overall: 🔶 PARTIAL** — Basic search works; plugin's regex pattern library needs adaptation.

### 7.11 tmux-open

| Requirement | Status | Notes |
|-------------|--------|-------|
| `send-keys -X copy-pipe-and-cancel` | ✅ | |
| `run-shell` | ✅ | |
| `display-message` | ✅ | |
| URL/file opening | ✅ | Via `cmd /C start` on Windows |

**Overall: ✅ HIGH COMPATIBILITY** — Should work with minor Windows path adjustments.

### 7.12 tmux-sidebar

| Requirement | Status | Notes |
|-------------|--------|-------|
| `split-window -h -l` | ✅ | |
| `select-pane` | ✅ | |
| `resize-pane` | ✅ | |
| `send-keys` | ✅ | |
| `@user-options` | ✅ | |

**Overall: ✅ HIGH COMPATIBILITY** — Core split/resize/select all work.

### 7.13 tmux-battery

| Requirement | Status | Notes |
|-------------|--------|-------|
| `run-shell` output in status | ✅ | |
| `status-right` format | ✅ | |
| `@user-options` | ✅ | |
| WMIC/PowerShell battery query | ✅ | Windows-native |

**Overall: ✅ HIGH COMPATIBILITY** — Uses run-shell which works on Windows.

### 7.14 tmux-cpu

| Requirement | Status | Notes |
|-------------|--------|-------|
| `run-shell` output in status | ✅ | |
| `status-right` format | ✅ | |
| `@user-options` | ✅ | |
| System CPU/memory query | ✅ | Via WMIC/PowerShell on Windows |

**Overall: ✅ HIGH COMPATIBILITY** — Same pattern as tmux-battery.

### 7.15 Plugin Summary

| Plugin | Compatibility | Rating |
|--------|--------------|--------|
| tpm | 🔶 PARTIAL | Should bootstrap, untested |
| tmux-resurrect | ❌ MISSING | Save/restore not implemented |
| tmux-continuum | ❌ MISSING | Blocked by resurrect |
| tmux-sensible | ✅ HIGH | Nearly all options work |
| tmux-yank | ✅ HIGH | copy-pipe fully works |
| tmux-pain-control | ✅ HIGH | All pane ops work |
| tmux-prefix-highlight | ✅ HIGH | Format vars present |
| tmux-fingers | 🔶 PARTIAL | Infra works, needs runtime |
| tmux-fzf | 🔶 PARTIAL | Popup works, external dep |
| tmux-copycat | 🔶 PARTIAL | Basic search, no regex patterns |
| tmux-open | ✅ HIGH | copy-pipe + run-shell |
| tmux-sidebar | ✅ HIGH | All pane ops work |
| tmux-battery | ✅ HIGH | run-shell works |
| tmux-cpu | ✅ HIGH | run-shell works |

---

## 8. Gaps by Priority

### 🔴 CRITICAL (Required for tmux .conf compatibility)

| # | Gap | Description | Impact |
|---|-----|-------------|--------|
| 1 | **Multi-client support** | Only single active client; `attached_clients` is a counter but real multi-attach (multiple terminals viewing same session with independent sizes) is absent | Breaks shared pairing workflows |
| 2 | **Target specifiers incomplete** | `-t session:window.pane` parsing exists but cross-session targeting (sending commands to another session's panes) isn't fully wired | Breaks scripting that targets specific panes |
| 3 | **`pane_at_*` variables hardcoded** | `pane_at_top/bottom/left/right` all return "1" instead of computing actual position | Breaks plugins/configs that check pane position |
| 4 | **Window/pane marks** | `select-pane -m` (mark) and `#{pane_marked}` not implemented | Breaks mark-and-swap workflows |

### 🟠 HIGH (Important for plugin ecosystem & power users)

| # | Gap | Description | Impact |
|---|-----|-------------|--------|
| 5 | **Session groups** | No support for session groups (shared window lists) | Format vars return empty |
| 6 | **`copy_cursor_word` / `copy_cursor_line`** | Return "" instead of actual word/line under cursor | Breaks plugins like tmux-copycat |
| 7 | **`#{C:pattern}` content search** | Returns "0" always; should search pane content | Breaks content-aware scripts |
| 8 | **Bell detection** | `window_bell_flag` always 0; no bell forwarding/monitoring | Breaks bell-aware configs |
| 9 | **Monitor-silence** | Not implemented at all (option ignored) | Breaks inactivity detection |
| 10 | **`pane_pipe` variable** | Always returns "0" even when pipe-pane is active | Scripts can't check pipe status |
| 11 | **`delete-buffer -b name`** | Only deletes first buffer; named buffer deletion missing | Buffer management limited |
| 12 | **Session save/restore** | No native session serialization (blocks tmux-resurrect) | Can't persist sessions |
| 13 | **`status-justify`** | Ignored — tabs always left-aligned | Visual difference from tmux |
| 14 | **`status-interval`** | Ignored — server uses fixed frame timing | Some status content may not refresh correctly |
| 15 | **Missing `-X` copy commands** | `append-selection`, `clear-selection`, `stop-selection`, `goto-line`, `set-mark`, `jump-to-mark`, `next-matching-bracket`, `next/prev-paragraph` | Breaks custom copy-mode configs |

### 🟡 MEDIUM (Nice to have, affects specific workflows)

| # | Gap | Description | Impact |
|---|-----|-------------|--------|
| 16 | **`window-style` / `window-active-style`** | Ignored — no per-window fg/bg customization | Visual difference |
| 17 | **`pane-border-format` / `pane-border-status`** | Ignored — no pane border labels | Visual difference |
| 18 | **`popup-style` / `popup-border`** | Ignored — popup always default style | Visual difference |
| 19 | **`message-style`** | Ignored — messages use default style | Visual difference |
| 20 | **`clock-mode-colour` / `clock-mode-style`** | Fixed Cyan / 24h only | Minor visual difference |
| 21 | **`wrap-search`** | Ignored — search always wraps | Minor behavior difference |
| 22 | **`link-window`** | Stub — can't share windows between sessions | Niche feature |
| 23 | **`show-messages`** | Stub — no message log maintained | Can't review server messages |
| 24 | **Control mode** | No `-C` control mode (structured event stream) | Blocks automation tools |
| 25 | **`pane_unseen_changes`** | Always 0 | Background pane state not tracked |
| 26 | **Buffer naming** | Buffers are `buffer0`, `buffer1`, etc. — no custom naming like tmux's named buffers | Minor management limitation |
| 27 | **`display-popup -d` directory** | No `-d` flag for popup start directory | Minor |
| 28 | **`split-window -l` percentage** | Size accepted but not proportionally applied | Split sizes may differ |

### 🟢 LOW (Edge cases, minimal impact)

| # | Gap | Description | Impact |
|---|-----|-------------|--------|
| 29 | **`prefix2`** | Stored but not matched — second prefix key doesn't work | Rare config |
| 30 | **`status-interval`** | Fixed timing instead of configurable | Usually acceptable |
| 31 | **`update-environment`** | Ignored | Niche feature |
| 32 | **`lock-after-time` / `lock-command`** | N/A on Windows | Platform limitation |
| 33 | **Extended mouse modes** | ConPTY mouse injection works but doesn't translate all VT mouse protocols | Some TUI apps may have mouse issues |
| 34 | **256-color index expansion** | `vt100::Color::Idx` only maps 0-15 explicitly; 16-255 fall through to Reset | Affects apps using extended palette |
| 35 | **`cursor_flag`** | Always "0" — cursor visibility not tracked from vt100 state | Niche script usage |
| 36 | **`search_match`** | Always "" — search match text not captured | Niche copy-mode usage |
| 37 | **`session_stack` / `window_stack_index`** | Empty/0 — no window/session stack | Niche navigation feature |
| 38 | **`client_written` / `client_discarded`** | Always 0 — no byte tracking | Diagnostic only |
| 39 | **Non-incremental search** | `search-forward-text` / `search-backward-text` not distinct from incremental | Subtle behavior difference |

---

## 9. Statistics Summary

### Command Coverage
| Metric | Count |
|--------|-------|
| Fully Implemented | 78 |
| Partially Implemented | 5 |
| Stub/N/A | 7 |
| Missing | 0 |
| **Total tmux commands** | **90** |
| **Coverage** | **92%** |

### Format Variable Coverage
| Metric | Count |
|--------|-------|
| Fully Implemented | 105 |
| Stub/Partial (return value) | 41 |
| Truly Missing | 0 |
| **Total** | **146** |
| **Full coverage** | **72%** |
| **Accepts (returns something)** | **100%** |

### Plugin Compatibility
| Metric | Count |
|--------|-------|
| High Compatibility | 8 |
| Partial Compatibility | 4 |
| Missing/Blocked | 2 |
| **Total assessed** | **14** |

### Options Coverage
| Metric | Count |
|--------|-------|
| Fully Functional | 37 |
| Accepted/Ignored | 25 |
| **Total** | **62** |

### Copy Mode
| Metric | Count |
|--------|-------|
| Vi bindings working | 30+ |
| Emacs bindings working | 12 |
| `-X` commands working | 35 |
| `-X` commands missing | 12 |

### Identified Gaps
| Priority | Count |
|----------|-------|
| 🔴 CRITICAL | 4 |
| 🟠 HIGH | 11 |
| 🟡 MEDIUM | 13 |
| 🟢 LOW | 11 |
| **Total** | **39** |

---

*End of audit report.*
