# Downloads the ffmpeg build flagra bundles (the release ffmpeg\version.env names)
# into ffmpeg\dist, where the installer build picks it up. Does nothing if it's
# already there. Used by install-local.ps1 and the release workflow.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$v = Get-Content (Join-Path $PSScriptRoot 'version.env') | Where-Object { $_ -match '^\w+=' } | Out-String | ConvertFrom-StringData
$tag = "ffmpeg-$($v.FFMPEG_VERSION)-$($v.BUILD)"
$dist = Join-Path $PSScriptRoot 'dist'
$stamp = Join-Path $dist 'tag.txt'

if ((Test-Path $stamp) -and (Get-Content $stamp -Raw).Trim() -eq $tag -and (Test-Path (Join-Path $dist 'ffmpeg.exe'))) {
  Write-Host "ffmpeg: $tag is already in ffmpeg\dist"
  return
}
New-Item -ItemType Directory -Force $dist | Out-Null
foreach ($file in 'ffmpeg.exe', 'COPYING.GPLv2', 'README.txt') {
  Invoke-WebRequest "https://github.com/kayfgit/flagra/releases/download/$tag/$file" -OutFile (Join-Path $dist $file) -UseBasicParsing
}
Set-Content $stamp $tag
Write-Host "ffmpeg: fetched $tag"
