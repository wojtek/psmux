# Elysium psmux fork

This fork follows upstream psmux and keeps only behavior the Elysium workflow
still requires and upstream does not yet provide.

The current base is `marlocarlo/master` at
`a055bf1fa1f1f76d1b9fc5d0fadf3a2b8c406424` (2026-10-16). The maintained layer
is reapplied on that tip, without the previous fork history.

## Maintained differences

- `send-keys` recognizes options only before `--` or the first key operand.
  Later tokens beginning with `-` are sent to the pane instead of being dropped
  or reinterpreted as targets. `cli::parse_send_keys_args` is the one parser
  both the one-shot and the control route read, so a flag means the same thing
  on both. Its help routes are side-effect-free, and an unknown long option
  before the operand boundary fails with a literal-input hint. `send-keys -R`
  resets the target pane's parsed terminal state and screen.
  Upstream's `normalize_command_flags`, `normalize_attached_target_flag` and
  `validate_flag_arguments` stop at `--`, at a bare `-` and at the first token
  that does not begin with `-`, using each command's tmux flag table (#795).
  Its `outer_target_scan_end` only bounds deferred commands and explicit `--`.
  Those boundaries differ from the fork's: a dash-leading first key such as
  `-z` or `-N3` does not end their region. All four now stop at the send-keys
  operand boundary
  instead — the first token `cli::send_keys_option_token` does not accept — so
  the normalizers, the missing-value validator, the leading-`-t` scan and the
  shared parser agree. Without that, `send-keys -l -z -t=foo` reached the pane
  as `-z-tfoo`, `send-keys -l -N3 -t=foo` as `-N3-tfoo`, a value-less-looking
  literal `-t` after the boundary failed validation as a missing target, and a
  literal detached `-t <name>` among the keys was resolved on live state and
  rewritten to a `%id`.

  The boundary accepts exactly the forms a normalizer really splits before the
  shared parser runs: a recognized option cluster, the `=`-form of a
  value-taking flag (`-t=foo`, `-N=3`), and the attached form of `-t` alone
  (`-tfoo`). `normalize_command_flags` splits that attached `-t` for send-keys
  on its equals pass as well as on the attached-target pass, because the
  server's dispatch route and the coalescing fast path run only
  `normalize_flag_equals` before the parser. An attached `-N3` is deliberately
  not an option: no pass splits it, so it is the first key operand and reaches
  the pane literally, and no attached-`N` handling was added to make the
  recognizer accept it. Ordinary leading options keep working, including a
  leading value that itself begins with `-` (`-t -x=foo`), a mixed cluster
  (`-lt foo`) and an explicit `--`. Every other command keeps upstream's
  generic region.

  What upstream does not have at all is the classification, the help routes and
  `-R`. The coalescing fast path normalizes option tokens the same way the
  dispatch route does and defers to the classifier, so a literal `--help` is
  never typed into a pane. `-M` stays recognized, is routed to
  `CtrlReq::SendMouse(client_id)` and is excluded from byte coalescing.
- Backspace is encoded as DEL (`0x7f`) in `encode_key_event`, consistently with
  the Windows console and SSH input paths. Upstream still emits `0x08` there,
  which its own `parse_modified_special_key` comment measures ConPTY reading as
  `VK_BACK` **with** `LEFT_CTRL_PRESSED`, so a physical Backspace would arrive
  as Ctrl+Backspace. Modified Backspace behavior remains upstream-owned.
- Mouse transport gives a pane identified as a native `INPUT_RECORD` reader
  precedence over an SSH/WSL descendant. A native TUI can run an SSH/WSL tool
  without handing over its console; its mouse reports stay on the ConPTY parser
  instead of being injected as Escape plus typed characters. Readers with VT
  input enabled retain the existing bridge route. This reuses the existing
  console-mode query and cache.
- `kill-server -h`, `kill-server --help`, and `help kill-server` are
  side-effect-free. Any other `kill-server` argument fails before startup
  cleanup or shutdown, so a misspelled help flag cannot destroy sessions. This
  currently also refuses upstream's `-a`/`--all` machine-wide sweep (#649),
  which upstream's general help still lists.
- The PowerShell Claude shim composes psmux teammate-mode behavior with an
  existing profile-defined `claude` **function** instead of replacing the
  owner's default flags: it captures that function's ScriptBlock once and chains
  to it, so the owner's defaults and teammate mode both apply. Upstream's #805
  resolved the same problem the other way, leaving a profile-defined function or
  alias completely alone, which also drops teammate mode for that user. The fork
  leaves **aliases and filters** intact: alias resolution and filter pipeline
  semantics are outside this function wrapper. It keeps upstream's #475 call-time
  `Get-Command` resolution when there is no user function.
- The integration runner `tests/run_all_integration.ps1` declares its `param`
  block first, so CI's full integration job actually binds `-Pattern` and runs
  the suite. Upstream puts a statement before `param(`, matches no tests and
  reports success.
- `psmux pick` attaches from a normal shell and opens `choose-session` on its
  first frame. The chooser uses the available width, preserves both ends of long
  session names (`middle_ellipsize`), ends each row with a compact
  `N windows YYYY.MM.DD HH:MM` tail plus `@` only when attached, and accents
  attached rows. `$` renames the
  highlighted session within its own `-L` namespace. The rename runs on a
  background worker, so Enter never waits on the network: the worker asks that
  server for its session name, exactly as the server reports it (a leading space
  included, through `fetch_session_info_exact`), and takes the namespace as
  whatever precedes `__<name>` in the registry name, so a namespace containing
  `__` and a client started without `-L` both work, and a server whose name does
  not match its registry entry is not renamed. Invalid names and collisions are
  reported from the worker. The server reserves the target name
  (`reserve_session_rename_target`) before it changes its registry, which is
  authoritative even when two servers rename at once, and `CtrlReq::RenameSession`
  carries a response channel: a rename the server acknowledges with a bare `OK`
  is reported as success while real transport failures keep their own message.
  The terminal caret sits inside the rename dialog at the cell width of the typed
  name, published through upstream's `prompt_cursor_pos`. A paste into the rename
  dialog is taken as in the other client-side overlays: an `Event::Paste` is
  appended to the name, and the character events that arrive inside the
  duplicate-paste window it opens are ignored (#290). While the dialog is open it
  is modal, so no mouse event reaches the pane, the popup drag logic or the host
  terminal's font zoom.
- The session option `tab-colour` applies a Windows Terminal tab color without
  rewriting the normal 0-255 text palette and resets it when cleared, when the
  client switches to a session without one, and when it detaches. Upstream added
  `key-table` beside it in the option catalog but not `tab-colour`.
- A Ctrl+V fallback never sends the clipboard to a pane hidden behind a psmux
  dialog. `route_active_paste!` extends upstream's `route_paste_to_overlay`
  routing to the picker's rename field, and `clipboard_read_back_allowed` skips
  the Release fallback's clipboard read-back while a chooser, key viewer, client
  confirmation, server popup, menu, confirm prompt, display-panes or customize is
  open. Clock mode keeps it, because a paste there only closes the clock.
  Upstream's paste delivery and duplicate accounting remain intact.
- Console-backed SSH VT input translates Ctrl+J, modified Enter, and Escape into
  Win32 input records when a Windows pane application such as Codex requests DEC
  private mode 9001 (`win32_input_special_key_seq`, reached from
  `write_pane_input`). Upstream's `named_key_record` cannot cover these: it only
  reads complete VT named-key sequences of three bytes or more starting with
  `ESC`, so a bare LF and an `ESC CR` pair fall through to it.
- Server liveness trusts the recorded `pid:creation_filetime` pair over the
  process image name (`pid_anchor_decision`). An image name may support "alive"
  but never "dead" on its own, so renaming or moving a running binary cannot make
  live servers look dead and lose their registry entries. Upstream's #650
  process-table lookup stays underneath; its test for an unsigned `.pid` entry
  naming another image expects an inconclusive verdict instead of dead.
  Upstream's stale-port-tax test for a recycled PID likewise records it with a
  signed entry whose creation time does not match the live process, which is
  still reaped without a network probe.
- The OSC 8 hyperlink overlay is clipped against the frame that was just drawn
  (`visible_hyperlink_runs` over `drawn_frame.buffer`), so a link under the
  session chooser, the `$` rename dialog or a popup is never repainted over it
  (#361). The clipped links ride upstream's atomic frame queue, before its single
  frame write.
- Replacing an installed binary never stops a server or kills a process.
  `scripts/build.ps1` drops upstream's `kill-server -a` and process kills and
  moves only a locked installed binary into a timestamped subdirectory under the
  same filename; `scripts/install-local.ps1` installs a built binary into
  `~/.local/bin`, moving every installed alias aside the same way before it
  updates each; and `scripts/psmux-binary-update.ps1` owns that move-aside rule,
  because renaming a running image makes live servers look dead. Moving aside,
  writing the new binaries and, for `install-local.ps1`, the `-V` check are one
  transaction: if any step fails, every binary already moved is put back before
  the error is reported, and preserved originals are pruned only after the check
  passes. Moved binaries are absent from their paths while the replacement runs.
  If putting one back fails too, both errors are reported and that original
  stays in its move-aside directory, so its path may be empty or still hold the
  new binary. `build.ps1` covers only the locked binaries it moves; `cargo
  install` replaces the others itself.
- Panes are not given `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`. Upstream sets it
  for every pane whose `claude-code-fix-tty` is on (`src/pane.rs`), which turns
  Claude Code agent teams on for every Claude started inside psmux; the fork
  leaves that choice to Claude's own settings. A value the pane already inherits
  still reaches the child untouched, as upstream's #805 intended. Upstream's
  `set -g claude-code-fix-tty off` also stops the default, but it takes the
  teammate-mode flag injection with it, which is not the same choice.
- Ctrl+Delete in the `prefix $` / `prefix ,` rename overlay clears the whole
  name through the prompt editor's existing C-u action, so the name the overlay
  opens on goes in one key. Upstream treats Ctrl+Delete there as a plain
  Delete, which does nothing at the end of the prefilled name. `prompt_edit`
  itself is unchanged.

## Upstream-owned and excluded behavior

Current upstream owns the queued `;` sub-command dispatch: its connection loop
takes the next one at the top of the loop only, with the same reasoning the fork
carried, so the fork's copy of that fix is dropped. Upstream's `pending_chain`,
its deferred run-shell tails (PR #740) and its error-ends-the-list rule are the
implementation now.

Current upstream owns the console VT reader's `escape-time`: `ESCAPE_TIME_MS` is
pushed to the client with every state frame and read afresh each pass through
`escape_wait_ms`, clamped to `ESC_COALESCE_MS`, so it follows the attached
session when the client switches. The fork's `EscapeTimeout` arc, its
`send_control_with_response_to` / `ControlRouting` addressing and its
`query_escape_time_ms` startup probe are dropped with it.

Current upstream owns the pane-title and window-index prompts, folded into the
one command prompt (`command_input` / `command_buf` / `command_back` and
`parse_command_prompt_args`), so the fork's separate `pane_renaming`,
`pane_title_buf`, `window_idx_input` and `window_idx_buf` state is gone. Upstream
also owns prompt caret publication (`prompt_cursor_pos`, #741 and #746) and the
repaint when an overlay closes with nothing else to draw (`overlay_closed`,
#767), which is what the fork's `prompt_overlay_cursor` and its "Enter on the
attached session repaints at once" patch were for.

Current upstream owns duplicate `rename-session` detection: `rename_blocker`
refuses at the CLI with tmux's `duplicate session: NAME` at exit 1 and at the
server before any registry write (#774, including the case-only spelling), and
`rekey_session_guard` moves the per-name mutex (#505). The fork keeps its
pre-mutation reservation beside it and adds one thing upstream's guard does not
do: the server answers the requester through the rename response channel, so a
picker worker reports the collision instead of waiting out its confirm timeout.

Upstream `c8fa938` and `ef0396c` own the independent terminal-paste guard fix,
so the former fork patch `f3110c7` is dropped. Upstream `53309cc` and `e25efe4`
own ordinary prompt paste routing; only the picker extension and broader dialog
exclusion above remain. Upstream `350855d` and `43702df` own dead-reader
connection teardown, Windows read-timeout retries and reconnect size reporting;
the previous fork backport `9596fdc` is dropped. Upstream #805 owns keeping a
value the pane already inherits, including `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`
when the user set it themselves.

Every targeted command request uses upstream's stable `TargetedSender`
(`e9b9bcc`), including `send-keys -R`, repeated keys and paste. The former
reset-specific temporary-focus patch is unnecessary and is not carried.
Copy-mode requests retain upstream's counted `SendKeysXRun` and error replies
(`d3da12a`); foreground shell command tails retain upstream's deferred handling
(`cbcf4eb`); replies retain upstream's buffered write and graceful close
(`853c934`); rendering retains the atomic frame path (`ae29378`). `send-keys -M`
keeps upstream's `CtrlReq::SendMouse(client_id)` (#799), which the fork's earlier
parser did not know.

Current upstream owns the base Win32-input-mode Escape repair, local Windows
Ctrl+Enter-to-LF delivery used by physical Ctrl+J, modified-Enter behavior,
Ctrl+Backspace handling, the SSH VT reader's own reading of bare LF as Ctrl+J
while CR remains Enter (#642) and its keeping a pasted CRLF or an LF inside a
paste burst as a line ending (#598), the M-Escape double byte (#766), and the
current mouse/scroll implementation including Ctrl+Wheel font zoom, apart from
the native record-reader routing precedence listed above. Keep those
implementations rather than duplicating older fork patches; the SSH bridge above
extends their delivery into a pane that requested Win32 input mode.

The control plane remains upstream's authenticated loopback TCP transport. The
old named-pipe fork and superseded Elysium mouse patch are intentionally not
carried. The fork's `expand_command_alias_and_normalize` is not carried either:
upstream owns alias expansion and where `normalize_flag_equals` runs.

## Updating upstream

Before any fork-specific code change, fetch current `marlocarlo/master` and
bring the fork's `master` forward first. Rebuild directly from that upstream tip
and reapply only the maintained differences above. Land the verified result on
the fork's `master`; routine maintenance of this fork does not use published
side branches or pull requests within the fork. A temporary detached worktree
is allowed when needed to protect active uncommitted files. If upstream now
provides a maintained difference, keep upstream's implementation and remove the
redundant fork patch. Do not merge accumulated fork history over the new base.

Run the focused, server-free Rust tests for the retained differences and
`cargo check` locally. Run the complete suite only in CI or another disposable
Windows environment, as required by `AGENTS.md`, before replacing the installed
executable.

The retained #805 and dangling-flag tests follow the fork's function composition,
opt-in agent teams and literal send-keys boundary. Plugin discovery fixtures set
XDG_CONFIG_HOME alongside HOME and USERPROFILE so host settings cannot redirect
their temporary plugin roots.
