#![windows_subsystem = "windows"]

mod audio;
mod config;
mod hotkey;
mod logging;
mod recorder;
mod ts;
mod ui;
mod util;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::{
    FOLDERID_Videos, SHGetKnownFolderPath, ShellExecuteW, Shell_NotifyIconW, KNOWN_FOLDER_FLAG, NIF_ICON,
    NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use config::Config;
use recorder::Recorder;
use util::{copy_wide, wide};

pub const CLASS: PCWSTR = w!("PowerClipWnd");
const WM_TRAY: u32 = WM_APP + 1;
const WM_REMOTE_CLIP: u32 = WM_APP + 2;
const WM_REMOTE_QUIT: u32 = WM_APP + 3;
const WM_NOTE: u32 = WM_APP + 4;
/// From the GUI: reload powerclip.ini. wParam 1 = no "settings reloaded" notification.
pub const WM_REMOTE_RELOAD: u32 = WM_APP + 5;
/// From the GUI: returns (buffered ms << 32) | ffmpeg pid.
pub const WM_STATUS: u32 = WM_APP + 6;
const HOTKEY_ID: i32 = 1;

const ID_OPEN: usize = 6;
const ID_SAVE: usize = 1;
const ID_FOLDER: usize = 2;
const ID_SETTINGS: usize = 3;
const ID_RELOAD: usize = 4;
const ID_LOG: usize = 5;
const ID_QUIT: usize = 9;

struct App {
    dir: PathBuf,
    cfg_path: PathBuf,
    cfg: Arc<Mutex<Config>>,
    rec: Recorder,
    saving: AtomicBool,
    hwnd: AtomicIsize,
    taskbar_created: AtomicU32,
    /// Notification waiting to be shown on the UI thread: (title, text, is_error).
    note: Mutex<Option<(String, String, bool)>>,
}

static APP: OnceLock<App> = OnceLock::new();

fn app() -> &'static App {
    APP.get().unwrap()
}

fn hwnd() -> HWND {
    HWND(app().hwnd.load(Ordering::Relaxed) as _)
}

fn main() {
    // `powerclip --clip` / `--quit` control the running instance (bind to mouse
    // buttons, Stream Deck, AutoHotkey, ...).
    let arg = std::env::args().nth(1);
    if let Some(arg) = &arg {
        let msg = match arg.as_str() {
            "--clip" => WM_REMOTE_CLIP,
            "--quit" => WM_REMOTE_QUIT,
            _ => 0,
        };
        if msg != 0 {
            let ok = unsafe {
                FindWindowW(CLASS, PCWSTR::null())
                    .and_then(|h| PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0)))
                    .is_ok()
            };
            std::process::exit(if ok { 0 } else { 1 });
        }
    }

    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    // The GUI is a separate process (this same exe with --ui) that only exists
    // while its window is open, so the recorder never carries WebView2's memory.
    if arg.as_deref() == Some("--ui") {
        ui::run(dir);
        return;
    }
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("Local\\PowerClipSingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Launching PowerClip again (Start menu, desktop shortcut) opens the GUI.
            ui::run(dir);
            return;
        }
        std::mem::forget(_mutex);
    }
    logging::init(&dir.join("powerclip.log"));

    let cfg_path = dir.join("powerclip.ini");
    let (cfg, warnings) = config::load(&cfg_path);
    for w in &warnings {
        log!("settings: {}", w);
    }
    log!("PowerClip started: {:?}", cfg);
    let cfg = Arc::new(Mutex::new(cfg));
    let rec = Recorder::start(dir.clone(), cfg.clone());

    let _ = APP.set(App {
        dir,
        cfg_path,
        cfg,
        rec,
        saving: AtomicBool::new(false),
        hwnd: AtomicIsize::new(0),
        taskbar_created: AtomicU32::new(0),
        note: Mutex::new(None),
    });

    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0), CLASS, w!("PowerClip"), WS_OVERLAPPED,
            0, 0, 0, 0, None, None, Some(instance.into()), None,
        ) else {
            log!("cannot create window");
            return;
        };
        app().hwnd.store(h.0 as isize, Ordering::Relaxed);
        app().taskbar_created.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        add_tray_icon();
        register_hotkey();
        if !warnings.is_empty() {
            notify("PowerClip settings", &format!("{} (see powerclip.log)", warnings[0]), true);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let mut nid = tray_data();
        let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
    }
    app().rec.shutdown();
    log!("PowerClip stopped");
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => save_clip(),
        WM_REMOTE_CLIP => save_clip(),
        WM_REMOTE_QUIT => PostQuitMessage(0),
        WM_REMOTE_RELOAD => reload(wp.0 != 0),
        WM_STATUS => {
            let (pid, buffered) = app().rec.status();
            return LRESULT(((buffered.as_millis().min(u32::MAX as u128) as isize) << 32) | pid as isize);
        }
        WM_NOTE => {
            if let Some((title, text, err)) = app().note.lock().unwrap().take() {
                notify(&title, &text, err);
            }
        }
        WM_TRAY => match lp.0 as u32 {
            WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(h),
            WM_LBUTTONDBLCLK => open_ui(),
            _ => {}
        },
        WM_COMMAND => match wp.0 & 0xffff {
            ID_OPEN => open_ui(),
            ID_SAVE => save_clip(),
            ID_FOLDER => {
                let dir = recorder::output_dir(&app().cfg.lock().unwrap());
                let _ = std::fs::create_dir_all(&dir);
                open(&dir);
            }
            ID_SETTINGS => open(&app().cfg_path),
            ID_RELOAD => reload(false),
            ID_LOG => open(&app().dir.join("powerclip.log")),
            ID_QUIT => PostQuitMessage(0),
            _ => {}
        },
        _ if msg != 0 && msg == app().taskbar_created.load(Ordering::Relaxed) => add_tray_icon(),
        _ => return DefWindowProcW(h, msg, wp, lp),
    }
    LRESULT(0)
}

fn save_clip() {
    let a = app();
    if a.saving.swap(true, Ordering::AcqRel) {
        return; // still writing the previous clip
    }
    std::thread::spawn(|| {
        let a = app();
        let cfg = a.cfg.lock().unwrap().clone();
        let result = a.rec.save_clip();
        a.saving.store(false, Ordering::Release);
        let note = match result {
            Ok((path, secs)) => {
                log!("saved {:.0}s clip: {}", secs, path.display());
                if cfg.beep {
                    unsafe { let _ = MessageBeep(MB_OK); }
                }
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                cfg.notify.then(|| (format!("Saved last {:.0}s", secs), name, false))
            }
            Err(e) => {
                log!("clip failed: {}", e);
                if cfg.beep {
                    unsafe { let _ = MessageBeep(MB_ICONHAND); }
                }
                Some(("Clip failed".to_string(), e, true))
            }
        };
        if let Some(n) = note {
            *a.note.lock().unwrap() = Some(n);
            unsafe { let _ = PostMessageW(Some(hwnd()), WM_NOTE, WPARAM(0), LPARAM(0)); }
        }
    });
}

/// Re-reads powerclip.ini. Capture only restarts (and the replay buffer only
/// starts over) when an encoding/capture setting actually changed.
fn reload(quiet: bool) {
    let a = app();
    let (cfg, warnings) = config::load(&a.cfg_path);
    for w in &warnings {
        log!("settings: {}", w);
    }
    log!("settings reloaded: {:?}", cfg);
    let old = std::mem::replace(&mut *a.cfg.lock().unwrap(), cfg.clone());
    if old.capture_differs(&cfg) {
        a.rec.restart();
    } else if old.clip_seconds != cfg.clip_seconds {
        a.rec.set_clip_seconds(cfg.clip_seconds);
    }
    if old.hotkey != cfg.hotkey {
        register_hotkey();
    }
    unsafe {
        let mut nid = tray_data();
        let _ = Shell_NotifyIconW(NIM_MODIFY, &mut nid);
    }
    match warnings.first() {
        Some(w) => notify("PowerClip settings", &format!("{} (see powerclip.log)", w), true),
        None if !quiet => notify("PowerClip", "Settings reloaded", false),
        None => {}
    }
}

/// Opens the GUI in its own process (or focuses it if it's already open).
fn open_ui() {
    if let Ok(exe) = std::env::current_exe() {
        if let Err(e) = std::process::Command::new(exe).arg("--ui").spawn() {
            log!("cannot open the window: {}", e);
        }
    }
}

fn register_hotkey() {
    let combo = app().cfg.lock().unwrap().hotkey.clone();
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd()), HOTKEY_ID);
        let Ok((mods, vk)) = hotkey::parse(&combo) else { return };
        if RegisterHotKey(Some(hwnd()), HOTKEY_ID, mods, vk).is_err() {
            log!("hotkey {} is already taken by another program", combo);
            notify("PowerClip", &format!("Hotkey {} is used by another program. Pick another in settings.", combo), true);
        } else {
            log!("hotkey {} registered", combo);
        }
    }
}

fn tray_data() -> NOTIFYICONDATAW {
    let cfg = app().cfg.lock().unwrap();
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd(),
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: tray_icon(),
        ..Default::default()
    };
    copy_wide(&mut nid.szTip, &format!("PowerClip - {} saves the last {}s", cfg.hotkey, cfg.clip_seconds));
    nid
}

/// The app icon embedded in the exe (tauri-build puts it at resource 32512), at tray size.
fn tray_icon() -> HICON {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let size = GetSystemMetrics(SM_CXSMICON);
        LoadImageW(Some(instance.into()), PCWSTR(32512 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn add_tray_icon() {
    unsafe {
        let mut nid = tray_data();
        let _ = Shell_NotifyIconW(NIM_ADD, &mut nid);
    }
}

fn notify(title: &str, text: &str, error: bool) {
    let mut nid = tray_data();
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);
    unsafe { let _ = Shell_NotifyIconW(NIM_MODIFY, &mut nid); }
}

unsafe fn show_menu(h: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let hk = app().cfg.lock().unwrap().hotkey.clone();
    let save = wide(&format!("Save clip now\t{}", hk));
    let _ = AppendMenuW(menu, MF_STRING, ID_OPEN, w!("Open PowerClip"));
    let _ = SetMenuDefaultItem(menu, ID_OPEN as u32, 0);
    let _ = AppendMenuW(menu, MF_STRING, ID_SAVE, PCWSTR(save.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_FOLDER, w!("Open clips folder"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Edit powerclip.ini"));
    let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("Reload settings"));
    let _ = AppendMenuW(menu, MF_STRING, ID_LOG, w!("Open log"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Required so the menu closes when clicking elsewhere.
    let _ = SetForegroundWindow(h);
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, None, h, None);
    let _ = DestroyMenu(menu);
}

fn open(path: &std::path::Path) {
    let p = wide(&path.to_string_lossy());
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn videos_dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Videos, KNOWN_FOLDER_FLAG(0), None) {
            let s = p.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("Videos")).unwrap_or_else(|| PathBuf::from("."))
}
