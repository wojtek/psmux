$ErrorActionPreference = 'Stop'

$toolsDir = "$(Split-Path -Parent $MyInvocation.MyCommand.Definition)"
$url64 = 'https://github.com/marlocarlo/psmux/releases/download/v0.3.5/psmux-v0.3.5-windows-x64.zip'

$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  unzipLocation  = $toolsDir
  url64bit       = $url64
  checksum64     = 'PLACEHOLDER_WILL_BE_COMPUTED_BY_CI'
  checksumType64 = 'sha256'
}

Install-ChocolateyZipPackage @packageArgs

# Create shims for psmux, pmux, and tmux
$psmuxPath = Join-Path $toolsDir "psmux.exe"
$pmuxPath = Join-Path $toolsDir "pmux.exe"
$tmuxPath = Join-Path $toolsDir "tmux.exe"

Install-BinFile -Name "psmux" -Path $psmuxPath
Install-BinFile -Name "pmux" -Path $pmuxPath
Install-BinFile -Name "tmux" -Path $tmuxPath
