//! Desktop audio (WASAPI loopback) and the microphone, mixed into one gap-free
//! 48 kHz f32 stereo stream whose sample N corresponds exactly to time
//! `t0 + N / 48000` on the QPC clock. That is what keeps it in sync with the
//! video ffmpeg captures.
//!
//! Each source's packets are placed on that timeline by their own QPC
//! timestamps (which also absorbs drift between sound card and system clocks),
//! scaled by its volume and summed. Loopback delivers nothing while nothing is
//! playing, so the mix is flushed by the clock and gaps simply stay silent.
//! Volumes and which sources are on apply live, from `Controls`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};

use crate::config::Config;
use crate::log;
use crate::util::qpc_100ns;

pub const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// How far behind real time the mix is flushed, so late packets still land in
/// place. Kept well under ffmpeg's scheduling slack (see AUDIO_SHIFT_SECS in
/// recorder.rs): microphones can deliver a little later than loopback.
const LATENCY_100NS: i64 = 600_000; // 60 ms
/// Timestamp mismatch tolerated before re-aligning (avoids clicks from rounding jitter).
const TOLERANCE_FRAMES: i64 = RATE as i64 / 50; // 20 ms

/// What the recorder can change while recording: which sources are on, their
/// volumes, and the peak levels the window shows as meters.
pub struct Controls {
    desktop_on: AtomicBool,
    mic_on: AtomicBool,
    desktop_gain: AtomicU32,
    mic_gain: AtomicU32,
    desktop_peak: AtomicU32,
    mic_peak: AtomicU32,
}

impl Controls {
    pub fn new(cfg: &Config) -> Self {
        let c = Controls {
            desktop_on: AtomicBool::new(false),
            mic_on: AtomicBool::new(false),
            desktop_gain: AtomicU32::new(0),
            mic_gain: AtomicU32::new(0),
            desktop_peak: AtomicU32::new(0),
            mic_peak: AtomicU32::new(0),
        };
        c.apply(cfg);
        c
    }

    pub fn apply(&self, cfg: &Config) {
        self.desktop_on.store(cfg.audio, Ordering::Relaxed);
        self.mic_on.store(cfg.mic, Ordering::Relaxed);
        self.desktop_gain.store((cfg.desktop_volume as f32 / 100.0).to_bits(), Ordering::Relaxed);
        self.mic_gain.store((cfg.mic_volume as f32 / 100.0).to_bits(), Ordering::Relaxed);
    }

    /// Loudest sample of each source (after its volume) since the last call;
    /// 1.0 is full scale, above that clips.
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.desktop_peak.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.mic_peak.swap(0, Ordering::Relaxed)),
        )
    }

    fn on(&self, kind: Kind) -> bool {
        match kind {
            Kind::Desktop => self.desktop_on.load(Ordering::Relaxed),
            Kind::Mic => self.mic_on.load(Ordering::Relaxed),
        }
    }

    fn gain(&self, kind: Kind) -> f32 {
        f32::from_bits(match kind {
            Kind::Desktop => self.desktop_gain.load(Ordering::Relaxed),
            Kind::Mic => self.mic_gain.load(Ordering::Relaxed),
        })
    }

    fn note_peak(&self, kind: Kind, peak: f32) {
        let slot = match kind {
            Kind::Desktop => &self.desktop_peak,
            Kind::Mic => &self.mic_peak,
        };
        let _ = slot.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
            (peak > f32::from_bits(old)).then(|| peak.to_bits())
        });
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Desktop,
    Mic,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Desktop => "desktop audio",
            Kind::Mic => "microphone",
        }
    }

    fn flow(self) -> EDataFlow {
        match self {
            Kind::Desktop => eRender,
            Kind::Mic => eCapture,
        }
    }
}

/// The mixed timeline: samples from `flushed` on, still open to more sources.
struct Mixer {
    t0: i64,
    flushed: i64,
    /// Interleaved stereo, starting at frame `flushed`.
    buf: Vec<f32>,
    tx: Sender<Vec<u8>>,
}

impl Mixer {
    fn frame_at(&self, t_100ns: i64) -> i64 {
        ((t_100ns - self.t0) as i128 * RATE as i128 / 10_000_000) as i64
    }

    /// Adds `data` (interleaved stereo) times `gain` at frame `start`. Whatever
    /// falls before `flushed` arrived too late and is dropped. Returns the peak.
    fn add(&mut self, start: i64, data: &[f32], gain: f32) -> f32 {
        let frames = data.len() / CHANNELS;
        let late = (self.flushed - start).max(0) as usize;
        if late >= frames {
            return 0.0;
        }
        let at = (start + late as i64 - self.flushed) as usize * CHANNELS;
        let data = &data[late * CHANNELS..];
        if self.buf.len() < at + data.len() {
            self.buf.resize(at + data.len(), 0.0);
        }
        let mut peak = 0f32;
        for (dst, &s) in self.buf[at..].iter_mut().zip(data) {
            let s = s * gain;
            peak = peak.max(s.abs());
            *dst += s;
        }
        peak
    }

    /// Sends everything before frame `upto` to ffmpeg (silence where no source
    /// wrote). Returns false once ffmpeg is gone.
    fn flush(&mut self, upto: i64) -> bool {
        let n = (upto - self.flushed).max(0) as usize * CHANNELS;
        if n == 0 {
            return true;
        }
        if self.buf.len() < n {
            self.buf.resize(n, 0.0);
        }
        let mut out = Vec::with_capacity(n * 4);
        for s in self.buf.drain(..n) {
            out.extend_from_slice(&s.clamp(-1.0, 1.0).to_le_bytes());
        }
        self.flushed = upto;
        self.tx.send(out).is_ok()
    }
}

/// One WASAPI capture stream: loopback of the default output, or the default microphone.
struct Source {
    kind: Kind,
    enumerator: IMMDeviceEnumerator,
    device_id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Frame where this source's next packet should start, to keep it contiguous.
    next: Option<i64>,
}

impl Source {
    fn open(kind: Kind) -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(kind.flow(), eConsole)?;
            let device_id = take_pwstr(device.GetId()?);
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            // Ask for exactly what ffmpeg is told to expect; Windows converts
            // from whatever the device uses (channels, rate, depth; mono mics too).
            let fmt = WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: CHANNELS as u16,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * (CHANNELS * 4) as u32,
                nBlockAlign: (CHANNELS * 4) as u16,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            if kind == Kind::Desktop {
                flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
            }
            // 1 s buffer: generous, we poll every 10 ms.
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, &fmt, None)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Source { kind, enumerator, device_id, client, capture, next: None })
        }
    }

    /// Where a packet of `frames` goes, and how many of its first frames to skip.
    fn place(&mut self, at: Option<i64>, frames: usize, now: i64) -> (i64, usize) {
        let (start, skip) = match (self.next, at) {
            // A gap (e.g. loopback after silence): jump to where it really is.
            (Some(n), Some(a)) if a > n + TOLERANCE_FRAMES => (a, 0),
            // Earlier than where we left off (late, or a fast device clock): skip the overlap.
            (Some(n), Some(a)) if a < n - TOLERANCE_FRAMES => (n, ((n - a) as usize).min(frames)),
            (Some(n), _) => (n, 0),
            (None, Some(a)) => (a, 0),
            (None, None) => (now, 0),
        };
        self.next = Some(start + (frames - skip) as i64);
        (start, skip)
    }

    /// Mixes every waiting packet in. Returns the loudest sample.
    fn drain(&mut self, mixer: &mut Mixer, gain: f32) -> windows::core::Result<f32> {
        let mut peak = 0f32;
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                let frames_n = frames as usize;
                let at = (flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0).then(|| mixer.frame_at(qpc as i64));
                let (start, skip) = self.place(at, frames_n, mixer.frame_at(qpc_100ns()));
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                if !silent && skip < frames_n && gain > 0.0 {
                    let bytes = std::slice::from_raw_parts(data, frames_n * CHANNELS * 4);
                    let samples: Vec<f32> = bytes[skip * CHANNELS * 4..]
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .collect();
                    peak = peak.max(mixer.add(start, &samples, gain));
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(peak)
    }

    /// True when Windows' default device for this source is no longer the one we capture.
    fn default_changed(&self) -> bool {
        unsafe {
            match self.enumerator.GetDefaultAudioEndpoint(self.kind.flow(), eConsole).and_then(|d| d.GetId()) {
                Ok(id) => take_pwstr(id) != self.device_id,
                Err(_) => true,
            }
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe { let _ = self.client.Stop(); }
    }
}

unsafe fn take_pwstr(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

/// A source that comes and goes: opened while it's switched on, reopened after
/// errors or when the default device changes, closed (so Windows stops showing
/// the microphone as in use) while it's off.
struct Slot {
    kind: Kind,
    source: Option<Source>,
    retry_at: Instant,
    checked_at: Instant,
    last_err: String,
}

impl Slot {
    fn new(kind: Kind) -> Self {
        let now = Instant::now();
        Slot { kind, source: None, retry_at: now, checked_at: now, last_err: String::new() }
    }

    fn tick(&mut self, mixer: &mut Mixer, controls: &Controls) {
        if !controls.on(self.kind) {
            if self.source.take().is_some() {
                log!("audio: {} off", self.kind.name());
            }
            return;
        }
        if self.source.is_none() {
            if Instant::now() < self.retry_at {
                return;
            }
            match Source::open(self.kind) {
                Ok(s) => {
                    log!("audio: capturing the default {}", if self.kind == Kind::Mic { "microphone" } else { "output device" });
                    self.last_err.clear();
                    self.source = Some(s);
                    self.checked_at = Instant::now();
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg != self.last_err {
                        log!("audio: cannot open {} (recording without it): {}", self.kind.name(), msg);
                        self.last_err = msg;
                    }
                    self.retry_at = Instant::now() + Duration::from_millis(500);
                    return;
                }
            }
        }
        let Some(src) = self.source.as_mut() else { return };
        match src.drain(mixer, controls.gain(self.kind)) {
            Ok(peak) => controls.note_peak(self.kind, peak),
            Err(e) => {
                log!("audio: {} error, reopening: {}", self.kind.name(), e);
                self.source = None;
                self.retry_at = Instant::now() + Duration::from_millis(500);
                return;
            }
        }
        if self.checked_at.elapsed() > Duration::from_secs(2) {
            self.checked_at = Instant::now();
            if src.default_changed() {
                log!("audio: default {} changed, switching", self.kind.name());
                self.source = None;
            }
        }
    }
}

/// Runs until `stop` is set or ffmpeg hangs up. `t0` (QPC, 100ns) is the time of sample 0.
pub fn run(stop: &AtomicBool, t0: i64, tx: Sender<Vec<u8>>, controls: &Controls) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    let mut mixer = Mixer { t0, flushed: 0, buf: Vec::with_capacity(RATE as usize), tx };
    let mut slots = [Slot::new(Kind::Desktop), Slot::new(Kind::Mic)];
    while !stop.load(Ordering::Relaxed) {
        for slot in &mut slots {
            slot.tick(&mut mixer, controls);
        }
        let upto = mixer.frame_at(qpc_100ns() - LATENCY_100NS);
        if !mixer.flush(upto) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixer() -> (Mixer, std::sync::mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Mixer { t0: 0, flushed: 0, buf: Vec::new(), tx }, rx)
    }

    fn samples(bytes: &[u8]) -> Vec<f32> {
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    #[test]
    fn sources_are_summed_with_their_gain_and_gaps_are_silent() {
        let (mut m, rx) = mixer();
        m.add(2, &[0.5, 0.5, 0.5, 0.5], 1.0); // frames 2-3
        m.add(3, &[0.2, 0.2], 0.5); // frame 3, at half volume
        assert!(m.flush(5));
        let got = samples(&rx.recv().unwrap());
        let want = [0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.6, 0.6, 0.0, 0.0];
        assert_eq!(got.len(), want.len());
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6), "{:?}", got);
    }

    #[test]
    fn late_samples_are_dropped_and_the_mix_is_clamped() {
        let (mut m, rx) = mixer();
        assert!(m.flush(2));
        rx.recv().unwrap();
        m.add(1, &[0.9, 0.9, 0.9, 0.9], 1.0); // frame 1 is already gone, frame 2 isn't
        m.add(2, &[0.9, -0.9], 1.0);
        assert!(m.flush(3));
        assert_eq!(samples(&rx.recv().unwrap()), vec![1.0, 0.0]);
    }
}
