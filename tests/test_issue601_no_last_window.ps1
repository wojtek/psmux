# Issue #601 follow up: a session with no last window must not report one.
#
# Found while re-running the #601 reproduction on 3.4.0 (20aa1d7) and master
# (652d08b4).  The reporter's own setup, a fresh session with windows
# 0:main 1:lazygit 2:claude created with `new-window -d`, listed as
#
#     psmux  0:main*- 1:lazygit 2:claude      (window 0 is active AND last)
#     tmux   0:main* 1:lazygit 2:claude       (tmux 3.4, same commands)
#
# and `last-window` there exited 0 doing nothing, where tmux says
# `no last window` at exit 1 (cmd-select-window.c:118-120).
#
# Root cause: AppState::last_window_idx is a bare Vec position that starts at
# 0, so "no last window yet" and "the last window is position 0" were the same
# value, and every reader compared the window's position with it unchecked.
# In tmux the current window is never on the session's last-window stack
# (session.c:508-509 removes it before pushing the old current), so
# #{window_last_flag} (format.c:3120) can never be 1 on the active window.
#
# Routes covered: the CLI (list-windows formats, last-window, select-window -l)
# and the persistent control route (the one an attached client uses).
#
# Binary: $env:PSMUX_BIN if set, else psmux on PATH.  Runs under its own -L
# namespace; honours PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i601nolast" + (Get-Random -Maximum 99999)
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

$env:PSMUX_RUN_NOPAUSE = '1'
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }
Write-Host "binary: $PSMUX  namespace: $NS" -ForegroundColor Cyan

function P { & $PSMUX -L $NS @args 2>&1 }
function Wins($S) {
    ((P list-windows -t $S -F '#{window_index}:#{window_name}#{?window_active,*,}#{?window_last_flag,-,}') | Out-String) -replace '\s+', ' ' | ForEach-Object { $_.Trim() }
}
function Run([string[]]$argv) {
    $out = & $PSMUX -L $NS @argv 2>&1
    @{ rc = $LASTEXITCODE; out = ((($out | Out-String) -replace '\s+', ' ').Trim()) }
}

$S = "nl"
P new-session -d -s $S -n main -x 120 -y 30 | Out-Null
Start-Sleep -Seconds 1
P new-window -d -t "${S}:1" -n lazygit | Out-Null
P new-window -d -t "${S}:2" -n claude | Out-Null
P select-window -t "${S}:0" | Out-Null
Start-Sleep -Milliseconds 300

Write-Host "`n[1] fresh session: no window is the last window" -ForegroundColor Yellow
$w = Wins $S
Write-Info "list-windows: [$w]   (tmux 3.4: [0:main* 1:lazygit 2:claude])"
if ($w -eq '0:main* 1:lazygit 2:claude') { Write-Pass "no window carries window_last_flag" }
else { Write-Fail "window_last_flag set with no last window: [$w]" }
$raw = ((P list-windows -t $S -F '#{window_index}:#{window_raw_flags}|#{window_flags}') | Out-String) -replace '\s+', ' '
Write-Info "raw flags: [$($raw.Trim())]"
if ($raw -notmatch '-') { Write-Pass "#{window_raw_flags} / #{window_flags} carry no '-'" }
else { Write-Fail "a window flag says '-' with no last window: [$($raw.Trim())]" }

Write-Host "`n[2] last-window with no last window is tmux's error" -ForegroundColor Yellow
$r = Run @('last-window', '-t', $S)
Write-Info "last-window: rc=$($r.rc) out='$($r.out)'"
if ($r.rc -ne 0 -and $r.out -match 'no last window') { Write-Pass "last-window: 'no last window' at a non zero exit" }
else { Write-Fail "last-window with no last window: rc=$($r.rc) out='$($r.out)' (tmux: rc 1 'no last window')" }
$r = Run @('select-window', '-l', '-t', $S)
Write-Info "select-window -l: rc=$($r.rc) out='$($r.out)'"
if ($r.rc -ne 0 -and $r.out -match 'no last window') { Write-Pass "select-window -l: 'no last window' at a non zero exit" }
else { Write-Fail "select-window -l with no last window: rc=$($r.rc) out='$($r.out)'" }
if ((Wins $S) -eq '0:main* 1:lazygit 2:claude') { Write-Pass "the refused last-window changed nothing" }
else { Write-Fail "the refused last-window changed the windows: [$(Wins $S)]" }

Write-Host "`n[3] the reporter's swap keeps the active window off the last flag" -ForegroundColor Yellow
P swap-window -t "${S}:2" | Out-Null
Start-Sleep -Milliseconds 200
$w = Wins $S
Write-Info "after swap-window -t ${S}:2: [$w]   (tmux 3.4: [0:claude* 1:lazygit 2:main])"
if ($w -eq '0:claude* 1:lazygit 2:main') { Write-Pass "matches tmux" } else { Write-Fail "after swap: [$w]" }

Write-Host "`n[4] once a window has been left, last-window works both ways" -ForegroundColor Yellow
P select-window -t "${S}:1" | Out-Null
Start-Sleep -Milliseconds 200
$w = Wins $S
if ($w -eq '0:claude- 1:lazygit* 2:main') { Write-Pass "select-window :1 makes :0 the last window [$w]" } else { Write-Fail "after select-window :1: [$w]" }
$r = Run @('last-window', '-t', $S)
$w = Wins $S
if ($r.rc -eq 0 -and $w -eq '0:claude* 1:lazygit- 2:main') { Write-Pass "last-window toggles back [$w]" }
else { Write-Fail "last-window: rc=$($r.rc) out='$($r.out)' windows [$w]" }
P select-window -t "${S}:0" | Out-Null
if ((Wins $S) -eq '0:claude* 1:lazygit- 2:main') { Write-Pass "selecting the current window again keeps the last window" }
else { Write-Fail "select of the current window moved the last window: [$(Wins $S)]" }

Write-Host "`n[5] killing the last window leaves no last window" -ForegroundColor Yellow
P kill-window -t "${S}:1" | Out-Null
Start-Sleep -Milliseconds 300
$w = Wins $S
Write-Info "after kill-window :1: [$w]   (tmux 3.4: [0:claude* 2:main])"
if ($w -eq '0:claude* 2:main') { Write-Pass "no stale last flag after the last window died" } else { Write-Fail "after kill-window: [$w]" }
$r = Run @('last-window', '-t', $S)
if ($r.rc -ne 0 -and $r.out -match 'no last window') { Write-Pass "last-window after the last window died: 'no last window'" }
else { Write-Fail "last-window after the last window died: rc=$($r.rc) out='$($r.out)' windows [$(Wins $S)]" }

P kill-server | Out-Null
Write-Host "`nPassed: $script:Pass  Failed: $script:Fail" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
