# Elysium psmux fork

This branch follows upstream psmux and keeps only behavior the Elysium workflow
still requires and upstream does not yet provide.

## Maintained differences

- `send-keys` recognizes options only before `--` or the first key operand.
  Later tokens beginning with `-` are sent to the pane instead of being dropped
  or reinterpreted as targets.
- Backspace is encoded as DEL (`0x7f`) consistently with the Windows console and
  SSH input paths.
- `kill-server -h`, `kill-server --help`, and `help kill-server` are
  side-effect-free. Any other `kill-server` argument fails before startup
  cleanup or shutdown, so a misspelled help flag cannot destroy sessions.
- `send-keys --help` remains side-effect-free, and misspelled long options remain
  available as literal input after the explicit operand boundary.

The control plane remains upstream's authenticated loopback TCP transport. The
old named-pipe fork is intentionally not carried. Current upstream also owns the
scroll fix: ordinary and inline-TUI panes scroll psmux history instead of
receiving synthetic arrow keys, so the superseded Elysium mouse patch is not
reapplied over upstream's newer mouse-protocol logic.

## Updating upstream

Start a fresh branch from the selected upstream revision and reapply only the
maintained differences above. Run their focused, server-free Rust tests and
`cargo check` locally. Run the complete suite only in CI or another disposable
Windows environment, as required by `AGENTS.md`, before replacing the installed
executable.
