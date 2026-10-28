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

# flagra records through ffmpeg, and needs a build with ddagrab (Windows screen
# capture, ffmpeg 6.1+). Use one that's already installed, otherwise fetch one.
function Test-Ffmpeg($path) {
  try { (& $path -hide_banner -filters 2>$null | Out-String) -match '\bddagrab\b' } catch { $false }
}
$ffmpeg = Join-Path $dir 'ffmpeg\ffmpeg.exe'
$onPath = Get-Command ffmpeg -ErrorAction SilentlyContinue
if ((Test-Path $ffmpeg) -and (Test-Ffmpeg $ffmpeg)) {
  Write-Host 'ffmpeg is already set up.'
} elseif ($onPath -and (Test-Ffmpeg $onPath.Source)) {
  Write-Host "Using the ffmpeg on your PATH ($($onPath.Source))."
} else {
  # The "essentials" build from gyan.dev, the Windows build ffmpeg.org links to.
  $ffUrl = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip'
  $zip = Join-Path $env:TEMP 'flagra-ffmpeg.zip'
  $sum = Join-Path $env:TEMP 'flagra-ffmpeg.zip.sha256'
  Write-Host 'Downloading ffmpeg (about 110 MB, only this once)...'
  Invoke-WebRequest $ffUrl -OutFile $zip -UseBasicParsing
  Invoke-WebRequest "$ffUrl.sha256" -OutFile $sum -UseBasicParsing
  $expected = ((Get-Content $sum -Raw).Trim() -split '\s+')[0]
  $actual = (Get-FileHash $zip -Algorithm SHA256).Hash
  Remove-Item $sum
  if ($actual -ne $expected) {
    Remove-Item $zip
    throw 'The ffmpeg download is damaged (checksum mismatch). Run the installer again.'
  }

  # Only ffmpeg.exe is needed, not the whole package.
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  New-Item -ItemType Directory -Force (Split-Path $ffmpeg) | Out-Null
  $archive = [IO.Compression.ZipFile]::OpenRead($zip)
  try {
    $entry = $archive.Entries | Where-Object FullName -like '*/bin/ffmpeg.exe' | Select-Object -First 1
    if (-not $entry) { throw 'ffmpeg.exe is missing from the download.' }
    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $ffmpeg, $true)
  } finally {
    $archive.Dispose()
    Remove-Item $zip
  }
  if (-not (Test-Ffmpeg $ffmpeg)) { throw 'The downloaded ffmpeg cannot capture the screen (no ddagrab).' }
}

Start-Process $exe

# Show the hotkey actually in use (it may have been changed in an earlier install).
$hotkey = 'Alt+F10'
$ini = Join-Path $dir 'flagra.ini'
if (Test-Path $ini) {
  $line = Select-String -Path $ini -Pattern '^\s*hotkey\s*=\s*(\S.*)$' | Select-Object -First 1
  if ($line) { $hotkey = $line.Matches[0].Groups[1].Value.Trim() }
}
Write-Host "flagra is recording in the tray. Press $hotkey to save a clip; double-click the tray icon to open flagra."
