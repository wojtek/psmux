# A failed binary replacement puts every original installed binary back.
#
# scripts/build.ps1 and scripts/install-local.ps1 move locked installed
# binaries aside (never renaming them, never stopping a process) before
# `cargo install` or the copy writes the new ones. Neither restored them when
# that step failed: a failed build left psmux.exe missing from PATH while
# servers kept running the moved image. Both now replace through
# Invoke-PsmuxBinaryReplacement, which restores every moved binary on failure.
#
# Pure test: it works only inside a fresh sandbox directory with dummy files.
# It never runs psmux, never touches ~/.local/bin, PATH or cargo's bin
# directory, and never starts or stops a process.

param(
    [string]$SandboxRoot = [System.IO.Path]::GetTempPath()
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "..\scripts\psmux-binary-update.ps1")

$script:failures = 0
function Assert-That([bool]$Condition, [string]$Message) {
    if ($Condition) {
        Write-Host "ok   $Message"
    } else {
        Write-Host "FAIL $Message" -ForegroundColor Red
        $script:failures++
    }
}

function New-Sandbox {
    $dir = Join-Path $SandboxRoot ("psmux-binary-rollback-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $dir | Out-Null
    return $dir
}

function Set-Dummy([string]$Path, [string]$Content) {
    [System.IO.File]::WriteAllText($Path, $Content)
}

function Get-Dummy([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return "<missing>" }
    return [System.IO.File]::ReadAllText($Path)
}

# A running server's image: exclusive access is refused, which is what
# Test-PsmuxBinaryLocked detects, while the file may still change directory.
function Open-LikeRunningImage([string]$Path) {
    return [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete))
}

$sandboxes = [System.Collections.Generic.List[string]]::new()
try {
    # 1. A failed build restores a locked binary that was moved aside.
    $bin = New-Sandbox; $sandboxes.Add($bin)
    Set-Dummy (Join-Path $bin "psmux.exe") "old-psmux"
    Set-Dummy (Join-Path $bin "tmux.exe") "old-tmux"
    $running = Open-LikeRunningImage (Join-Path $bin "psmux.exe")
    try {
        $threw = $false
        try {
            $null = Invoke-PsmuxBinaryReplacement `
                -DestinationDirectory $bin `
                -BinaryNames @("psmux.exe", "pmux.exe", "tmux.exe") `
                -DirectoryPrefix "psmux-build-move-aside" `
                -LogPrefix "[test]" `
                -Replace { throw "injected cargo install failure" }
        } catch {
            $threw = $true
        }
        Assert-That $threw "the build failure still propagates"
    } finally {
        $running.Dispose()
    }
    Assert-That ((Get-Dummy (Join-Path $bin "psmux.exe")) -eq "old-psmux") "a failed build puts the locked psmux.exe back"
    Assert-That ((Get-Dummy (Join-Path $bin "tmux.exe")) -eq "old-tmux") "an unlocked alias the build never moved is untouched"

    # 2. A copy that fails part-way restores every original, locked or not.
    $bin = New-Sandbox; $sandboxes.Add($bin)
    Set-Dummy (Join-Path $bin "psmux.exe") "old-psmux"
    Set-Dummy (Join-Path $bin "pmux.exe") "old-pmux"
    $running = Open-LikeRunningImage (Join-Path $bin "psmux.exe")
    try {
        $threw = $false
        try {
            $null = Invoke-PsmuxBinaryReplacement `
                -DestinationDirectory $bin `
                -BinaryNames @("psmux.exe", "pmux.exe") `
                -DirectoryPrefix "psmux-install-move-aside" `
                -LogPrefix "[test]" `
                -IncludeUnlocked `
                -Replace {
                    Set-Dummy (Join-Path $bin "psmux.exe") "new-psmux"
                    Set-Dummy (Join-Path $bin "pmux.exe") "partial"
                    throw "injected copy failure"
                }
        } catch {
            $threw = $true
        }
        Assert-That $threw "the copy failure still propagates"
    } finally {
        $running.Dispose()
    }
    Assert-That ((Get-Dummy (Join-Path $bin "psmux.exe")) -eq "old-psmux") "a failed install puts the locked psmux.exe back"
    Assert-That ((Get-Dummy (Join-Path $bin "pmux.exe")) -eq "old-pmux") "a failed install puts the unlocked pmux.exe back"

    # 3. A move that fails part-way puts back the binaries already moved.
    $bin = New-Sandbox; $sandboxes.Add($bin)
    Set-Dummy (Join-Path $bin "psmux.exe") "old-psmux"
    Set-Dummy (Join-Path $bin "pmux.exe") "old-pmux"
    $running = Open-LikeRunningImage (Join-Path $bin "psmux.exe")
    # Open without delete sharing, so this binary cannot change directory.
    $unmovable = [System.IO.File]::Open(
        (Join-Path $bin "pmux.exe"),
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read)
    $script:replaced = $false
    try {
        $threw = $false
        try {
            $null = Invoke-PsmuxBinaryReplacement `
                -DestinationDirectory $bin `
                -BinaryNames @("psmux.exe", "pmux.exe") `
                -DirectoryPrefix "psmux-install-move-aside" `
                -LogPrefix "[test]" `
                -Replace { $script:replaced = $true }
        } catch {
            $threw = $true
        }
        Assert-That $threw "the failed move still propagates"
    } finally {
        $unmovable.Dispose()
        $running.Dispose()
    }
    Assert-That (-not $script:replaced) "nothing is replaced after a failed move"
    Assert-That ((Get-Dummy (Join-Path $bin "psmux.exe")) -eq "old-psmux") "a failed second move puts the first binary back"
    Assert-That ((Get-Dummy (Join-Path $bin "pmux.exe")) -eq "old-pmux") "the binary that could not move stays in place"

    # 4. A successful replacement installs the new binaries and keeps the moved originals aside.
    $bin = New-Sandbox; $sandboxes.Add($bin)
    Set-Dummy (Join-Path $bin "psmux.exe") "old-psmux"
    $moved = @(Invoke-PsmuxBinaryReplacement `
        -DestinationDirectory $bin `
        -BinaryNames @("psmux.exe") `
        -DirectoryPrefix "psmux-install-move-aside" `
        -LogPrefix "[test]" `
        -IncludeUnlocked `
        -Replace { Set-Dummy (Join-Path $bin "psmux.exe") "new-psmux" })
    Assert-That ((Get-Dummy (Join-Path $bin "psmux.exe")) -eq "new-psmux") "a successful install leaves the new binary in place"
    Assert-That ($moved.Count -eq 1 -and (Get-Dummy $moved[0].Destination) -eq "old-psmux") "the original waits aside for the caller's cleanup"
} finally {
    foreach ($sandbox in $sandboxes) {
        Remove-Item -LiteralPath $sandbox -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($script:failures -gt 0) {
    Write-Host "$($script:failures) check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host "all checks passed"
exit 0
