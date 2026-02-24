<#
.SYNOPSIS
    SSH wrapper that enables mouse reporting for psmux on Windows.

.DESCRIPTION
    ConPTY on Windows 10 (and early Windows 11) consumes DECSET mouse-enable
    escape sequences from psmux's stdout, so your SSH client never learns
    that it should send mouse data.

    This script enables mouse reporting on YOUR LOCAL terminal before starting
    SSH.  Your terminal then sends SGR mouse events through the SSH connection,
    and psmux's VT parser on the remote side decodes them.

    Run this on your LOCAL machine (the one you type ssh from).

.PARAMETER SshArgs
    Arguments to pass to ssh (e.g. user@host, -p 2222, etc.)

.EXAMPLE
    .\psmux-ssh.ps1 user@windowshost
    .\psmux-ssh.ps1 -p 2222 user@myserver
#>

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$SshArgs
)

if (-not $SshArgs -or $SshArgs.Count -eq 0) {
    Write-Host "Usage: psmux-ssh.ps1 [ssh-options] [user@]host"
    Write-Host ""
    Write-Host "Wraps an SSH session with terminal mouse support for psmux."
    Write-Host "Run this on your LOCAL machine (where you type ssh from)."
    exit 0
}

# DECSET sequences for mouse reporting (SGR extended mode):
#   1000 = basic press/release   1002 = button-event (drag)
#   1003 = any-event (motion)    1006 = SGR extended coordinates
$MouseOn  = "`e[?1000h`e[?1002h`e[?1003h`e[?1006h"
$MouseOff = "`e[?1000l`e[?1002l`e[?1003l`e[?1006l"

try {
    # Enable mouse reporting on the local terminal.
    [Console]::Write($MouseOn)

    # Run SSH with all arguments passed through.
    & ssh @SshArgs
}
finally {
    # Always disable mouse reporting on exit to restore the terminal.
    [Console]::Write($MouseOff)
}
