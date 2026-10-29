# Writes a release's GitHub notes: its section of CHANGELOG.md, then how to
# install. Fails if CHANGELOG.md has no section for that version yet.
#   ./.github/release-notes.ps1 0.2.1 notes.md
param(
  [Parameter(Mandatory)] [string]$Version,
  [Parameter(Mandatory)] [string]$Out
)
$ErrorActionPreference = 'Stop'

$changelog = (Get-Content (Join-Path $PSScriptRoot '..\CHANGELOG.md') -Raw).Replace("`r`n", "`n")
$section = [regex]::Match($changelog, "(?ms)^(## \[$([regex]::Escape($Version))\]\([^)]*\) \([^)]*\))\n(.*?)(?=^## |\z)")
if (-not $section.Success) {
  throw "CHANGELOG.md has no section for $Version. Rename its 'Unreleased' heading to " +
    "'## [$Version](https://github.com/kayfgit/flagra/compare/v<previous>...v$Version) (yyyy-mm-dd)' first."
}

$install = @'
## Install

Paste into PowerShell. It installs for your user (no admin) and starts flagra:

```powershell
irm https://raw.githubusercontent.com/kayfgit/flagra/main/install.ps1 | iex
```

Or download **flagra-setup.exe** below, or **flagra-portable.zip** to run it from any folder without installing.
Both come with the ffmpeg flagra needs. Then press **Alt+F10** to save the last 20 seconds.
'@

$notes = $section.Groups[1].Value + "`n`n" + $section.Groups[2].Value.Trim() + "`n`n" + $install.Replace("`r`n", "`n")
[IO.File]::WriteAllText($Out, $notes)
