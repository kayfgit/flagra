# Third-party software

flagra itself is MIT licensed (see [LICENSE](LICENSE)). It ships with, or is built from, the following.

## FFmpeg and x264 (bundled, GPL v2 or later)

The installer and the portable zip include `ffmpeg\ffmpeg.exe`: [FFmpeg](https://ffmpeg.org) with
[x264](https://www.videolan.org/developers/x264.html), built by [ffmpeg/build.sh](ffmpeg/build.sh) with only the
parts flagra uses. flagra runs it as a separate program; it is not linked into flagra.

Both are licensed under the GNU General Public License, version 2 or later. A copy of the license is installed
next to it (`ffmpeg\COPYING.GPLv2.txt`). The exact source code each bundled build was made from, and the
script that built it, are attached to its release on this repository, named `ffmpeg-<version>-<build>`
(the current one is named in [ffmpeg/version.env](ffmpeg/version.env)).

## Rust crates and the window

flagra.exe is built from Rust crates under permissive licenses (MIT, Apache-2.0 and similar), including
[Tauri](https://tauri.app) and [windows-rs](https://github.com/microsoft/windows-rs). The window runs on
Microsoft Edge WebView2, which is part of Windows and not shipped with flagra.
