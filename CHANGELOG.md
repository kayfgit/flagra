# Changelog

Each release's notes on GitHub are its section below. Add new entries under
"Unreleased"; at release time it becomes the version's heading.

## [0.3.0](https://github.com/kayfgit/flagra/compare/v0.2.0...v0.3.0) (2026-11-01)

### Features

* a ShadowPlay-style popup when a clip is saved, in a corner of the screen the game is on; it never takes focus and clicks go through it (`:popup on|off|test|<corner>`) ([c303790](https://github.com/kayfgit/flagra/commit/c303790))
* the bundled ffmpeg carries flagra's name and icon, so Task Manager lists it inside the flagra group instead of as a stray ffmpeg.exe ([814e817](https://github.com/kayfgit/flagra/commit/814e817))
* GPU encoding like ShadowPlay's: flagra uses NVENC (NVIDIA) or AMF (AMD) when your graphics card has one, for high quality at almost no CPU cost, and falls back to x264 if it doesn't or if it fails (`:encoder`) ([d845bfb](https://github.com/kayfgit/flagra/commit/d845bfb))
* quality profiles from potato to ultra, which set resolution, frame rate, bitrate and preset together (`:quality`) ([d845bfb](https://github.com/kayfgit/flagra/commit/d845bfb))
* `:size 20` sets the bitrate that keeps clips under 20 MB at your clip length, handy for Discord ([d845bfb](https://github.com/kayfgit/flagra/commit/d845bfb))
* the tray menu is dark ([d845bfb](https://github.com/kayfgit/flagra/commit/d845bfb))

### Changes

* Windows notifications on save are off by default for new installs, now that the popup does that job ([c303790](https://github.com/kayfgit/flagra/commit/c303790))

### Bug Fixes

* clips no longer freeze on one frame while a game keeps the GPU busy: capture gets GPU priority over the game, and if it still falls behind real time it restarts instead of staying frozen while audio piles up in memory ([ddded6a](https://github.com/kayfgit/flagra/commit/ddded6a))
* holding d in the clip list no longer sends clip after clip to the Recycle Bin; a held key now counts as one press, and tapping d can't delete several clips in a row ([ddded6a](https://github.com/kayfgit/flagra/commit/ddded6a))
* the hotkeys now work in games that switch off other programs' hotkeys while they're focused, by also reading the raw keyboard feed ([ddded6a](https://github.com/kayfgit/flagra/commit/ddded6a))
* a hotkey another program holds no longer goes dead until flagra restarts: it still works, and flagra keeps asking for it until it's free ([ddded6a](https://github.com/kayfgit/flagra/commit/ddded6a))

## [0.2.0](https://github.com/kayfgit/flagra/compare/v0.1.0...v0.2.0) (2026-10-28)

### Features

* microphone recording, mixed with desktop audio, with live volumes and level meters in settings (`:mic`, `:desktop`) ([766a132](https://github.com/kayfgit/flagra/commit/766a132))
* open instantly: `:keepready` keeps the window ready in the background so it opens in ~50 ms ([766a132](https://github.com/kayfgit/flagra/commit/766a132))
* ✕ and `:q!` quit flagra entirely; ─, Esc and `:q` close the window to the tray ([766a132](https://github.com/kayfgit/flagra/commit/766a132))
* the player seeks with h and l (Shift: 1 s) ([766a132](https://github.com/kayfgit/flagra/commit/766a132))
* flagra comes with its own 7 MB ffmpeg, so there's nothing else to install; the portable download is now a zip ([713f51a](https://github.com/kayfgit/flagra/commit/713f51a))
* starts with Windows, so it's recording from the moment you log in (`:autostart`) ([d972cae](https://github.com/kayfgit/flagra/commit/d972cae))
* frameless window with its own buttons, `:move`, `:resize` and `:f`, and Alt+F9 to open or close flagra from anywhere ([b9cf61b](https://github.com/kayfgit/flagra/commit/b9cf61b))
* flagra is MIT licensed ([713f51a](https://github.com/kayfgit/flagra/commit/713f51a))

### Bug Fixes

* any resolution other than native made capture fail to start; the window now shows ffmpeg's actual error when it stops ([d972cae](https://github.com/kayfgit/flagra/commit/d972cae))
* the help screen scrolls with j and k, and the scrollbars match the theme ([b9cf61b](https://github.com/kayfgit/flagra/commit/b9cf61b))

## [0.1.0](https://github.com/kayfgit/flagra/releases/tag/v0.1.0) (2026-10-28)

### Features

* instant replay: a hotkey saves the last N seconds of screen and desktop audio, from a buffer kept in RAM ([f25fed6](https://github.com/kayfgit/flagra/commit/f25fed6))
* a keyboard-driven window to play, copy, rename and delete clips and change settings ([f25fed6](https://github.com/kayfgit/flagra/commit/f25fed6))
* one-line installer and GitHub releases ([b08ea04](https://github.com/kayfgit/flagra/commit/b08ea04))
