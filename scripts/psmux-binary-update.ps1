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
        [string]$DirectoryPrefix
    )

    $moveAsideDirectory = $null
    $movedBinaries = [System.Collections.Generic.List[object]]::new()

    foreach ($binaryName in $BinaryNames) {
        $installedBinary = Join-Path $DestinationDirectory $binaryName
        if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) {
            continue
        }
        if (-not (Test-PsmuxBinaryLocked -LiteralPath $installedBinary)) {
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
