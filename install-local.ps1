# Builds this checkout and installs it for the current user, in the same place
# the one-line installer puts releases (settings kept), then starts it.
# For trying changes before they're released:
#   .\install-local.ps1
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

npx --yes @tauri-apps/cli@2 build
if ($LASTEXITCODE) { throw 'Build failed' }
$version = (Get-Content tauri.conf.json -Raw | ConvertFrom-Json).version
$setup = "target\release\bundle\nsis\flagra_${version}_x64-setup.exe"

# The recorder and an open window are both flagra.exe; ffmpeg exits with the recorder.
Get-Process flagra -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500
Start-Process $setup -ArgumentList '/S' -Wait

$exe = Join-Path $env:LOCALAPPDATA 'flagra\flagra.exe'
if (-not (Test-Path $exe)) { throw "Install finished but $exe is missing" }
Start-Process $exe
$build = git rev-parse --short HEAD
if (git status --porcelain) { $build += ' + uncommitted changes' }
Write-Host "Installed flagra $version ($build) and started it."
