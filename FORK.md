# Elysium psmux fork

This fork follows upstream psmux and keeps only behavior the Elysium workflow
still requires and upstream does not yet provide.

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
- A Ctrl+V never sends the clipboard to a pane hidden behind a psmux dialog.
  Upstream's Windows paste fallback reads the clipboard on the Ctrl+V Release
  when nothing was buffered for the pane and sends it with `send-paste`, which
  the server writes to the active pane whatever overlay is up, so with a dialog
  open the text went into the pane behind it. The fork skips that read-back
  while a client-side dialog (the command prompt, the rename dialogs, the
  window-index prompt, the choosers, the key viewer or a client confirm prompt)
  or a server popup, menu, confirm prompt, display-panes or customize is open.
  Clock mode keeps it, because a paste there only closes the clock.
- An independent terminal paste, such as SSH bracketed paste, does not leave
  upstream's Ctrl+V duplicate guard armed, fixing upstream defect `d828af2`
  that dropped every later paste from that client.
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
- The post-draw OSC 8 hyperlink overlay is clipped against the frame that was
  just drawn, so a link under the session chooser, the `$` rename dialog or a
  popup is never repainted over it (#361).
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

## Upstream-owned and excluded behavior

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
