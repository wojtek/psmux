# Elysium psmux fork

This branch follows upstream psmux and keeps only the Windows terminal behavior
that the Elysium workflow still requires.

## Maintained differences

- `send-keys` recognizes options only before `--` or the first key operand.
  Later tokens beginning with `-` are sent to the pane instead of being dropped.
- Backspace is encoded as DEL (`0x7f`) consistently with the Windows console and
  SSH input paths.

The control plane is upstream's authenticated loopback TCP transport. An older
fork used Windows named pipes to work around a blank SSH attach, but untouched
upstream 3.3.7 passed the same SSH-to-desktop-session attach and detach path, so
that transport fork is intentionally not carried forward.

When updating upstream, start a fresh branch from the selected upstream release,
reapply the maintained differences above, and run their focused tests plus the
complete Rust suite before replacing the installed executable.
