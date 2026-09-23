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

        # Receives each move the moment it succeeds, so a move that fails later
        # in the loop cannot strand the binaries already moved: the caller
        # still holds their records and can put them back.
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[object]]$MovedBinaries,

        # Move every installed binary, not only locked ones, so a failed
        # replacement can put each original back.
        [switch]$IncludeUnlocked
    )

    $moveAsideDirectory = $null

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
            New-Item -ItemType Directory -Path $moveAsideDirectory -ErrorAction Stop | Out-Null
        }

        # PSMUX_SERVER_IMAGE_NAMES defines server identity by image name. Change directories, never filenames.
        $movedBinary = Join-Path $moveAsideDirectory $binaryName
        Move-Item -LiteralPath $installedBinary -Destination $movedBinary -ErrorAction Stop
        $MovedBinaries.Add([pscustomobject]@{
            Source = $installedBinary
            Destination = $movedBinary
        })
    }
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
                Remove-Item -LiteralPath $movedBinary.Source -Force -ErrorAction Stop
            }
            Move-Item -LiteralPath $movedBinary.Destination -Destination $movedBinary.Source -ErrorAction Stop
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

    # The moves and the replacement are one transaction: a failure in either,
    # including a move that fails after others succeeded, puts every binary
    # already moved back. Leaving them in the move-aside directory made psmux
    # vanish from PATH while its servers kept running.
    # Never stops or kills a process: running binaries only change directory.
    $movedBinaries = [System.Collections.Generic.List[object]]::new()
    try {
        Move-PsmuxLockedBinariesAside `
            -DestinationDirectory $DestinationDirectory `
            -BinaryNames $BinaryNames `
            -DirectoryPrefix $DirectoryPrefix `
            -MovedBinaries $movedBinaries `
            -IncludeUnlocked:$IncludeUnlocked
        foreach ($movedBinary in $movedBinaries) {
            Write-Host "$LogPrefix Moved binary aside without renaming: $($movedBinary.Source) -> $($movedBinary.Destination)" -ForegroundColor Yellow
        }

        & $Replace
    } catch {
        $replaceError = $_
        Write-Host "$LogPrefix Replacement failed; restoring the original binaries" -ForegroundColor Yellow
        Restore-PsmuxMovedBinaries -MovedBinaries $movedBinaries.ToArray()
        throw $replaceError
    }

    return $movedBinaries.ToArray()
}
