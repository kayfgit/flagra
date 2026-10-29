//! The "clip saved" popup: a small toast in a corner of the screen the game is
//! on, like ShadowPlay's. It never takes focus and clicks go through it, so the
//! game keeps running underneath. The recorder draws it itself with GDI, so it
//! shows instantly and costs nothing between clips. Runs on the UI thread.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Corner;
use crate::util::wide;

const CLASS: PCWSTR = w!("FlagraPopup");
const TIMER: usize = 1;
const FADE_IN_MS: f32 = 120.0;
const HOLD_MS: f32 = 2600.0;
const FADE_OUT_MS: f32 = 450.0;
const OPACITY: f32 = 240.0;

// The app's palette (ui/style.css).
const BG: COLORREF = rgb(0x1c, 0x1a, 0x16);
const RULE: COLORREF = rgb(0x2a, 0x27, 0x22);
const BRIGHT: COLORREF = rgb(0xf7, 0xf3, 0xea);
const MUTED: COLORREF = rgb(0x9a, 0x92, 0x83);
const AMBER: COLORREF = rgb(0xf2, 0xa3, 0x3a);
const ERROR: COLORREF = rgb(0xef, 0x7a, 0x62);

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

struct Toast {
    title: String,
    detail: String,
    error: bool,
    scale: f32,
    since: Instant,
}

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static TOAST: Mutex<Option<Toast>> = Mutex::new(None);

/// Shows (or replaces) the popup. Call on the thread that runs the message loop.
pub fn show(title: &str, detail: &str, error: bool, corner: Corner) {
    unsafe {
        let Some(hwnd) = window() else { return };
        // The monitor the user is looking at: the one with the foreground window (the game).
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let (w, h, margin) = (px(360.0), px(64.0), px(20.0));
        let area = info.rcWork;
        let x = match corner {
            Corner::TopLeft | Corner::BottomLeft => area.left + margin,
            Corner::TopRight | Corner::BottomRight => area.right - w - margin,
        };
        let y = match corner {
            Corner::TopLeft | Corner::TopRight => area.top + margin,
            Corner::BottomLeft | Corner::BottomRight => area.bottom - h - margin,
        };

        *TOAST.lock().unwrap() = Some(Toast {
            title: title.to_string(),
            detail: detail.to_string(),
            error,
            scale,
            since: Instant::now(),
        });
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        // Topmost again every time: games and fullscreen apps push themselves up too.
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = InvalidateRect(Some(hwnd), None, true);
        SetTimer(Some(hwnd), TIMER, 15, None);
    }
}

/// The popup window, created the first time it's needed.
unsafe fn window() -> Option<HWND> {
    let existing = WINDOW.load(Ordering::Relaxed);
    if existing != 0 {
        return Some(HWND(existing as _));
    }
    let instance = GetModuleHandleW(None).ok()?;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        lpszClassName: CLASS,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&wc);
    // Layered + transparent = click-through; no-activate + tool window = never
    // steals focus from the game and stays off the taskbar and Alt+Tab.
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
        CLASS,
        w!("flagra"),
        WS_POPUP,
        0, 0, 0, 0,
        None, None, Some(instance.into()), None,
    )
    .ok()?;
    let round = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &round as *const _ as *const _,
        std::mem::size_of_val(&round) as u32,
    );
    WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
    Some(hwnd)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER if wp.0 == TIMER => {
            fade(hwnd);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn fade(hwnd: HWND) {
    let Some(since) = TOAST.lock().unwrap().as_ref().map(|t| t.since) else { return };
    let ms = since.elapsed().as_secs_f32() * 1000.0;
    let level = if ms < FADE_IN_MS {
        ms / FADE_IN_MS
    } else if ms < FADE_IN_MS + HOLD_MS {
        1.0
    } else {
        1.0 - (ms - FADE_IN_MS - HOLD_MS) / FADE_OUT_MS
    };
    if level <= 0.0 {
        let _ = KillTimer(Some(hwnd), TIMER);
        let _ = ShowWindow(hwnd, SW_HIDE);
        *TOAST.lock().unwrap() = None;
        return;
    }
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (level * OPACITY) as u8, LWA_ALPHA);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if let Some(t) = TOAST.lock().unwrap().as_ref() {
        let px = |v: f32| (v * t.scale).round() as i32;
        fill(hdc, &rc, BG);
        let edge = CreateSolidBrush(RULE);
        FrameRect(hdc, &rc, edge);
        let _ = DeleteObject(edge.into());
        // The accent strip on the left, amber (red if the clip failed).
        let strip = RECT { left: 0, top: 0, right: px(4.0), bottom: rc.bottom };
        fill(hdc, &strip, if t.error { ERROR } else { AMBER });

        let face = font_face();
        let title_font = font(&face, px(15.0), 600);
        let detail_font = font(&face, px(12.5), 400);
        SetBkMode(hdc, TRANSPARENT);
        let left = px(18.0);
        let right = rc.right - px(14.0);

        let old = SelectObject(hdc, title_font.into());
        SetTextColor(hdc, if t.error { ERROR } else { BRIGHT });
        let mut r = RECT { left, top: px(12.0), right, bottom: px(34.0) };
        text(hdc, &t.title, &mut r);
        SelectObject(hdc, detail_font.into());
        SetTextColor(hdc, MUTED);
        let mut r = RECT { left, top: px(35.0), right, bottom: px(54.0) };
        text(hdc, &t.detail, &mut r);
        SelectObject(hdc, old);
        let _ = DeleteObject(title_font.into());
        let _ = DeleteObject(detail_font.into());
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn fill(hdc: HDC, rc: &RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, rc, brush);
    let _ = DeleteObject(brush.into());
}

unsafe fn text(hdc: HDC, s: &str, r: &mut RECT) {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(hdc, &mut buf, r, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
}

unsafe fn font(face: &[u16], height: i32, weight: i32) -> HFONT {
    CreateFontW(
        -height, 0, 0, 0, weight, 0, 0, 0,
        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
        (FF_MODERN.0 | FIXED_PITCH.0) as u32,
        PCWSTR(face.as_ptr()),
    )
}

/// The window's monospace font if it's installed, else Windows' own.
fn font_face() -> Vec<u16> {
    for face in ["JetBrains Mono", "Cascadia Mono", "Consolas"] {
        if unsafe { font_installed(face) } {
            return wide(face);
        }
    }
    wide("Consolas")
}

unsafe fn font_installed(face: &str) -> bool {
    unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, flag: LPARAM) -> i32 {
        *(flag.0 as *mut bool) = true;
        0
    }
    let mut lf = LOGFONTW { lfCharSet: DEFAULT_CHARSET, ..Default::default() };
    for (dst, src) in lf.lfFaceName.iter_mut().zip(face.encode_utf16()) {
        *dst = src;
    }
    let mut hit = false;
    let hdc = GetDC(None);
    EnumFontFamiliesExW(hdc, &lf, Some(found), LPARAM(&mut hit as *mut bool as isize), 0);
    ReleaseDC(None, hdc);
    hit
}
