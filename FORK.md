# Elysium psmux fork

This fork follows upstream psmux and keeps only behavior the Elysium workflow
still requires and upstream does not yet provide.

The current base is `marlocarlo/master` at
`83a69a9d6a427d78bc4b9adf7a0269a3b6ec04cd` (2026-10-06). The maintained layer
is reapplied on that tip, without the previous fork history.

## Maintained differences

- `send-keys` recognizes options only before `--` or the first key operand.
  Later tokens beginning with `-` are sent to the pane instead of being dropped
  or reinterpreted as targets. Its help routes are side-effect-free, and an
  unknown long option before the operand boundary fails with a literal-input
  hint. `send-keys -R` resets the target pane's parsed terminal state and screen.
- Each queued `;` sub-command is taken from the connection's queue once.
  Upstream's connection loop takes the next one at both the bottom and the top
  of the loop, so with two or more queued it drops every other one; the fork
  takes each only at the top. That is the whole difference: a command that
  ends a one-shot connection (such as `session-info`) still ends the rest of
  the chain, and an `if-shell` command run while sub-commands are queued can
  still be replaced by the next one, as upstream.
- Backspace is encoded as DEL (`0x7f`) consistently with the Windows console and
  SSH input paths. Modified Backspace behavior remains upstream-owned.
- `kill-server -h`, `kill-server --help`, and `help kill-server` are
  side-effect-free. Any other `kill-server` argument fails before startup
  cleanup or shutdown, so a misspelled help flag cannot destroy sessions. This
  currently also refuses upstream's `-a`/`--all` machine-wide sweep (#649),
  which upstream's general help still lists.
- The PowerShell Claude shim composes psmux teammate-mode behavior with an
  existing profile-defined `claude` function instead of replacing the owner's
  default flags.
- The integration runner `tests/run_all_integration.ps1` declares its `param`
  block first, so CI's full integration job actually binds `-Pattern` and runs
  the suite. Upstream puts a statement before `param(`, matches no tests and
  reports success.
- `psmux pick` attaches from a normal shell and opens `choose-session` on its
  first frame. The chooser uses the available width, preserves both ends of
  long session names, ends each row with a compact `N windows YYYY.MM.DD HH:MM`
  tail plus `@` only when a client is attached so the name keeps the freed
  columns, and accents attached rows. `$` renames the highlighted session within
  its own `-L` namespace. The rename runs on a background worker, so Enter never
  waits on the network: the worker asks that server for its session name,
  exactly as the server reports it (a leading space included), and takes the
  namespace as whatever precedes `__<name>` in the registry name, so a namespace
  containing `__` and a client started without `-L` both work, and a server
  whose name does not match its registry entry is not renamed. Invalid names and
  collisions are reported from the worker, and the server reserves the target name before it changes
  its registry, a rename the server acknowledges with a bare `OK` is reported as
  success while real transport failures keep their own message, and the
  terminal caret sits inside the rename dialog at the cell width of the typed
  name. A paste into the rename dialog is taken as in the other client-side
  overlays: an `Event::Paste` is appended to the name, and the character events
  that arrive inside the duplicate-paste window it opens are ignored (#290).
  Enter on the session the client is already attached to closes the
  chooser and repaints at once; upstream leaves it painted until the next key,
  which then also reaches the pane.
- The session option `tab-colour` applies a Windows Terminal tab color without
  rewriting the normal 0-255 text palette and resets it when cleared, when the
  client switches to a session without one, and when it detaches.
- A Ctrl+V fallback never sends the clipboard to a pane hidden behind a psmux
  dialog. Upstream now routes ordinary command, rename, pane-title and index
  prompts into their own text buffers. The fork extends that routing to the
  picker's rename field and skips clipboard read-back while a chooser, key
  viewer, client confirmation, server popup, menu, confirm prompt, display-panes
  or customize is open. Clock mode keeps it, because a paste there only closes
  the clock. Upstream's paste delivery and duplicate accounting remain intact.
- Console-backed SSH VT input uses the attached session's `escape-time` for
  pending Escape sequences, following it when the client switches sessions,
  and translates Ctrl+J, modified Enter, and Escape into Win32
  input records when a Windows pane application such as Codex requests DEC
  private mode 9001.
- Server liveness trusts the recorded `pid:creation_filetime` pair over the
  process image name. An image name may support "alive" but never "dead" on its
  own, so renaming or moving a running binary cannot make live servers look dead
  and lose their registry entries. Upstream's #650 process-table lookup stays
  underneath; its test for an unsigned `.pid` entry naming another image expects
  an inconclusive verdict instead of dead. Upstream's stale-port-tax test for a
  recycled PID likewise records it with a signed entry whose creation time does
  not match the live process, which is still reaped without a network probe.
- The OSC 8 hyperlink overlay is clipped against the frame that was
  just drawn, so a link under the session chooser, the `$` rename dialog or a
  popup is never repainted over it (#361). The clipped links and the dialog's
  caret use upstream's atomic frame queue, before its single frame write.
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
- Panes are not given `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`. Upstream sets
  it for every pane in `set_tmux_env` (`src/pane.rs`), which turns Claude Code
  agent teams on for every Claude started inside psmux; the fork leaves that
  choice to Claude's own settings.

## Upstream-owned and excluded behavior

Upstream `c8fa938` and `ef0396c` own the independent terminal-paste guard fix,
so the former fork patch `f3110c7` is dropped. Upstream `53309cc` and `e25efe4`
own ordinary prompt paste routing; only the picker extension and broader
dialog exclusion above remain. Upstream `350855d` and `43702df` own dead-reader
connection teardown, Windows read-timeout retries and reconnect size reporting;
the previous fork backport `9596fdc` is dropped.

Every targeted command request uses upstream's stable `TargetedSender`
(`e9b9bcc`), including `send-keys -R`, repeated keys and paste. The former
reset-specific temporary-focus patch is unnecessary and is not carried.
Copy-mode requests retain upstream's counted `SendKeysXRun` and error replies
(`d3da12a`); foreground shell command tails retain upstream's deferred handling
(`cbcf4eb`); replies retain upstream's buffered write and graceful close
(`853c934`); rendering retains the atomic frame path (`ae29378`).

Current upstream owns the base Win32-input-mode Escape repair, local Windows
Ctrl+Enter-to-LF delivery used by physical Ctrl+J, modified-Enter behavior,
Ctrl+Backspace handling, the SSH VT reader's own reading of bare LF as Ctrl+J
while CR remains Enter (#642) and its keeping a pasted CRLF or an LF inside a
paste burst as a line ending (#598), and the current mouse/scroll
implementation. Keep
those implementations rather than duplicating older fork patches; the SSH bridge
above extends their delivery into a pane that requested Win32 input mode.

The control plane remains upstream's authenticated loopback TCP transport. The
old named-pipe fork and superseded Elysium mouse patch are intentionally not
carried.

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
