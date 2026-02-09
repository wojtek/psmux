$ErrorActionPreference = 'Stop'

$toolsDir = "$(Split-Path -Parent $MyInvocation.MyCommand.Definition)"
$url64 = 'https://github.com/marlocarlo/psmux/releases/download/v0.2.7/psmux-0.2.7-windows-x64.zip'

$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  unzipLocation  = $toolsDir
  url64bit       = $url64
  checksum64     = '18D1BA8E34E2A72FC39D9713E9038C50F3B9AEB0BC9AFB56096E08C40CD92655'
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
