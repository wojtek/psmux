function Test-PsmuxBinaryLocked {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$LiteralPath
    )

    $lockProbe = $null
    try {
        $lockProbe = [System.IO.File]::Open(
            $LiteralPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
        return $false
    } catch [System.IO.IOException] {
        return $true
    } finally {
        if ($null -ne $lockProbe) {
            $lockProbe.Dispose()
        }
    }
}

function Move-PsmuxLockedBinariesAside {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$DestinationDirectory,

        [Parameter(Mandatory)]
        [string[]]$BinaryNames,

        [Parameter(Mandatory)]
        [string]$DirectoryPrefix,

        # Move every installed binary, not only locked ones, so a failed
        # replacement can put each original back.
        [switch]$IncludeUnlocked
    )

    $moveAsideDirectory = $null
    $movedBinaries = [System.Collections.Generic.List[object]]::new()

    foreach ($binaryName in $BinaryNames) {
        $installedBinary = Join-Path $DestinationDirectory $binaryName
        if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) {
            continue
        }
        if (-not $IncludeUnlocked -and -not (Test-PsmuxBinaryLocked -LiteralPath $installedBinary)) {
            continue
        }

        if ($null -eq $moveAsideDirectory) {
            $moveAsideDirectory = Join-Path $DestinationDirectory "$DirectoryPrefix-$(Get-Date -Format 'yyyyMMdd-HHmmssfff')-$PID"
            New-Item -ItemType Directory -Path $moveAsideDirectory | Out-Null
        }

        # PSMUX_SERVER_IMAGE_NAMES defines server identity by image name. Change directories, never filenames.
        $movedBinary = Join-Path $moveAsideDirectory $binaryName
        Move-Item -LiteralPath $installedBinary -Destination $movedBinary
        $movedBinaries.Add([pscustomobject]@{
            Source = $installedBinary
            Destination = $movedBinary
        })
    }

    return $movedBinaries.ToArray()
}

function Restore-PsmuxMovedBinaries {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [object[]]$MovedBinaries
    )

    # Put every moved original back where it was. A file now at the original
    # path is a partial or unwanted replacement and makes way for it.
    $failures = [System.Collections.Generic.List[string]]::new()
    foreach ($movedBinary in $MovedBinaries) {
        try {
            if (Test-Path -LiteralPath $movedBinary.Source) {
                Remove-Item -LiteralPath $movedBinary.Source -Force
            }
            Move-Item -LiteralPath $movedBinary.Destination -Destination $movedBinary.Source
        } catch {
            $failures.Add("$($movedBinary.Destination) -> $($movedBinary.Source): $_")
        }
    }
    if ($failures.Count -gt 0) {
        throw "could not restore moved psmux binaries: $($failures -join '; ')"
    }
}

function Invoke-PsmuxBinaryReplacement {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$DestinationDirectory,

        [Parameter(Mandatory)]
        [string[]]$BinaryNames,

        [Parameter(Mandatory)]
        [string]$DirectoryPrefix,

        [Parameter(Mandatory)]
        [string]$LogPrefix,

        # Writes the new binaries into $DestinationDirectory; any failure must throw.
        [Parameter(Mandatory)]
        [scriptblock]$Replace,

        [switch]$IncludeUnlocked
    )

    # Never stops or kills a process: running binaries only change directory.
    $movedBinaries = @(Move-PsmuxLockedBinariesAside `
        -DestinationDirectory $DestinationDirectory `
        -BinaryNames $BinaryNames `
        -DirectoryPrefix $DirectoryPrefix `
        -IncludeUnlocked:$IncludeUnlocked)
    foreach ($movedBinary in $movedBinaries) {
        Write-Host "$LogPrefix Moved binary aside without renaming: $($movedBinary.Source) -> $($movedBinary.Destination)" -ForegroundColor Yellow
    }

    # A failure after the move used to leave the originals in the move-aside
    # directory, so psmux vanished from PATH while its servers kept running.
    try {
        & $Replace
    } catch {
        $replaceError = $_
        Write-Host "$LogPrefix Replacement failed; restoring the original binaries" -ForegroundColor Yellow
        Restore-PsmuxMovedBinaries -MovedBinaries $movedBinaries
        throw $replaceError
    }

    return $movedBinaries
}
