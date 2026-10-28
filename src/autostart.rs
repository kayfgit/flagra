//! Start with Windows: the per-user Run key, the same mechanism tradu's autostart uses.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

use crate::log;
use crate::util::wide;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("flagra");

/// Points the Run entry at this exe (or removes it). Run on every start, so it
/// follows the exe if it moves. Windows' own "Startup apps" switch, in Task
/// Manager or Settings, still wins: it's stored separately and left alone.
pub fn sync(enabled: bool) {
    unsafe {
        if enabled {
            let Ok(exe) = std::env::current_exe() else { return };
            let data = wide(&format!("\"{}\"", exe.display()));
            let r = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE,
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            );
            if r != ERROR_SUCCESS {
                log!("cannot set start with Windows: error {}", r.0);
            }
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE);
            if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
                log!("cannot remove start with Windows: error {}", r.0);
            }
        }
    }
}
