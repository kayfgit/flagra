//! Desktop audio via WASAPI loopback, turned into a gap-free 48 kHz f32 stereo
//! stream whose sample N corresponds exactly to time `t0 + N / 48000` on the QPC
//! clock. That is what keeps it in sync with the video ffmpeg captures.
//!
//! Loopback delivers nothing while nothing is playing, so silence is synthesized
//! from the clock; packets are placed by their own QPC timestamps, which also
//! absorbs drift between the sound card's clock and the system clock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};

use crate::log;
use crate::util::qpc_100ns;

pub const RATE: u32 = 48_000;
const BYTES_PER_FRAME: usize = 8; // 2 channels * f32
/// How far behind real time the stream runs, so late packets still land in place.
/// Must stay well under ffmpeg's 100 ms scheduling tolerance: if audio trails the
/// video by more than that, ffmpeg pauses the video capture and frames get lost.
const LATENCY_100NS: i64 = 400_000; // 40 ms
/// Timestamp mismatch tolerated before re-aligning (avoids clicks from rounding jitter).
const TOLERANCE_FRAMES: i64 = RATE as i64 / 50; // 20 ms

struct Timeline {
    t0: i64,
    written: i64,
    tx: Sender<Vec<u8>>,
}

impl Timeline {
    fn frame_at(&self, t_100ns: i64) -> i64 {
        ((t_100ns - self.t0) as i128 * RATE as i128 / 10_000_000) as i64
    }

    /// Returns false once the consumer (ffmpeg) is gone.
    fn pad_to(&mut self, frame: i64) -> bool {
        while self.written < frame {
            let n = (frame - self.written).min(RATE as i64 / 10);
            if self.tx.send(vec![0u8; n as usize * BYTES_PER_FRAME]).is_err() {
                return false;
            }
            self.written += n;
        }
        true
    }

    fn push(&mut self, at_100ns: Option<i64>, frames: usize, data: Option<&[u8]>) -> bool {
        let mut skip = 0usize;
        if let Some(t) = at_100ns {
            let diff = self.frame_at(t) - self.written;
            if diff > TOLERANCE_FRAMES {
                if !self.pad_to(self.written + diff) {
                    return false;
                }
            } else if diff < -TOLERANCE_FRAMES {
                // Arrived too late (or the device clock runs fast): drop what overlaps.
                skip = ((-diff) as usize).min(frames);
            }
        }
        let n = frames - skip;
        if n == 0 {
            return true;
        }
        let buf = match data {
            Some(d) => d[skip * BYTES_PER_FRAME..frames * BYTES_PER_FRAME].to_vec(),
            None => vec![0u8; n * BYTES_PER_FRAME],
        };
        self.written += n as i64;
        self.tx.send(buf).is_ok()
    }
}

struct Loopback {
    enumerator: IMMDeviceEnumerator,
    device_id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

impl Loopback {
    fn open() -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let device_id = take_pwstr(device.GetId()?);
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            // Ask for exactly what ffmpeg is told to expect; Windows converts
            // from whatever the device mix format is (channels, rate, depth).
            let fmt = WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: 2,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * BYTES_PER_FRAME as u32,
                nBlockAlign: BYTES_PER_FRAME as u16,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                10_000_000, // 1 s buffer: generous, we poll every 10 ms
                0,
                &fmt,
                None,
            )?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Loopback { enumerator, device_id, client, capture })
        }
    }

    fn drain(&self, tl: &mut Timeline) -> windows::core::Result<bool> {
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                let at = (flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0).then_some(qpc as i64);
                let bytes = (!silent).then(|| std::slice::from_raw_parts(data, frames as usize * BYTES_PER_FRAME));
                let alive = tl.push(at, frames as usize, bytes);
                self.capture.ReleaseBuffer(frames)?;
                if !alive {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// True when Windows' default output device is no longer the one we capture.
    fn default_changed(&self) -> bool {
        unsafe {
            match self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole).and_then(|d| d.GetId()) {
                Ok(id) => take_pwstr(id) != self.device_id,
                Err(_) => true,
            }
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe { let _ = self.client.Stop(); }
    }
}

unsafe fn take_pwstr(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

/// Runs until `stop` is set or the consumer hangs up. `t0` (QPC, 100ns) is the
/// time of audio sample 0.
pub fn run(stop: &AtomicBool, t0: i64, tx: Sender<Vec<u8>>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    let mut tl = Timeline { t0, written: 0, tx };
    let mut last_err = String::new();
    let tick = Duration::from_millis(10);
    let catch_up = |tl: &mut Timeline| {
        let target = tl.frame_at(qpc_100ns() - LATENCY_100NS);
        tl.pad_to(target)
    };

    while !stop.load(Ordering::Relaxed) {
        match Loopback::open() {
            Ok(lb) => {
                log!("audio: capturing default output device");
                last_err.clear();
                let mut last_check = Instant::now();
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    match lb.drain(&mut tl) {
                        Ok(true) => {}
                        Ok(false) => return,
                        Err(e) => {
                            log!("audio: capture error, reopening: {}", e);
                            break;
                        }
                    }
                    if !catch_up(&mut tl) {
                        return;
                    }
                    if last_check.elapsed() > Duration::from_secs(2) {
                        last_check = Instant::now();
                        if lb.default_changed() {
                            log!("audio: default output device changed, switching");
                            break;
                        }
                    }
                    std::thread::sleep(tick);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg != last_err {
                    log!("audio: cannot open loopback capture (recording silence): {}", msg);
                    last_err = msg;
                }
            }
        }
        // Keep the stream flowing with silence while retrying.
        for _ in 0..50 {
            if stop.load(Ordering::Relaxed) || !catch_up(&mut tl) {
                return;
            }
            std::thread::sleep(tick);
        }
    }
}
