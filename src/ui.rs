//! The GUI: a Tauri window running in its own process (`flagra.exe --ui`).
//! It exists only while the window is open; closing it ends the process and
//! WebView2 with it. It talks to the recorder through flagra.ini, the clips
//! folder and window messages, never by sharing memory.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Instant, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, State, WebviewWindow};
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GlobalFree, FILETIME, HANDLE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::{
    SHFileOperationW, ShellExecuteW, DROPFILES, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    FO_DELETE, SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_SHOWNORMAL,
};

use crate::config::{self, Config, Priority};
use crate::{hotkey, recorder, util::wide, CLASS, WM_REMOTE_CLIP, WM_REMOTE_QUIT, WM_REMOTE_RELOAD, WM_STATUS};

const CF_HDROP: u32 = 15;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Settings the GUI may change, in the order it shows them.
const KEYS: &[&str] = &[
    "hotkey", "open_hotkey", "clip_seconds", "fps", "resolution", "video_bitrate_kbps", "preset", "audio",
    "audio_bitrate_kbps", "monitor", "draw_mouse", "priority", "output_dir", "ffmpeg_path", "beep", "notify",
];

struct CpuSample {
    pid: u32,
    cpu_100ns: u64,
    at: Instant,
}

struct UiState {
    dir: PathBuf,
    cfg_path: PathBuf,
    cpu: Mutex<Option<CpuSample>>,
    /// Clip durations by path, valid while the file's modified time is unchanged.
    durations: Mutex<HashMap<PathBuf, (u64, Option<f64>)>>,
}

pub fn run(dir: PathBuf) {
    let cfg_path = dir.join("flagra.ini");
    tauri::Builder::default()
        // Opening flagra while the window is already up just focuses it.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| focus(app)))
        .manage(UiState {
            dir,
            cfg_path,
            cpu: Mutex::new(None),
            durations: Mutex::new(HashMap::new()),
        })
        .setup(|app| {
            let state = app.state::<UiState>();
            let cfg = config::load(&state.cfg_path).0;
            allow_clips_dir(app.handle(), &cfg);
            if let Some(window) = app.get_webview_window("main") {
                place(&window, &cfg);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ready, close, minimize, move_window, resize_window, save_placement, reset_placement, toggle_fullscreen,
            load, status, set_setting, list_clips, save_clip, delete_clip, rename_clip, copy_clip, reveal, play,
            open_special, start_recorder, stop_recorder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the flagra window");
}

fn focus(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Lets <video> load clips from the clips folder (and nothing else) over asset://.
fn allow_clips_dir(app: &AppHandle, cfg: &Config) {
    let dir = recorder::output_dir(cfg);
    let _ = fs::create_dir_all(&dir);
    let _ = app.asset_protocol_scope().allow_directory(&dir, false);
}

/// The window starts hidden and is shown once the page has drawn, so it never flashes white.
#[tauri::command]
fn ready(window: WebviewWindow) {
    let _ = window.show();
    let _ = window.set_focus();
}

const DEFAULT_SIZE: [f64; 2] = [980.0, 600.0];
const MIN_SIZE: [f64; 2] = [720.0, 420.0];

/// Opens the window on the monitor under the mouse, at the size and spot
/// :resize and :move left it (or centered). Always kept fully on screen.
fn place(window: &WebviewWindow, cfg: &Config) {
    if let Some([w, h]) = cfg.window_size {
        let _ = window.set_size(LogicalSize::new(w.max(MIN_SIZE[0]), h.max(MIN_SIZE[1])));
    }
    let placed = (|| {
        let cursor = window.cursor_position().ok()?;
        let monitor = window.monitor_from_point(cursor.x, cursor.y).ok()??;
        let size = window.outer_size().ok()?;
        let (origin, area, scale) = (monitor.position(), monitor.size(), monitor.scale_factor());
        let (w, h) = (size.width as i32, size.height as i32);
        let (x, y) = match cfg.window_position {
            Some([x, y]) => ((x * scale) as i32, (y * scale) as i32),
            None => ((area.width as i32 - w) / 2, (area.height as i32 - h) / 2),
        };
        let x = x.clamp(0, (area.width as i32 - w).max(0));
        let y = y.clamp(0, (area.height as i32 - h).max(0));
        window.set_position(PhysicalPosition::new(origin.x + x, origin.y + y)).ok()
    })();
    if placed.is_none() {
        let _ = window.center();
    }
}

#[tauri::command]
fn minimize(window: WebviewWindow) {
    let _ = window.minimize();
}

/// One :move step, in logical pixels.
#[tauri::command]
fn move_window(window: WebviewWindow, dx: f64, dy: f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    if let Ok(pos) = window.outer_position() {
        let _ = window.set_position(PhysicalPosition::new(pos.x + (dx * scale) as i32, pos.y + (dy * scale) as i32));
    }
}

/// One :resize step, in logical pixels.
#[tauri::command]
fn resize_window(window: WebviewWindow, dw: f64, dh: f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    if let Ok(size) = window.inner_size() {
        let size = size.to_logical::<f64>(scale);
        let _ = window.set_size(LogicalSize::new((size.width + dw).max(MIN_SIZE[0]), (size.height + dh).max(MIN_SIZE[1])));
    }
}

/// Remembers where :move ("position") or :resize ("size") left the window.
#[tauri::command]
fn save_placement(window: WebviewWindow, state: State<UiState>, what: String) -> Result<(), String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let value = if what == "position" {
        let monitor = window.current_monitor().ok().flatten().ok_or("the window isn't on a monitor")?;
        let pos = window.outer_position().map_err(|e| e.to_string())?;
        let origin = monitor.position();
        format!("{}, {}", ((pos.x - origin.x) as f64 / scale).round(), ((pos.y - origin.y) as f64 / scale).round())
    } else {
        let size = window.inner_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
        format!("{}, {}", size.width.round(), size.height.round())
    };
    write_placement(&state, &what, &value)
}

/// :move reset / :resize reset: back to centered, or to the default size.
#[tauri::command]
fn reset_placement(window: WebviewWindow, state: State<UiState>, what: String) -> Result<(), String> {
    write_placement(&state, &what, "")?;
    if what == "size" {
        let _ = window.set_size(LogicalSize::new(DEFAULT_SIZE[0], DEFAULT_SIZE[1]));
    }
    place(&window, &config::load(&state.cfg_path).0);
    Ok(())
}

fn write_placement(state: &UiState, what: &str, value: &str) -> Result<(), String> {
    let key = if what == "position" { "window_position" } else { "window_size" };
    let text = fs::read_to_string(&state.cfg_path).unwrap_or_else(|_| config::TEMPLATE.to_string());
    fs::write(&state.cfg_path, config::set_value(&text, key, value)).map_err(|e| format!("cannot save settings: {}", e))
}

/// :f / :fullscreen. Returns whether the window is now fullscreen.
#[tauri::command]
fn toggle_fullscreen(window: WebviewWindow) -> bool {
    let on = !window.is_fullscreen().unwrap_or(false);
    let _ = window.set_fullscreen(on);
    on
}

/// Closing ends this process, and WebView2 with it; the recorder keeps going.
#[tauri::command]
fn close(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn load(state: State<UiState>) -> Value {
    let (cfg, warnings) = config::load(&state.cfg_path);
    json!({
        "config": config_json(&cfg),
        "warnings": warnings,
        "ini": state.cfg_path.to_string_lossy(),
        "cpus": std::thread::available_parallelism().map_or(1, |n| n.get()),
    })
}

fn config_json(c: &Config) -> Value {
    json!({
        "hotkey": c.hotkey,
        "open_hotkey": c.open_hotkey,
        "clip_seconds": c.clip_seconds,
        "fps": c.fps,
        "resolution": c.resolution.map_or("native".to_string(), |(w, h)| format!("{}x{}", w, h)),
        "video_bitrate_kbps": c.video_bitrate_kbps,
        "preset": c.preset,
        "audio": c.audio,
        "audio_bitrate_kbps": c.audio_bitrate_kbps,
        "monitor": c.monitor,
        "draw_mouse": c.draw_mouse,
        "priority": match c.priority {
            Priority::Idle => "idle",
            Priority::BelowNormal => "below_normal",
            Priority::Normal => "normal",
            Priority::AboveNormal => "above_normal",
        },
        "output_dir": recorder::output_dir(c).to_string_lossy(),
        "output_dir_custom": c.output_dir.is_some(),
        "ffmpeg_path": c.ffmpeg_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "beep": c.beep,
        "notify": c.notify,
    })
}

fn recorder_window() -> Option<HWND> {
    unsafe { FindWindowW(CLASS, PCWSTR::null()).ok() }
}

#[derive(Serialize, Default)]
struct Status {
    running: bool,
    recording: bool,
    buffered: f64,
    cpu: Option<f64>,
    mem_mb: Option<f64>,
    error: Option<String>,
}

#[tauri::command]
fn status(state: State<UiState>) -> Status {
    let Some(h) = recorder_window() else { return Status::default() };
    let mut res = 0usize;
    let answered = unsafe {
        SendMessageTimeoutW(h, WM_STATUS, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 500, Some(&mut res))
    };
    if answered.0 == 0 {
        return Status { running: true, error: Some("the recorder isn't responding".into()), ..Default::default() };
    }
    let pid = (res & 0xffff_ffff) as u32;
    let mut st = Status { running: true, recording: pid != 0, buffered: (res >> 32) as f64 / 1000.0, ..Default::default() };
    if pid == 0 {
        st.error = last_capture_error(&state.dir);
        return st;
    }
    if let Some((cpu_100ns, mem)) = unsafe { process_stats(pid) } {
        st.mem_mb = mem;
        let now = Instant::now();
        let mut prev = state.cpu.lock().unwrap();
        if let Some(p) = prev.as_ref().filter(|p| p.pid == pid) {
            let wall = now.duration_since(p.at).as_secs_f64();
            let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
            if wall > 0.2 {
                st.cpu = Some((cpu_100ns.saturating_sub(p.cpu_100ns)) as f64 / 1e7 / wall / cpus * 100.0);
            }
        }
        *prev = Some(CpuSample { pid, cpu_100ns, at: now });
    }
    st
}

/// Total CPU time (100ns units) and working set (MB) of a process.
unsafe fn process_stats(pid: u32) -> Option<(u64, Option<f64>)> {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    let times = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user).is_ok();
    let mut pmc = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    let mem = GetProcessMemoryInfo(h, &mut pmc, pmc.cb).is_ok().then(|| pmc.WorkingSetSize as f64 / 1_048_576.0);
    let _ = CloseHandle(h);
    let ticks = |t: FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
    times.then(|| (ticks(kernel) + ticks(user), mem))
}

/// The recorder's most recent capture problem, from its log.
fn last_capture_error(dir: &Path) -> Option<String> {
    let mut f = File::open(dir.join("flagra.log")).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(16 * 1024))).ok()?;
    let mut tail = String::new();
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    tail.push_str(&String::from_utf8_lossy(&bytes));
    tail.lines()
        .rev()
        .filter_map(|l| l.split_once("] ").map(|(_, m)| m))
        .find(|m| m.starts_with("capture:") && !m.starts_with("capture: starting"))
        .map(|m| m.trim_start_matches("capture: ").to_string())
}

/// Changes one setting in flagra.ini (keeping its comments) and tells the
/// recorder to pick it up.
#[tauri::command]
fn set_setting(app: AppHandle, state: State<UiState>, key: String, value: String) -> Result<Value, String> {
    if !KEYS.contains(&key.as_str()) {
        return Err(format!("unknown setting {}", key));
    }
    let value = value.trim();
    if value.contains(['#', '\n', '\r']) {
        return Err("values can't contain # or line breaks".into());
    }
    // Validate the value alone first, so a bad one is reported instead of silently ignored.
    let (_, warnings) = config::parse(&format!("{} = {}", key, value));
    if let Some(w) = warnings.first() {
        return Err(w.trim_start_matches("line 1: ").replace(", using default", ""));
    }
    let text = fs::read_to_string(&state.cfg_path).unwrap_or_else(|_| config::TEMPLATE.to_string());
    let (old, _) = config::parse(&text);
    let new_text = config::set_value(&text, &key, value);
    let (cfg, _) = config::parse(&new_text);
    if let (Some(open), true) = (&cfg.open_hotkey, key.ends_with("hotkey")) {
        if open.eq_ignore_ascii_case(&cfg.hotkey) {
            return Err(format!("{} is already the {} hotkey", open, if key == "hotkey" { "open" } else { "clip" }));
        }
    }
    let changed = match key.as_str() {
        "hotkey" => Some(&cfg.hotkey).filter(|k| !k.eq_ignore_ascii_case(&old.hotkey)),
        "open_hotkey" => cfg.open_hotkey.as_ref().filter(|k| old.open_hotkey.as_ref().is_none_or(|o| !k.eq_ignore_ascii_case(o))),
        _ => None,
    };
    if let Some(combo) = changed {
        if !hotkey_free(combo) {
            return Err(format!("{} is already used by another program", combo));
        }
    }
    fs::write(&state.cfg_path, new_text).map_err(|e| format!("cannot save settings: {}", e))?;
    if let Some(h) = recorder_window() {
        unsafe { let _ = PostMessageW(Some(h), WM_REMOTE_RELOAD, WPARAM(1), LPARAM(0)); }
    }
    if key == "output_dir" {
        allow_clips_dir(&app, &cfg);
    }
    Ok(json!({ "config": config_json(&cfg), "restarted": old.capture_differs(&cfg) }))
}

/// Whether a combo can be registered (i.e. no other program holds it).
fn hotkey_free(combo: &str) -> bool {
    let Ok((mods, vk)) = hotkey::parse(combo) else { return false };
    unsafe {
        let free = RegisterHotKey(None, 0x7ffe, mods, vk).is_ok();
        if free {
            let _ = UnregisterHotKey(None, 0x7ffe);
        }
        free
    }
}

#[derive(Serialize)]
struct Clip {
    name: String,
    path: String,
    size: u64,
    /// Unix milliseconds.
    modified: u64,
    duration: Option<f64>,
}

#[tauri::command]
fn list_clips(state: State<UiState>) -> Vec<Clip> {
    let dir = recorder::output_dir(&config::load(&state.cfg_path).0);
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut cache = state.durations.lock().unwrap();
    let mut clips: Vec<Clip> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
            let path = e.path();
            let duration = match cache.get(&path) {
                Some(&(m, d)) if m == modified => d,
                _ => {
                    let d = mp4_duration(&path);
                    cache.insert(path.clone(), (modified, d));
                    d
                }
            };
            Some(Clip {
                name: e.file_name().to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
                size: meta.len(),
                modified,
                duration,
            })
        })
        .collect();
    clips.sort_by(|a, b| b.modified.cmp(&a.modified));
    clips
}

/// Reads the duration from the mp4's movie header without decoding anything.
fn mp4_duration(path: &Path) -> Option<f64> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let (moov, moov_end) = find_box(&mut f, 0, len, b"moov")?;
    let (mvhd, _) = find_box(&mut f, moov, moov_end, b"mvhd")?;
    f.seek(SeekFrom::Start(mvhd)).ok()?;
    let mut b = [0u8; 32];
    f.read_exact(&mut b).ok()?;
    let (timescale, duration) = if b[0] == 1 {
        (u32::from_be_bytes(b[20..24].try_into().ok()?) as f64, u64::from_be_bytes(b[24..32].try_into().ok()?) as f64)
    } else {
        (u32::from_be_bytes(b[12..16].try_into().ok()?) as f64, u32::from_be_bytes(b[16..20].try_into().ok()?) as f64)
    };
    (timescale > 0.0).then(|| duration / timescale)
}

/// Finds a box of type `kind` between `start` and `end`; returns where its payload starts and where it ends.
fn find_box(f: &mut File, start: u64, end: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let mut pos = start;
    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes(h[..4].try_into().ok()?) as u64;
        let mut header = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().ok()?);
            header = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < header {
            return None;
        }
        if &h[4..8] == kind {
            return Some((pos + header, (pos + size).min(end)));
        }
        pos += size;
    }
    None
}

/// Only clips inside the clips folder are touched.
fn clip_path(state: &UiState, path: &str) -> Result<PathBuf, String> {
    let dir = recorder::output_dir(&config::load(&state.cfg_path).0);
    let p = PathBuf::from(path);
    let inside = p.parent().and_then(|d| d.canonicalize().ok()) == dir.canonicalize().ok();
    let mp4 = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4"));
    if !(inside && mp4 && p.is_file()) {
        return Err("that clip no longer exists".into());
    }
    Ok(p)
}

#[tauri::command]
fn save_clip() -> Result<(), String> {
    let h = recorder_window().ok_or("flagra isn't recording (:start starts it)")?;
    unsafe { PostMessageW(Some(h), WM_REMOTE_CLIP, WPARAM(0), LPARAM(0)) }.map_err(|e| e.to_string())
}

/// Moves a clip to the Recycle Bin (never deletes it outright).
#[tauri::command]
fn delete_clip(state: State<UiState>, path: String) -> Result<(), String> {
    let p = clip_path(&state, &path)?;
    let from: Vec<u16> = p.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 || op.fAnyOperationsAborted.as_bool() || p.exists() {
        return Err(format!("couldn't move it to the Recycle Bin (error {})", r));
    }
    Ok(())
}

#[tauri::command]
fn rename_clip(state: State<UiState>, path: String, name: String) -> Result<String, String> {
    let p = clip_path(&state, &path)?;
    let mut name = name.trim().to_string();
    if name.to_ascii_lowercase().ends_with(".mp4") {
        name.truncate(name.len() - 4);
    }
    let name = name.trim_end_matches([' ', '.']);
    if name.is_empty() || name.len() > 200 || name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']) {
        return Err("a name can't be empty or contain < > : \" / \\ | ? *".into());
    }
    let target = p.with_file_name(format!("{}.mp4", name));
    if target == p {
        return Ok(path);
    }
    if target.exists() {
        return Err(format!("{}.mp4 already exists", name));
    }
    fs::rename(&p, &target).map_err(|e| format!("cannot rename: {}", e))?;
    Ok(target.to_string_lossy().into_owned())
}

/// Puts the clip file on the clipboard, ready to paste into Discord, Explorer, chats...
#[tauri::command]
fn copy_clip(state: State<UiState>, path: String) -> Result<(), String> {
    let p = clip_path(&state, &path)?;
    let file: Vec<u16> = p.as_os_str().encode_wide().chain([0, 0]).collect();
    let header = std::mem::size_of::<DROPFILES>();
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, header + file.len() * 2).map_err(|e| e.to_string())?;
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("out of memory".into());
        }
        ptr.cast::<DROPFILES>().write(DROPFILES {
            pFiles: header as u32,
            pt: POINT::default(),
            fNC: BOOL(0),
            fWide: BOOL(1),
        });
        std::ptr::copy_nonoverlapping(file.as_ptr(), ptr.add(header).cast::<u16>(), file.len());
        let _ = GlobalUnlock(mem);
        if OpenClipboard(None).is_err() {
            let _ = GlobalFree(Some(mem));
            return Err("the clipboard is busy, try again".into());
        }
        let _ = EmptyClipboard();
        let set = SetClipboardData(CF_HDROP, Some(HANDLE(mem.0)));
        let _ = CloseClipboard();
        if set.is_err() {
            let _ = GlobalFree(Some(mem));
            return Err("couldn't put the clip on the clipboard".into());
        }
    }
    Ok(())
}

/// Opens Explorer with the clip selected.
#[tauri::command]
fn reveal(state: State<UiState>, path: String) -> Result<(), String> {
    let p = clip_path(&state, &path)?;
    Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", p.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Plays the clip in the default video player.
#[tauri::command]
fn play(state: State<UiState>, path: String) -> Result<(), String> {
    shell_open(&clip_path(&state, &path)?);
    Ok(())
}

#[tauri::command]
fn open_special(state: State<UiState>, what: String) -> Result<(), String> {
    let path = match what.as_str() {
        "folder" => {
            let dir = recorder::output_dir(&config::load(&state.cfg_path).0);
            let _ = fs::create_dir_all(&dir);
            dir
        }
        "log" => state.dir.join("flagra.log"),
        "ffmpeg_log" => state.dir.join("ffmpeg.log"),
        "ini" => state.cfg_path.clone(),
        _ => return Err(format!("nothing to open called {}", what)),
    };
    if !path.exists() {
        return Err(format!("{} doesn't exist yet", path.display()));
    }
    shell_open(&path);
    Ok(())
}

fn shell_open(path: &Path) {
    let p = wide(&path.to_string_lossy());
    unsafe {
        ShellExecuteW(None, windows::core::w!("open"), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

#[tauri::command]
fn start_recorder() -> Result<(), String> {
    if recorder_window().is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start the recorder: {}", e))
}

#[tauri::command]
fn stop_recorder() {
    if let Some(h) = recorder_window() {
        unsafe { let _ = PostMessageW(Some(h), WM_REMOTE_QUIT, WPARAM(0), LPARAM(0)); }
    }
}
