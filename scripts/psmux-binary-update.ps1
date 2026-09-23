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
        try {
            Restore-PsmuxMovedBinaries -MovedBinaries $movedBinaries.ToArray()
        } catch {
            # Report why the replacement failed, not only why the restore did:
            # the restore error used to replace it.
            throw [System.InvalidOperationException]::new(
                "psmux binary replacement failed: $($replaceError.Exception.Message); $($_.Exception.Message)",
                $replaceError.Exception)
        }
        throw $replaceError
    }

    return $movedBinaries.ToArray()
}

function Remove-PsmuxUnlockedPreservedBinaries {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$DestinationDirectory,

        [Parameter(Mandatory)]
        [string]$LogPrefix
    )

    # Preserved binaries that no running server holds any more are pruned; a
    # locked one waits for a later install.
    $preservedDirectories = @(
        Get-ChildItem -LiteralPath $DestinationDirectory -Directory |
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
                Write-Host "$LogPrefix Kept locked preserved binary: $($preservedFile.FullName)" -ForegroundColor Yellow
                continue
            }

            Remove-Item -LiteralPath $preservedFile.FullName -Force
            $prunedFileCount++
            Write-Host "$LogPrefix Pruned unlocked preserved binary: $($preservedFile.FullName)" -ForegroundColor Green
        }

        $nestedDirectories = @(Get-ChildItem -LiteralPath $preservedDirectory.FullName -Directory -Recurse | Sort-Object -Property FullName -Descending)
        foreach ($nestedDirectory in $nestedDirectories) {
            if (@(Get-ChildItem -LiteralPath $nestedDirectory.FullName -Force).Count -eq 0) {
                Remove-Item -LiteralPath $nestedDirectory.FullName -Force
                Write-Host "$LogPrefix Removed empty preserved directory: $($nestedDirectory.FullName)" -ForegroundColor Green
            }
        }

        if ((Test-Path -LiteralPath $preservedDirectory.FullName -PathType Container) -and
            @(Get-ChildItem -LiteralPath $preservedDirectory.FullName -Force).Count -eq 0) {
            Remove-Item -LiteralPath $preservedDirectory.FullName -Force
            Write-Host "$LogPrefix Removed empty preserved directory: $($preservedDirectory.FullName)" -ForegroundColor Green
        }
    }

    Write-Host "$LogPrefix Preserved-binary cleanup: pruned=$prunedFileCount kept=$keptFileCount"
}

function Install-PsmuxLocalBinary {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$SourcePath,

        [Parameter(Mandatory)]
        [string]$DestinationDirectory,

        # Proves the installed binaries work; any failure must throw.
        [Parameter(Mandatory)]
        [scriptblock]$Validate
    )

    $installNames = [System.Collections.Generic.List[string]]::new()
    $installNames.Add("psmux.exe")
    foreach ($aliasName in @("pmux.exe", "tmux.exe")) {
        if (Test-Path -LiteralPath (Join-Path $DestinationDirectory $aliasName) -PathType Leaf) {
            $installNames.Add($aliasName)
        }
    }

    # Every installed alias moves aside first, locked or not, so a failed copy can
    # put each original back. The version check belongs to the same transaction:
    # a binary that copies but does not run is rolled back while the originals
    # still exist. Checked after the cleanup, it found them already pruned.
    $null = Invoke-PsmuxBinaryReplacement `
        -DestinationDirectory $DestinationDirectory `
        -BinaryNames $installNames.ToArray() `
        -DirectoryPrefix "psmux-install-move-aside" `
        -LogPrefix "[install-local]" `
        -IncludeUnlocked `
        -Replace {
            foreach ($installName in $installNames) {
                $installedBinary = Join-Path $DestinationDirectory $installName
                Copy-Item -LiteralPath $SourcePath -Destination $installedBinary -Force
                Write-Host "[install-local] Installed: $installedBinary" -ForegroundColor Green
            }
            & $Validate
        }

    # Only a checked install gives up the originals.
    Remove-PsmuxUnlockedPreservedBinaries -DestinationDirectory $DestinationDirectory -LogPrefix "[install-local]"
}
