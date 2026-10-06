# Installs an already-built psmux binary without terminating running servers.

param(
    [string]$Source = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\psmux.exe"),
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin")
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "psmux-binary-update.ps1")

$sourcePath = [System.IO.Path]::GetFullPath($Source)
$destinationDirectory = [System.IO.Path]::GetFullPath($Destination)

if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
    throw "psmux source binary not found: $sourcePath"
}

if (-not (Test-Path -LiteralPath $destinationDirectory -PathType Container)) {
    New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null
}

$installedPsmux = Join-Path $destinationDirectory "psmux.exe"
Install-PsmuxLocalBinary -SourcePath $sourcePath -DestinationDirectory $destinationDirectory -Validate {
    Write-Host "[install-local] Verifying: $installedPsmux -V"
    & $installedPsmux -V
    if ($LASTEXITCODE -ne 0) {
        throw "Installed psmux version check failed with exit code $LASTEXITCODE"
    }
}

Write-Host "Servers already running keep executing the old binary until they exit; this update reaches new processes only."
