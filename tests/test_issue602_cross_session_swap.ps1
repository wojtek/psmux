# Issue #602 follow up: swap-window between two sessions must refuse, not
# exit 0 having done nothing.
#
# Found while re-running #602 on 3.4.0 (20aa1d7) and master (652d08b4):
#
#     move-window -s xa:1 -t xb:5   rc 1  "move-window: source is in session xa
#                                          (cross-session move-window is not supported ...)"
#     swap-window -s xa:0 -t xb:0   rc 0  ""      and neither session changed
#
# Each psmux session runs in its own server, so a window cannot cross between
# two of them; move-window (since #602) and swap-pane say so.  swap-window
# resolved `xa:0` inside xb's server as xb's own window 0, found source and
# destination equal, and returned success.  #602 asked for exactly this class
# of silent exit 0 to become an error the caller sees.  tmux itself swaps the
# two windows (cmd-swap-window.c:66-77); psmux cannot, so it refuses with the
# same wording move-window uses.
#
# Binary: $env:PSMUX_BIN if set, else psmux on PATH.  Own -L namespace;
# honours PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i602xs" + (Get-Random -Maximum 99999)
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

$env:PSMUX_RUN_NOPAUSE = '1'
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }
Write-Host "binary: $PSMUX  namespace: $NS" -ForegroundColor Cyan

function P { & $PSMUX -L $NS @args 2>&1 }
function Wins($S) { ((P list-windows -t $S -F '#{window_index}:#{window_name}') | Out-String) -replace '\s+', ' ' | ForEach-Object { $_.Trim() } }
function Run([string[]]$argv) {
    $out = & $PSMUX -L $NS @argv 2>&1
    @{ rc = $LASTEXITCODE; out = ((($out | Out-String) -replace '\s+', ' ').Trim()) }
}

P new-session -d -s xa -n one -x 120 -y 30 | Out-Null
P new-window -d -t xa:1 -n two | Out-Null
P new-session -d -s xb -n three -x 120 -y 30 | Out-Null
P new-window -d -t xb:1 -n four | Out-Null
Start-Sleep -Seconds 1
$a0 = Wins xa; $b0 = Wins xb
Write-Info "xa=[$a0] xb=[$b0]"

foreach ($argv in @(
        @('swap-window', '-s', 'xa:0', '-t', 'xb:0'),
        @('swap-window', '-s', 'xa:1', '-t', 'xb:1'),
        @('swap-window', '-s', 'xb:0', '-t', 'xa:1'))) {
    $r = Run $argv
    Write-Info "$($argv -join ' '): rc=$($r.rc) out='$($r.out)'"
    if ($r.rc -ne 0 -and $r.out -match 'cross-session swap-window is not supported') {
        Write-Pass "$($argv -join ' ') is refused with the cross-session error"
    } else {
        Write-Fail "$($argv -join ' '): rc=$($r.rc) out='$($r.out)' (expected a cross-session refusal at a non zero exit)"
    }
    if ((Wins xa) -eq $a0 -and (Wins xb) -eq $b0) { Write-Pass "  and neither session changed" }
    else { Write-Fail "  a session changed: xa=[$(Wins xa)] xb=[$(Wins xb)]" }
}

# Control: the same command inside one session still swaps (unchanged by this fix).
$r = Run @('swap-window', '-s', 'xb:0', '-t', 'xb:1')
$w = Wins xb
if ($r.rc -eq 0 -and $w -eq '0:four 1:three') { Write-Pass "same session swap-window still works [$w]" }
else { Write-Fail "same session swap-window: rc=$($r.rc) out='$($r.out)' xb=[$w]" }
# Control: a session qualified spec naming the session the server owns is not foreign.
$r = Run @('swap-window', '-t', 'xb:0')
if ($r.rc -eq 0) { Write-Pass "swap-window -t xb:0 (own session) still exits 0" }
else { Write-Fail "swap-window -t xb:0 (own session): rc=$($r.rc) out='$($r.out)'" }

P kill-server | Out-Null
Write-Host "`nPassed: $script:Pass  Failed: $script:Fail" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
