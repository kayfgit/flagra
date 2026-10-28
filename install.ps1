# Installs (or updates) flagra for the current user and starts it:
#   irm https://raw.githubusercontent.com/kayfgit/flagra/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # the progress bar makes downloads many times slower on PowerShell 5

$url = 'https://github.com/kayfgit/flagra/releases/latest/download/flagra-setup.exe'
$setup = Join-Path $env:TEMP 'flagra-setup.exe'
$dir = Join-Path $env:LOCALAPPDATA 'flagra'
$exe = Join-Path $dir 'flagra.exe'

Write-Host 'Downloading flagra...'
Invoke-WebRequest $url -OutFile $setup -UseBasicParsing

# An update can't replace the exe while it's running (the recorder and an open
# window are both flagra.exe; ffmpeg exits with the recorder).
Get-Process flagra -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

Write-Host 'Installing...'
Start-Process $setup -ArgumentList '/S' -Wait
Remove-Item $setup -ErrorAction SilentlyContinue
if (-not (Test-Path $exe)) { throw "Install finished but $exe is missing" }

# The installer comes with the ffmpeg flagra records through (in flagra\ffmpeg).
Start-Process $exe

# Show the hotkey actually in use (it may have been changed in an earlier install).
$hotkey = 'Alt+F10'
$ini = Join-Path $dir 'flagra.ini'
if (Test-Path $ini) {
  $line = Select-String -Path $ini -Pattern '^\s*hotkey\s*=\s*(\S.*)$' | Select-Object -First 1
  if ($line) { $hotkey = $line.Matches[0].Groups[1].Value.Trim() }
}
Write-Host "flagra is recording in the tray. Press $hotkey to save a clip; double-click the tray icon to open flagra."
