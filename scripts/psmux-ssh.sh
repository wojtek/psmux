#!/bin/bash
# psmux-ssh — SSH wrapper that enables mouse reporting for psmux on Windows.
#
# Problem: ConPTY on Windows 10 (and early Windows 11) consumes DECSET
# mouse-enable escape sequences from psmux's stdout, so your SSH client
# never learns that it should send mouse data.
#
# Solution: This script enables mouse reporting on YOUR LOCAL terminal
# before starting SSH.  Your terminal then sends SGR mouse events through
# the SSH connection, and psmux's VT parser on the remote side decodes them.
#
# Usage:
#   psmux-ssh user@windowshost
#   psmux-ssh -p 2222 user@host
#   psmux-ssh -- ssh -J jumphost user@windowshost   # custom ssh command
#
# If your first argument is '--', the rest is run as-is instead of
# prefixing 'ssh'.  This lets you use a custom SSH command.

set -e

if [ $# -eq 0 ]; then
    echo "Usage: psmux-ssh [ssh-options] [user@]host"
    echo "       psmux-ssh -- <custom-ssh-command> [args...]"
    echo
    echo "Wraps an SSH session with terminal mouse support for psmux."
    echo "Run this on your LOCAL machine (where you type ssh from)."
    exit 0
fi

# DECSET sequences for mouse reporting (SGR extended mode):
#   1000 = basic press/release   1002 = button-event (drag)
#   1003 = any-event (motion)    1006 = SGR extended coordinates
MOUSE_ON='\033[?1000h\033[?1002h\033[?1003h\033[?1006h'
MOUSE_OFF='\033[?1000l\033[?1002l\033[?1003l\033[?1006l'

# Always clean up on exit — restore the terminal even if SSH crashes.
cleanup() {
    printf "$MOUSE_OFF"
}
trap cleanup EXIT INT TERM HUP

# Enable mouse reporting on the local terminal.
printf "$MOUSE_ON"

# Run SSH (or custom command after --).
if [ "$1" = "--" ]; then
    shift
    exec "$@"
else
    exec ssh "$@"
fi
