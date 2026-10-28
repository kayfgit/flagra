use std::fs;
use std::path::{Path, PathBuf};

use crate::hotkey;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
}

impl Priority {
    /// Win32 process creation flag for this priority class.
    pub fn creation_flag(self) -> u32 {
        match self {
            Priority::Idle => 0x0000_0040,
            Priority::BelowNormal => 0x0000_4000,
            Priority::Normal => 0x0000_0020,
            Priority::AboveNormal => 0x0000_8000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub hotkey: String,
    /// Opens (or closes) the window from anywhere. None = no hotkey.
    pub open_hotkey: Option<String>,
    pub clip_seconds: u32,
    pub fps: u32,
    /// None = capture at the monitor's native resolution.
    pub resolution: Option<(u32, u32)>,
    pub video_bitrate_kbps: u32,
    pub preset: String,
    pub audio: bool,
    pub audio_bitrate_kbps: u32,
    pub monitor: u32,
    pub draw_mouse: bool,
    pub priority: Priority,
    /// None = <Videos>\flagra.
    pub output_dir: Option<PathBuf>,
    /// None = ffmpeg.exe next to flagra.exe, then PATH.
    pub ffmpeg_path: Option<PathBuf>,
    pub beep: bool,
    pub notify: bool,
    pub start_with_windows: bool,
    /// Where :move left the window, in logical px from its monitor's corner. None = centered.
    pub window_position: Option<[f64; 2]>,
    /// What :resize left the window at, in logical px. None = the default size.
    pub window_size: Option<[f64; 2]>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: "Alt+F10".into(),
            open_hotkey: Some("Alt+F9".into()),
            clip_seconds: 20,
            fps: 60,
            resolution: None,
            video_bitrate_kbps: 10_000,
            preset: "ultrafast".into(),
            audio: true,
            audio_bitrate_kbps: 160,
            monitor: 0,
            draw_mouse: true,
            priority: Priority::BelowNormal,
            output_dir: None,
            ffmpeg_path: None,
            beep: true,
            notify: true,
            start_with_windows: true,
            window_position: None,
            window_size: None,
        }
    }
}

const PRESETS: &[&str] = &[
    "ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow",
];

/// Written on first run. Keep the values in sync with `Config::default`.
pub const TEMPLATE: &str = r#"# flagra settings.
# After editing: right-click the tray icon -> "Reload settings" (or restart flagra).

# Key combo that saves a clip. Modifiers: Ctrl, Alt, Shift, Win.
# Keys: A-Z, 0-9, F1-F24, Home, End, Insert, Delete, PageUp, PageDown,
#       PrintScreen, Pause, ScrollLock, Numpad0-Numpad9, Space, Up, Down, Left, Right.
hotkey = Alt+F10

# Key combo that opens the flagra window from anywhere (and closes it again). Empty = none.
open_hotkey = Alt+F9

# How many seconds before the key press end up in the clip.
clip_seconds = 20

# Capture frame rate. 30 roughly halves CPU use compared to 60.
fps = 60

# "native" = monitor resolution, or WIDTHxHEIGHT (e.g. 1280x720) to scale down on the GPU.
# Lower resolution = much less CPU.
resolution = native

# Video bitrate in kbit/s. Higher = better quality and bigger files, at almost the same CPU cost.
video_bitrate_kbps = 10000

# x264 preset. ultrafast uses the least CPU; each step slower costs noticeably more CPU
# for better quality at the same bitrate. Options: ultrafast superfast veryfast faster fast medium
preset = ultrafast

# Record desktop audio (whatever plays on your default output device).
audio = true
audio_bitrate_kbps = 160

# Which monitor to record (0 = first).
monitor = 0
draw_mouse = true

# CPU priority of the encoder: idle, below_normal, normal, above_normal.
# below_normal lets games win when the CPU is busy (the clip may stutter instead of your game).
priority = below_normal

# Where clips go. Empty = your Videos folder\flagra.
output_dir =

# Path to ffmpeg.exe. Empty = ffmpeg.exe next to flagra.exe, otherwise the one on PATH.
ffmpeg_path =

# Beep when a clip is saved (works in fullscreen games), and show a notification.
beep = true
notify = true

# Start flagra (recording in the tray) when you log in to Windows.
start_with_windows = true

# Where the window opens and how big, set by :move and :resize in the window. Empty = centered, default size.
window_position =
window_size =
"#;

impl Config {
    /// True when switching from `self` to `other` needs the capture restarted
    /// (which starts the replay buffer over). Everything else applies live.
    pub fn capture_differs(&self, other: &Config) -> bool {
        self.fps != other.fps
            || self.resolution != other.resolution
            || self.video_bitrate_kbps != other.video_bitrate_kbps
            || self.preset != other.preset
            || self.audio != other.audio
            || self.audio_bitrate_kbps != other.audio_bitrate_kbps
            || self.monitor != other.monitor
            || self.draw_mouse != other.draw_mouse
            || self.priority != other.priority
            || self.ffmpeg_path != other.ffmpeg_path
    }
}

/// Returns `text` with `key` set to `value`, keeping every other line and
/// comment as it was. Appends the key if the file doesn't have it.
pub fn set_value(text: &str, key: &str, value: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut done = false;
    for line in text.lines() {
        let code = line.split('#').next().unwrap_or("");
        let matches = !done
            && code.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key));
        if matches {
            out.push_str(&format!("{} = {}", key, value));
            done = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !done {
        out.push_str(&format!("{} = {}\n", key, value));
    }
    out
}

pub fn load(path: &Path) -> (Config, Vec<String>) {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            let _ = fs::write(path, TEMPLATE);
            TEMPLATE.to_string()
        }
    };
    parse(&text)
}

pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut c = Config::default();
    let mut warn = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            warn.push(format!("line {}: expected key = value", i + 1));
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        let mut bad = |what: &str| warn.push(format!("line {}: {} ({:?}), using default", i + 1, what, val));
        match key.as_str() {
            "hotkey" => match hotkey::parse(val) {
                Ok(_) => c.hotkey = val.to_string(),
                Err(e) => bad(&e),
            },
            "open_hotkey" => {
                if val.is_empty() || val.eq_ignore_ascii_case("off") || val.eq_ignore_ascii_case("none") {
                    c.open_hotkey = None;
                } else {
                    match hotkey::parse(val) {
                        Ok(_) => c.open_hotkey = Some(val.to_string()),
                        Err(e) => bad(&e),
                    }
                }
            }
            "window_position" => pair(val, false, &mut c.window_position, &mut bad),
            "window_size" => pair(val, true, &mut c.window_size, &mut bad),
            "clip_seconds" => num(val, 1, 600, &mut c.clip_seconds, &mut bad),
            "fps" => num(val, 1, 240, &mut c.fps, &mut bad),
            "video_bitrate_kbps" => num(val, 100, 200_000, &mut c.video_bitrate_kbps, &mut bad),
            "audio_bitrate_kbps" => num(val, 32, 512, &mut c.audio_bitrate_kbps, &mut bad),
            "monitor" => num(val, 0, 16, &mut c.monitor, &mut bad),
            "resolution" => {
                let v = val.to_ascii_lowercase();
                if v == "native" || v.is_empty() {
                    c.resolution = None;
                } else if let Some((w, h)) = v.split_once('x') {
                    match (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                        // x264 needs even dimensions for 4:2:0
                        (Ok(w), Ok(h)) if (64..=7680).contains(&w) && (64..=4320).contains(&h) => {
                            c.resolution = Some((w & !1, h & !1))
                        }
                        _ => bad("expected native or WIDTHxHEIGHT"),
                    }
                } else {
                    bad("expected native or WIDTHxHEIGHT");
                }
            }
            "preset" => {
                let v = val.to_ascii_lowercase();
                if PRESETS.contains(&v.as_str()) {
                    c.preset = v;
                } else {
                    bad("unknown x264 preset");
                }
            }
            "audio" => boolean(val, &mut c.audio, &mut bad),
            "draw_mouse" => boolean(val, &mut c.draw_mouse, &mut bad),
            "beep" => boolean(val, &mut c.beep, &mut bad),
            "notify" => boolean(val, &mut c.notify, &mut bad),
            "start_with_windows" => boolean(val, &mut c.start_with_windows, &mut bad),
            "priority" => {
                c.priority = match val.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
                    "idle" | "low" => Priority::Idle,
                    "below_normal" => Priority::BelowNormal,
                    "normal" => Priority::Normal,
                    "above_normal" => Priority::AboveNormal,
                    _ => {
                        bad("expected idle, below_normal, normal or above_normal");
                        c.priority
                    }
                }
            }
            "output_dir" => c.output_dir = path_value(val),
            "ffmpeg_path" => c.ffmpeg_path = path_value(val),
            _ => warn.push(format!("line {}: unknown setting {:?}", i + 1, key)),
        }
    }
    (c, warn)
}

fn num(val: &str, min: u32, max: u32, out: &mut u32, bad: &mut impl FnMut(&str)) {
    match val.parse::<u32>() {
        Ok(n) if (min..=max).contains(&n) => *out = n,
        _ => bad(&format!("expected a number from {} to {}", min, max)),
    }
}

/// "x, y" in logical pixels; empty = None. Sizes must be positive.
fn pair(val: &str, size: bool, out: &mut Option<[f64; 2]>, bad: &mut impl FnMut(&str)) {
    if val.is_empty() {
        *out = None;
        return;
    }
    let nums: Vec<f64> = val.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    match nums[..] {
        [a, b] if a.is_finite() && b.is_finite() && (!size || (a > 0.0 && b > 0.0)) => *out = Some([a, b]),
        _ => bad("expected two numbers like 120, 80"),
    }
}

fn boolean(val: &str, out: &mut bool, bad: &mut impl FnMut(&str)) {
    match val.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => *out = true,
        "false" | "no" | "off" | "0" => *out = false,
        _ => bad("expected true or false"),
    }
}

fn path_value(val: &str) -> Option<PathBuf> {
    let v = val.trim_matches('"').trim();
    (!v.is_empty()).then(|| PathBuf::from(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_matches_defaults() {
        let (c, warn) = parse(TEMPLATE);
        let d = Config::default();
        assert!(warn.is_empty(), "{:?}", warn);
        assert_eq!(format!("{:?}", c), format!("{:?}", d));
    }

    #[test]
    fn parses_values_and_rejects_bad_ones() {
        let (c, warn) = parse("fps = 30\nresolution = 1281x720\npreset = turbo\naudio = off\noutput_dir = \"D:\\Clips\"");
        assert_eq!(c.fps, 30);
        assert_eq!(c.resolution, Some((1280, 720)));
        assert_eq!(c.preset, "ultrafast");
        assert!(!c.audio);
        assert_eq!(c.output_dir, Some(PathBuf::from("D:\\Clips")));
        assert_eq!(warn.len(), 1);
    }

    #[test]
    fn set_value_keeps_comments_and_appends_missing_keys() {
        let text = "# fps comment\nfps = 60\n# fps = 10 in a comment\nbeep = true\n";
        let out = set_value(text, "fps", "30");
        assert_eq!(out, "# fps comment\nfps = 30\n# fps = 10 in a comment\nbeep = true\n");
        let out = set_value(&out, "monitor", "1");
        assert!(out.ends_with("beep = true\nmonitor = 1\n"));
        assert_eq!(parse(&out).0.fps, 30);
    }
}
