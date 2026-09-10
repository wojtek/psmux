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

$binaryNames = [System.Collections.Generic.List[string]]::new()
$binaryNames.Add("psmux.exe")
foreach ($aliasName in @("pmux.exe", "tmux.exe")) {
    if (Test-Path -LiteralPath (Join-Path $destinationDirectory $aliasName) -PathType Leaf) {
        $binaryNames.Add($aliasName)
    }
}

$movedBinaries = @(Move-PsmuxLockedBinariesAside `
    -DestinationDirectory $destinationDirectory `
    -BinaryNames $binaryNames.ToArray() `
    -DirectoryPrefix "psmux-install-move-aside")

foreach ($movedBinary in $movedBinaries) {
    Write-Host "[install-local] Moved locked binary aside without renaming: $($movedBinary.Source) -> $($movedBinary.Destination)" -ForegroundColor Yellow
}

foreach ($binaryName in $binaryNames) {
    $installedBinary = Join-Path $destinationDirectory $binaryName
    Copy-Item -LiteralPath $sourcePath -Destination $installedBinary -Force
    Write-Host "[install-local] Installed: $installedBinary" -ForegroundColor Green
}

$preservedDirectories = @(
    Get-ChildItem -LiteralPath $destinationDirectory -Directory |
        Where-Object { $_.Name -like "psmux-install-move-aside-*" -or $_.Name -like "psmux-build-move-aside-*" } |
        Sort-Object -Property FullName -Unique
)
$prunedFileCount = 0
$keptFileCount = 0

foreach ($preservedDirectory in $preservedDirectories) {
    $preservedFiles = @(Get-ChildItem -LiteralPath $preservedDirectory.FullName -File -Recurse)
    foreach ($preservedFile in $preservedFiles) {
        if (Test-PsmuxBinaryLocked -LiteralPath $preservedFile.FullName) {
            $keptFileCount++
            Write-Host "[install-local] Kept locked preserved binary: $($preservedFile.FullName)" -ForegroundColor Yellow
            continue
        }

        Remove-Item -LiteralPath $preservedFile.FullName -Force
        $prunedFileCount++
        Write-Host "[install-local] Pruned unlocked preserved binary: $($preservedFile.FullName)" -ForegroundColor Green
    }

    $nestedDirectories = @(Get-ChildItem -LiteralPath $preservedDirectory.FullName -Directory -Recurse | Sort-Object -Property FullName -Descending)
    foreach ($nestedDirectory in $nestedDirectories) {
        if (@(Get-ChildItem -LiteralPath $nestedDirectory.FullName -Force).Count -eq 0) {
            Remove-Item -LiteralPath $nestedDirectory.FullName -Force
            Write-Host "[install-local] Removed empty preserved directory: $($nestedDirectory.FullName)" -ForegroundColor Green
        }
    }

    if ((Test-Path -LiteralPath $preservedDirectory.FullName -PathType Container) -and
        @(Get-ChildItem -LiteralPath $preservedDirectory.FullName -Force).Count -eq 0) {
        Remove-Item -LiteralPath $preservedDirectory.FullName -Force
        Write-Host "[install-local] Removed empty preserved directory: $($preservedDirectory.FullName)" -ForegroundColor Green
    }
}

Write-Host "[install-local] Preserved-binary cleanup: pruned=$prunedFileCount kept=$keptFileCount"

$installedPsmux = Join-Path $destinationDirectory "psmux.exe"
Write-Host "[install-local] Verifying: $installedPsmux -V"
& $installedPsmux -V
if ($LASTEXITCODE -ne 0) {
    throw "Installed psmux version check failed with exit code $LASTEXITCODE"
}

Write-Host "Servers already running keep executing the old binary until they exit; this update reaches new processes only."
