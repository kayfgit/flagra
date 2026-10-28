use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Truncates the log on every app start so it never grows across runs.
pub fn init(path: &Path) {
    if let Ok(f) = File::create(path) {
        let _ = LOG.set(Mutex::new(f));
    }
}

pub fn write(msg: &str) {
    if let Some(m) = LOG.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "[{}] {}", crate::util::clock_stamp(), msg);
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::logging::write(&format!($($t)*)) };
}
