param(
    [string]$PsmuxExe = $env:PSMUX_EXE,
    [string]$Pattern = "test_*.ps1",
    [switch]$FailFast
)
$ErrorActionPreference = "Continue"

function Resolve-PsmuxExe {
    param([string]$PreferredPath)

    if ($PreferredPath -and (Test-Path $PreferredPath)) {
        return (Resolve-Path $PreferredPath).Path
    }

    $candidates = @(
        "$PSScriptRoot\..\target\x86_64-pc-windows-msvc\release\psmux.exe",
        "$PSScriptRoot\..\target\release\psmux.exe",
        "$PSScriptRoot\..\target\debug\psmux.exe"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return (Resolve-Path $candidate).Path
        }
    }

    $cmd = Get-Command psmux -ErrorAction SilentlyContinue
    if ($cmd) {
        if ($cmd.Path) { return $cmd.Path }
        if ($cmd.Source) { return $cmd.Source }
    }

    return $null
}

$resolvedPsmuxExe = Resolve-PsmuxExe -PreferredPath $PsmuxExe
if (-not $resolvedPsmuxExe) {
    Write-Host "FATAL: could not locate psmux executable for integration suite" -ForegroundColor Red
    exit 1
}

$env:PSMUX_EXE = $resolvedPsmuxExe
$env:Path = "$(Split-Path -Parent $resolvedPsmuxExe);$env:Path"

$tests = Get-ChildItem -Path $PSScriptRoot -File -Filter $Pattern |
    Where-Object { $_.Name -like "test_*.ps1" } |
    Sort-Object -Property Name

if ($tests.Count -eq 0) {
    Write-Host "No tests matched pattern '$Pattern'." -ForegroundColor Yellow
    exit 0
}

$suitePass = 0
$suiteFail = 0
$failedTests = @()

Write-Host "`n=== Full Integration Test Suite ===" -ForegroundColor Cyan
Write-Host "psmux: $resolvedPsmuxExe" -ForegroundColor DarkGray
Write-Host "pattern: $Pattern" -ForegroundColor DarkGray
Write-Host "tests: $($tests.Count)" -ForegroundColor DarkGray

foreach ($test in $tests) {
    Write-Host "`n--- Running $($test.Name) ---" -ForegroundColor Yellow
    & pwsh -NoProfile -ExecutionPolicy Bypass -File $test.FullName
    $exitCode = $LASTEXITCODE

    if ($exitCode -eq 0) {
        Write-Host "[PASS] $($test.Name)" -ForegroundColor Green
        $suitePass++
    } else {
        Write-Host "[FAIL] $($test.Name) (exit=$exitCode)" -ForegroundColor Red
        $suiteFail++
        $failedTests += $test.Name

        if ($FailFast) {
            Write-Host "Fail-fast enabled; stopping after first failure." -ForegroundColor Red
            break
        }
    }
}

Write-Host "`n=== Full Integration Summary ===" -ForegroundColor Cyan
Write-Host "  Passed suites: $suitePass" -ForegroundColor Green
Write-Host "  Failed suites: $suiteFail" -ForegroundColor $(if ($suiteFail -gt 0) { "Red" } else { "Green" })

if ($failedTests.Count -gt 0) {
    Write-Host "  Failed tests:" -ForegroundColor Red
    foreach ($failed in $failedTests) {
        Write-Host "    - $failed" -ForegroundColor Red
    }
}

if ($suiteFail -gt 0) {
    exit 1
}

exit 0
