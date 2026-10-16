# scripts/build.ps1 — Build psmux + NSIS installer
# Usage:
#   .\scripts\build.ps1              # full build: cargo install + NSIS setup
#   .\scripts\build.ps1 -SkipSetup   # cargo install only (no NSIS)
#   .\scripts\build.ps1 -SetupOnly   # NSIS only (assumes binaries exist)

param(
    [switch]$SkipSetup,
    [switch]$SetupOnly
)

$ErrorActionPreference = "Stop"
$repoDir = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "psmux-binary-update.ps1")

Push-Location $repoDir
try {
    # ── Cargo install ─────────────────────────────────────────────────
    if (-not $SetupOnly) {
        if ($env:CARGO_INSTALL_ROOT) {
            $cargoInstallRoot = $env:CARGO_INSTALL_ROOT
        } elseif ($env:CARGO_HOME) {
            $cargoInstallRoot = $env:CARGO_HOME
        } else {
            $cargoInstallRoot = Join-Path $HOME ".cargo"
        }

        $cargoBinDir = Join-Path $cargoInstallRoot "bin"
        # A failed build puts every moved binary back, so psmux stays on PATH.
        $null = Invoke-PsmuxBinaryReplacement `
            -DestinationDirectory $cargoBinDir `
            -BinaryNames @("psmux.exe", "pmux.exe", "tmux.exe") `
            -DirectoryPrefix "psmux-build-move-aside" `
            -LogPrefix "[build]" `
            -Replace {
                Write-Host "[build] Running cargo install --path ." -ForegroundColor Cyan
                cargo install --path .
                if ($LASTEXITCODE -ne 0) {
                    throw "cargo install failed (exit $LASTEXITCODE)"
                }
            }
        Write-Host "[build] cargo install succeeded" -ForegroundColor Green
    }

    # ── NSIS installer ────────────────────────────────────────────────
    if (-not $SkipSetup) {
        # Find makensis
        $makensis = $null
        foreach ($candidate in @(
            "makensis",
            "$env:USERPROFILE\scoop\apps\nsis\current\bin\makensis.exe",
            "C:\Program Files (x86)\NSIS\makensis.exe",
            "C:\Program Files\NSIS\makensis.exe"
        )) {
            if (Get-Command $candidate -ErrorAction SilentlyContinue) {
                $makensis = (Get-Command $candidate).Source
                break
            }
            if (Test-Path $candidate) {
                $makensis = $candidate
                break
            }
        }

        if (-not $makensis) {
            Write-Host "[build] WARN: makensis not found — skipping installer build" -ForegroundColor Yellow
            Write-Host "[build] Install NSIS: scoop install nsis  (from extras bucket)" -ForegroundColor Yellow
        } else {
            # Read version from Cargo.toml
            $cargoToml = Get-Content "$repoDir\Cargo.toml" -Raw
            if ($cargoToml -match '(?m)^version\s*=\s*"([^"]+)"') {
                $ver = $Matches[1]
            } else {
                Write-Error "Could not parse version from Cargo.toml"
                exit 1
            }

            # Find source binaries
            $srcDir = "$repoDir\target\release"
            if (-not (Test-Path "$srcDir\psmux.exe")) {
                Write-Error "Release binaries not found at $srcDir — build first"
                exit 1
            }

            New-Item -ItemType Directory -Path "$repoDir\target\installer" -Force | Out-Null

            Write-Host "[build] Building NSIS installer (v$ver, x64)..." -ForegroundColor Cyan
            & $makensis /NOCD /DVERSION=$ver /DARCH=x64 "/DSOURCE_DIR=$srcDir" "/DREPO_DIR=$repoDir" "$repoDir\installer\psmux.nsi"
            if ($LASTEXITCODE -ne 0) {
                Write-Error "NSIS compilation failed (exit $LASTEXITCODE)"
                exit 1
            }

            $installer = "$repoDir\target\installer\psmux-v${ver}-x64-setup.exe"
            if (Test-Path $installer) {
                $sizeMB = [math]::Round((Get-Item $installer).Length / 1MB, 2)
                Write-Host "[build] Installer created: $installer ($sizeMB MB)" -ForegroundColor Green
            }
        }
    }

    # ── Portable test zip (temp dir) ─────────────────────────────────
    # Creates a zip of the release binaries in TEMP for test_install_speed.ps1.
    # Nothing is written inside the repo.
    $srcDir = "$repoDir\target\release"
    if (Test-Path "$srcDir\psmux.exe") {
        $zipDir = Join-Path $env:TEMP "psmux-test-artifacts"
        New-Item -ItemType Directory -Path $zipDir -Force | Out-Null
        $zipPath = Join-Path $zipDir "psmux-local-test.zip"
        Remove-Item $zipPath -Force -ErrorAction SilentlyContinue

        $stagingDir = Join-Path $env:TEMP "psmux-zip-staging"
        if (Test-Path $stagingDir) { Remove-Item $stagingDir -Recurse -Force }
        New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
        foreach ($bin in @("psmux.exe", "pmux.exe", "tmux.exe")) {
            $binSrc = Join-Path $srcDir $bin
            if (Test-Path $binSrc) { Copy-Item $binSrc $stagingDir }
        }
        Compress-Archive -Path "$stagingDir\*" -DestinationPath $zipPath -Force
        Remove-Item $stagingDir -Recurse -Force

        $sizeMB = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
        Write-Host "[build] Test zip created: $zipPath ($sizeMB MB)" -ForegroundColor Green
    }

    Write-Host "[build] Done!" -ForegroundColor Green
} finally {
    Pop-Location
}
