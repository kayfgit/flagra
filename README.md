# flagra

ShadowPlay-style instant replay for PCs that can't run ShadowPlay (no NVENC: GT 1030, GT 710, iGPU-less CPUs, ...).
Press a hotkey, get the last N seconds as an `.mp4`.

- Lives in the tray; default hotkey **Alt+F10**, saves the last **20 s** to `Videos\flagra`.
- Records the screen and desktop audio, kept in sync (measured within ±10 ms).
- The replay buffer lives in **RAM** (~25 MB at the default settings), so it doesn't write to disk constantly.
  The disk is only touched when you save a clip.
- Saving a clip takes no extra CPU: the buffer is already encoded, so it's just copied into an `.mp4`.

## Install

Paste this into PowerShell. It installs flagra for your user (no admin rights, no clicks) and starts it:

```powershell
irm https://raw.githubusercontent.com/kayfgit/flagra/main/install.ps1 | iex
```

Then press **Alt+F10** whenever something worth keeping happens, and **Alt+F9** to open flagra. It starts
with Windows, so it's always recording (`:autostart` turns that off). Run the same command again to update;
your settings are kept.

You can also download [flagra-setup.exe](https://github.com/kayfgit/flagra/releases/latest/download/flagra-setup.exe),
or [flagra-portable.zip](https://github.com/kayfgit/flagra/releases/latest/download/flagra-portable.zip) to run it
from any folder without installing. Everything comes with the ffmpeg flagra needs, so there's nothing else to
install. flagra needs Windows 10 or 11.

To uninstall, use **Settings › Apps**. Your clips in `Videos\flagra` are never touched.

## Build

```bash
cargo build --release
```

The exe ends up at `target\release\flagra.exe` (~3.2 MB, the GUI is embedded). On first run it writes
`flagra.ini` next to itself. The GUI uses WebView2, which ships with Windows 11 (and almost every
Windows 10). A plain build like this has no ffmpeg of its own, so it uses one on your `PATH`.

The installer bundles flagra's own ffmpeg:

```bash
./ffmpeg/fetch.ps1
npx @tauri-apps/cli@2 build --config tauri.bundle.conf.json
```

To try changes before releasing them, `.\install-local.ps1` does both, installs the result where the
one-liner does (settings kept), and restarts flagra. Pushing a `v*` tag makes GitHub Actions build the
installer and the portable zip and publish a release.

### The bundled ffmpeg

flagra runs ffmpeg as a separate program. Rather than depend on whatever ffmpeg a PC has (or a ~100 MB
general-purpose build), it ships a small one with only what it uses, pinned to the version it's tested with.
[ffmpeg/build.sh](ffmpeg/build.sh) cross-compiles it (with x264) for Windows; the
[ffmpeg workflow](.github/workflows/ffmpeg.yml) builds it, checks it on Windows and publishes it, with its exact
sources, as a release named `ffmpeg-<version>-<build>`. [ffmpeg/version.env](ffmpeg/version.env) says which one
flagra bundles: bump `BUILD` there to publish a new one. `ffmpeg_path` in the settings still overrides it.
## Use

| | |
|---|---|
| Save a clip | hotkey (default `Alt+F10`), `c` in the window, or tray → *Save clip now* |
| Open the window | `Alt+F9` from anywhere (press it again to close), double-click the tray icon, or start `flagra.exe` again |
| Something wrong? | `:log` in the window, or tray → *Open log* (ffmpeg's own errors go to `ffmpeg.log`) |

## The window

The recorder runs headless in the tray (~30 MB). The window is a **separate process**
(`flagra.exe --ui`) that only exists while it's open: closing it (Esc) ends the process and its
WebView2, and recording carries on untouched. Open, it costs about 90 MB (WebView2 trimmed the same
way as tradu, GPU off: that measured 50 MB lighter and no more CPU, even while playing clips).

Like tradu it has no Windows title bar: minimize and close sit at the top right, dragging the top or
bottom bar moves it, and the edges resize it. `:move` and `:resize` do the same from the keyboard
(`h j k l`, Shift for fine steps, Ctrl for big ones, `↵` when done) and flagra opens there from then
on; `:move reset` / `:resize reset` undo that. `:f` toggles fullscreen.

Everything is on the keyboard, in the same style as tradu:

| Key | Clips | Settings |
|---|---|---|
| `↑ ↓` / `j k` | select | select |
| `↵` | play here (again: pause) | change it (booleans toggle) |
| `y` | copy the file: paste into Discord, a chat, a folder | |
| `r` / `d d` | rename / move to the Recycle Bin | |
| `f` / `o` | show in Explorer / open in your video player | |
| `c` | save a clip now | |
| `tab` | settings | clips |
| `:` | command line | command line |
| `?` / `esc` | help (`j k` scroll it) / close the window | help / close the window |

While a clip plays: `space` pause, `← →` 5 s (Shift: 1 s), `, .` one frame, `m` mute, `esc` close.

Settings change through commands, with a preview of what they'll do as you type:
`:len 30`, `:fps 30`, `:res 720p`, `:bitrate 8m`, `:preset veryfast`, `:audio off` / `:audio 192`,
`:hotkey` / `:openkey` (then press the combo), `:monitor 1`, `:mouse`, `:priority below`, `:dir D:\Clips`,
`:beep`, `:notify`, `:ffmpeg PATH`. Plus `:clip`, `:folder`, `:log`, `:ini`, `:start` / `:stop`
(the background recorder), `:move`, `:resize`, `:f`, `:min` and `:help`.

Only settings that change the encoding (fps, resolution, bitrate, preset, audio, monitor, cursor,
priority, ffmpeg) restart capture, which starts the replay buffer over. The rest (hotkey, clip length,
folder, beep, notification) apply instantly without losing what's buffered.

You can also control it from the command line, e.g. to bind a mouse button or macro key:

```bash
flagra.exe --clip
```

`flagra.exe --quit` stops the running instance.


## Settings (`flagra.ini`)

The window edits this file for you (keeping its comments). Editing it by hand works too: tray →
*Edit flagra.ini*, then tray → *Reload settings*.

| Setting | Default | Notes |
|---|---|---|
| `hotkey` | `Alt+F10` | Saves a clip. Modifiers `Ctrl` `Alt` `Shift` `Win` + one key (`A`-`Z`, `0`-`9`, `F1`-`F24`, `Home`, `PageUp`, `Numpad0`, ...) |
| `open_hotkey` | `Alt+F9` | Opens the window from anywhere, and closes it if it's in front. Empty = none. |
| `clip_seconds` | `20` | Length of the saved clip (1-600). RAM use grows with this × bitrate. |
| `fps` | `60` | `30` roughly halves the encoding cost. |
| `resolution` | `native` | Or e.g. `1280x720`. Scaling happens on the GPU, and a lower resolution saves a lot of CPU. |
| `video_bitrate_kbps` | `10000` | Quality and file size. Barely affects CPU. |
| `preset` | `ultrafast` | x264 preset. Each step slower = better quality per bit, noticeably more CPU. |
| `audio` | `true` | Desktop audio (whatever plays on the default output device; follows device switches). |
| `audio_bitrate_kbps` | `160` | |
| `monitor` | `0` | Which display to record. |
| `draw_mouse` | `true` | |
| `priority` | `below_normal` | Encoder CPU priority. `below_normal` = when the CPU is maxed, the clip stutters instead of your game. |
| `output_dir` | *(Videos\flagra)* | |
| `ffmpeg_path` | *(auto)* | |
| `beep` / `notify` | `true` | Beep (audible in fullscreen games) and a tray notification on save. |
| `start_with_windows` | `true` | Starts flagra when you log in (a per-user Run entry; Task Manager's Startup apps switch still applies). |

### What it costs

Measured on an i5-9400F (6 cores) + GT 1030, 1080p60, defaults:

| | CPU (whole machine) | RAM |
|---|---|---|
| ffmpeg, idle desktop | ~10% | ~115 MB (about 70 MB of it is the GPU capture itself) |
| ffmpeg, full-screen fast motion | ~15% | ~115 MB |
| flagra.exe (recorder) | ~0.1% | ~30 MB (includes the replay buffer) |
| the window, only while open | ~0% idle | ~90 MB (≈165 MB while playing a clip) |

If that's too much while gaming, try these in order: `fps = 30`, then `resolution = 1280x720`.

## How it works

```
GPU:  Desktop Duplication (ddagrab) → pick freshest frame per slot → scale + RGB→NV12
CPU:  x264 ultrafast ─┐
WASAPI loopback ─AAC─┴→ MPEG-TS → flagra.exe RAM ring (split at keyframes, 1/s)
hotkey → copy the last N s of the ring → remux to .mp4 (no re-encode)
```

- **No hardware encoder** on these cards, so encoding is x264 on the CPU at the fastest preset.
  Everything else (capture, scaling, colour conversion) stays on the GPU, so the CPU only
  receives a ready-to-encode NV12 frame.
- **Smooth capture on high-refresh monitors.** ddagrab paced at 60 fps waits only half a frame for new content,
  so on a 120 Hz screen it often timed out and repeated a frame (~9% of frames measured). flagra polls at
  500 Hz and lets ffmpeg's `fps` filter pick the newest frame for each 1/60 s slot (0 repeats measured).
- **Audio sync.** ffmpeg can't capture desktop audio itself, so flagra.exe captures it with WASAPI loopback
  and pipes raw PCM in. Each audio packet is placed by its own hardware timestamp on a timeline that starts at
  a known instant, and silence is filled in when nothing is playing (loopback sends no data then). The video's
  first frame is pinned to the same instant through its wall-clock time.
- **The 0.15 s audio shift.** ffmpeg pauses any input that runs more than 100 ms ahead of the others. Audio
  naturally trails by about that much (buffering plus AAC framing), which kept pausing the video and dropping
  frames. So audio is stamped 0.15 s later while recording, and the shift is undone when a clip is saved.
  The side effect is that the last ~0.15 s of a clip has no audio.
- If ffmpeg dies (crash, display mode change in a game), it's restarted automatically. It runs inside a
  job object, so it never outlives flagra.

## Known limits

- Clips start on a keyframe (one per second), so a clip can be up to ~1 s longer than `clip_seconds`.
- Exclusive-fullscreen games that block Desktop Duplication show up black. Borderless/windowed always works,
  and most modern "fullscreen" modes do too.
- Records one monitor; no microphone yet.

## License

flagra is MIT licensed ([LICENSE](LICENSE)). The ffmpeg it ships is GPL v2 or later; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for that and where its source is.
