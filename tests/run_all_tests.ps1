# psmux Master Test Runner
# Runs all test suites and provides a summary

$ErrorActionPreference = "Continue"
$TestsDir = $PSScriptRoot
$Results = @{}

Write-Host ""
Write-Host "=" * 70
Write-Host "                    PSMUX TEST SUITE RUNNER"
Write-Host "=" * 70
Write-Host ""

# First, ensure the project is built
Write-Host "[BUILD] Building psmux in debug mode..."
Push-Location "$TestsDir\.."
cargo build 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0) {
    Write-Host "[BUILD] Build successful" -ForegroundColor Green
} else {
    Write-Host "[BUILD] Build failed!" -ForegroundColor Red
    exit 1
}
Pop-Location
Write-Host ""

# Run basic tests
Write-Host "Running: Basic Command Tests (test_all.ps1)"
Write-Host "-" * 70
& "$TestsDir\test_all.ps1"
$Results["Basic Commands"] = $LASTEXITCODE
Write-Host ""

# Run session tests (more comprehensive)
Write-Host "Running: Session Tests (test_session.ps1)"
Write-Host "-" * 70
& "$TestsDir\test_session.ps1"
$Results["Session Tests"] = $LASTEXITCODE
Write-Host ""

# Run config tests
Write-Host "Running: Config Tests (test_config.ps1)"
Write-Host "-" * 70
& "$TestsDir\test_config.ps1"
$Results["Config Tests"] = $LASTEXITCODE
Write-Host ""

# Run advanced feature tests
Write-Host "Running: Advanced Feature Tests (test_advanced.ps1)"
Write-Host "-" * 70
& "$TestsDir\test_advanced.ps1"
$Results["Advanced Features"] = $LASTEXITCODE
Write-Host ""

# Summary
Write-Host ""
Write-Host "=" * 70
Write-Host "                    FINAL TEST SUMMARY"
Write-Host "=" * 70

$AllPassed = $true
foreach ($suite in $Results.Keys) {
    if ($Results[$suite] -eq 0) {
        Write-Host "[PASS] $suite" -ForegroundColor Green
    } else {
        Write-Host "[FAIL] $suite" -ForegroundColor Red
        $AllPassed = $false
    }
}

Write-Host ""
if ($AllPassed) {
    Write-Host "All test suites passed!" -ForegroundColor Green
    exit 0
} else {
    Write-Host "Some test suites failed!" -ForegroundColor Red
    exit 1
}
