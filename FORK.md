# Elysium psmux fork

This fork follows upstream psmux and keeps only behavior the Elysium workflow
still requires and upstream does not yet provide.

## Maintained differences

- `send-keys` recognizes options only before `--` or the first key operand.
  Later tokens beginning with `-` are sent to the pane instead of being dropped
  or reinterpreted as targets. Its help routes are side-effect-free, and an
  unknown long option before the operand boundary fails with a literal-input
  hint. `send-keys -R` resets the target pane's parsed terminal state and screen.
- Backspace is encoded as DEL (`0x7f`) consistently with the Windows console and
  SSH input paths. Modified Backspace behavior remains upstream-owned.
- `kill-server -h`, `kill-server --help`, and `help kill-server` are
  side-effect-free. Any other `kill-server` argument fails before startup
  cleanup or shutdown, so a misspelled help flag cannot destroy sessions.
- The PowerShell Claude shim composes psmux teammate-mode behavior with an
  existing profile-defined `claude` function instead of replacing the owner's
  default flags.
- `psmux pick` attaches from a normal shell and opens `choose-session` on its
  first frame. The chooser uses the available width, preserves both ends of
  long session names, accents attached rows, and can rename its highlighted
  session with `$` and collision validation.
- The session option `tab-colour` applies a Windows Terminal tab color without
  rewriting the normal 0-255 text palette and resets it when cleared.
- Console-backed SSH VT input keeps bare LF as Ctrl+J/LF while CR remains Enter,
  uses the target server's `escape-time` for pending Escape sequences, and
  translates Ctrl+J, modified Enter, and Escape into Win32 input records when
  a Windows pane application such as Codex requests DEC private mode 9001.

## Upstream-owned and excluded behavior

Current upstream owns the base Win32-input-mode Escape repair, local Windows
Ctrl+Enter-to-LF delivery used by physical Ctrl+J, modified-Enter behavior,
Ctrl+Backspace handling, and the current mouse/scroll implementation. Keep those
implementations rather than duplicating older fork patches; the SSH bridge above
extends their delivery into a pane that requested Win32 input mode.

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
