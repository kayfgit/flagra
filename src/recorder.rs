//! Runs ffmpeg (screen capture + encoding) and keeps its output in the replay buffer.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

use crate::config::{Config, Encoder, GpuPriority};
use crate::ts::Ring;
use crate::util::{file_stamp, qpc_100ns, rtc_us};
use crate::{audio, log};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Extra seconds kept beyond clip_seconds, so a clip can always start on a keyframe.
const SLACK_SECS: u64 = 3;
/// ddagrab looks for a new desktop frame this many times per recorded frame (see ffmpeg_args).
const CAPTURE_POLLS_PER_FRAME: u32 = 2;
/// While recording, audio is stamped this much later than it really is, and
/// save_clip undoes it. ffmpeg pauses any input running >100 ms ahead of the
/// slowest one; audio is inherently ~100 ms behind (WASAPI + AAC framing), so
/// without this the video capture is the one that gets paused and drops frames.
const AUDIO_SHIFT_SECS: &str = "0.15";
/// Raw audio sent to ffmpeg per second (48 kHz, stereo, f32).
const AUDIO_BYTES_PER_SEC: f64 = audio::RATE as f64 * 2.0 * 4.0;
/// How far ffmpeg may fall behind live audio before capture is restarted.
const MAX_LAG_SECS: f64 = 5.0;
/// A GPU encoder that has produced nothing after this long isn't going to.
const GPU_ENCODER_TIMEOUT: Duration = Duration::from_secs(10);

// D3DKMT_SCHEDULINGPRIORITYCLASS values.
const GPU_PRIORITY_HIGH: i32 = 4;
const GPU_PRIORITY_REALTIME: i32 = 5;

#[link(name = "gdi32")]
extern "system" {
    fn D3DKMTSetProcessSchedulingPriorityClass(process: HANDLE, priority: i32) -> i32;
}

struct Shared {
    dir: PathBuf,
    cfg: Arc<Mutex<Config>>,
    ring: Mutex<Ring>,
    stop: AtomicBool,
    restart: AtomicBool,
    /// PID of the running ffmpeg, 0 while capture is down.
    pid: AtomicU32,
    /// Job object that kills ffmpeg if flagra dies, so it never lingers.
    job: isize,
    /// Which audio sources are on and how loud, applied live; and their levels.
    controls: Arc<audio::Controls>,
    /// GPU encoders that failed on this PC; asked again when settings change.
    failed: Mutex<Vec<Encoder>>,
    /// The encoder running now; shown in settings.
    encoder: Mutex<Option<Encoder>>,
}

/// Why a session ended, for the supervisor.
enum End {
    Requested,
    EncoderFailed(Encoder),
}

pub struct Recorder {
    shared: Arc<Shared>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Recorder {
    pub fn start(dir: PathBuf, cfg: Arc<Mutex<Config>>) -> Recorder {
        let job = unsafe { create_kill_job() };
        let keep = Duration::from_secs(cfg.lock().unwrap().clip_seconds as u64 + SLACK_SECS);
        let controls = Arc::new(audio::Controls::new(&cfg.lock().unwrap()));
        let shared = Arc::new(Shared {
            dir,
            cfg,
            ring: Mutex::new(Ring::new(keep)),
            stop: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            pid: AtomicU32::new(0),
            job,
            controls,
            failed: Mutex::new(Vec::new()),
            encoder: Mutex::new(None),
        });
        let s = shared.clone();
        let thread = thread::Builder::new().name("supervisor".into()).spawn(move || supervise(&s)).ok();
        Recorder { shared, thread: Mutex::new(thread) }
    }

    /// Applies audio source switches and volumes without restarting capture.
    pub fn apply_audio(&self, cfg: &Config) {
        self.shared.controls.apply(cfg);
    }

    /// Peak level of desktop audio and microphone since the last call (1.0 = full scale).
    pub fn levels(&self) -> (f32, f32) {
        self.shared.controls.take_peaks()
    }

    /// ffmpeg's PID (0 while capture is down) and how much is buffered.
    pub fn status(&self) -> (u32, Duration) {
        let buffered = self.shared.ring.lock().unwrap().buffered();
        (self.shared.pid.load(Ordering::Relaxed), buffered)
    }

    /// The encoder running now, if capture is up.
    pub fn encoder(&self) -> Option<Encoder> {
        *self.shared.encoder.lock().unwrap()
    }

    /// Applies a new clip length without restarting capture.
    pub fn set_clip_seconds(&self, secs: u32) {
        self.shared.ring.lock().unwrap().set_keep(Duration::from_secs(secs as u64 + SLACK_SECS));
    }

    /// Restarts capture so changed settings take effect.
    pub fn restart(&self) {
        self.shared.restart.store(true, Ordering::Relaxed);
    }

    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }

    /// Writes the last `clip_seconds` to an .mp4 and returns its path. Blocking.
    pub fn save_clip(&self) -> Result<(PathBuf, f64), String> {
        let cfg = self.shared.cfg.lock().unwrap().clone();
        let want = Duration::from_secs(cfg.clip_seconds as u64);
        let (data, buffered) = {
            let ring = self.shared.ring.lock().unwrap();
            (ring.snapshot(want), ring.buffered())
        };
        let data = data.ok_or("nothing recorded yet - is capture running? (see flagra.log)")?;
        let secs = buffered.min(want).as_secs_f64();

        let dir = output_dir(&cfg);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
        let stamp = file_stamp();
        let mut out = dir.join(format!("flagra_{}.mp4", stamp));
        let mut n = 2;
        while out.exists() {
            out = dir.join(format!("flagra_{}_{}.mp4", stamp, n));
            n += 1;
        }

        let ff = resolve_ffmpeg(&cfg, &self.shared.dir)?;
        // The buffer is read twice (video, and audio with its recording shift
        // undone), so it goes through a temp file rather than a pipe.
        let tmp = std::env::temp_dir().join(format!("flagra_{}.ts", std::process::id()));
        fs::write(&tmp, &data).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
        drop(data);
        // Pure remux (no re-encode): costs almost nothing.
        let output = Command::new(&ff)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-f", "mpegts", "-i"])
            .arg(&tmp)
            .args(["-itsoffset", &format!("-{}", AUDIO_SHIFT_SECS), "-f", "mpegts", "-i"])
            .arg(&tmp)
            .args(["-map", "0:v", "-map", "1:a?", "-c", "copy", "-movflags", "+faststart", "-y"])
            .arg(&out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = fs::remove_file(&tmp);
        let output = output.map_err(|e| format!("cannot start ffmpeg: {}", e))?;
        if !output.status.success() || !out.exists() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("ffmpeg failed to write the clip: {}", err.trim()));
        }
        Ok((out, secs))
    }
}

pub fn output_dir(cfg: &Config) -> PathBuf {
    cfg.output_dir.clone().unwrap_or_else(|| crate::videos_dir().join("flagra"))
}

fn supervise(s: &Shared) {
    let mut backoff = Duration::from_secs(1);
    while !s.stop.load(Ordering::Relaxed) {
        let cfg = s.cfg.lock().unwrap().clone();
        *s.ring.lock().unwrap() = Ring::new(Duration::from_secs(cfg.clip_seconds as u64 + SLACK_SECS));
        if s.restart.swap(false, Ordering::Relaxed) {
            s.failed.lock().unwrap().clear();
        }

        let started = Instant::now();
        let encoder = pick_encoder(cfg.encoder, &s.failed.lock().unwrap());
        let ended = run_session(s, &cfg, encoder);
        *s.encoder.lock().unwrap() = None;
        match ended {
            Ok(End::Requested) => {
                backoff = Duration::from_secs(1);
                continue;
            }
            Ok(End::EncoderFailed(e)) => {
                log!("capture: {} doesn't work on this PC, trying the next encoder", e.name());
                s.failed.lock().unwrap().push(e);
                continue;
            }
            Err(e) => log!("capture: {}", e),
        }
        if s.stop.load(Ordering::Relaxed) {
            break;
        }
        // Unexpected exit (crash, display mode change, ...): retry, backing off if it keeps failing fast.
        if started.elapsed() > Duration::from_secs(30) {
            backoff = Duration::from_secs(1);
        }
        log!("capture: restarting in {:?}", backoff);
        let until = Instant::now() + backoff;
        while Instant::now() < until && !s.stop.load(Ordering::Relaxed) && !s.restart.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// The first encoder to try: x264 always works, so it's always the last resort.
fn pick_encoder(wanted: Encoder, failed: &[Encoder]) -> Encoder {
    let order: &[Encoder] = match wanted {
        Encoder::Auto => &[Encoder::Nvenc, Encoder::Amf],
        Encoder::Nvenc => &[Encoder::Nvenc],
        Encoder::Amf => &[Encoder::Amf],
        Encoder::X264 => &[],
    };
    order.iter().copied().find(|e| !failed.contains(e)).unwrap_or(Encoder::X264)
}

fn run_session(s: &Shared, cfg: &Config, encoder: Encoder) -> Result<End, String> {
    let ff = resolve_ffmpeg(cfg, &s.dir)?;
    // Audio sample 0 == this instant. Both clocks are read back to back, which
    // lets ffmpeg map its video timestamps onto the audio timeline.
    let t0_qpc = qpc_100ns();
    let t0_rtc = rtc_us();
    let started = Instant::now();
    let args = ffmpeg_args(cfg, t0_rtc, encoder);

    let errlog = OpenOptions::new()
        .create(true)
        .append(true)
        .open(s.dir.join("ffmpeg.log"))
        .map_err(|e| format!("cannot open ffmpeg.log: {}", e))?;
    let log_start = errlog.metadata().map_or(0, |m| m.len());
    log!("capture: starting {} {}", ff.display(), args.join(" "));
    let mut child = Command::new(&ff)
        .args(&args)
        .stdin(if has_audio(cfg) { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(errlog)
        .creation_flags(CREATE_NO_WINDOW | cfg.priority.creation_flag())
        .spawn()
        .map_err(|e| format!("cannot start {}: {}", ff.display(), e))?;
    unsafe {
        let _ = AssignProcessToJobObject(HANDLE(s.job as _), HANDLE(child.as_raw_handle()));
    }
    let mut gpu_priority_set = false;
    s.pid.store(child.id(), Ordering::Relaxed);
    *s.encoder.lock().unwrap() = Some(encoder);

    let mut stdout = child.stdout.take().unwrap();
    let audio_stop = AtomicBool::new(false);
    // Audio bytes ffmpeg has taken so far. Audio arrives in real time, so this
    // shows whether ffmpeg is keeping up.
    let audio_taken = AtomicU64::new(0);
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => s.ring.lock().unwrap().push(&buf[..n]),
                }
            }
        });

        if let Some(mut stdin) = child.stdin.take() {
            let (tx, rx) = mpsc::channel::<Vec<u8>>();
            let audio_taken = &audio_taken;
            scope.spawn(move || {
                for chunk in rx {
                    if stdin.write_all(&chunk).is_err() {
                        break;
                    }
                    audio_taken.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                }
            });
            let audio_stop = &audio_stop;
            let controls = &*s.controls;
            scope.spawn(move || audio::run(audio_stop, t0_qpc, tx, controls));
        }

        let result = loop {
            if s.stop.load(Ordering::Relaxed) || s.restart.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                break Ok(End::Requested);
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if encoder != Encoder::X264 && encoder_complained(&s.dir, log_start, encoder) {
                        break Ok(End::EncoderFailed(encoder));
                    }
                    let why = last_ffmpeg_error(&s.dir).unwrap_or_else(|| format!("{}, see ffmpeg.log", status));
                    break Err(format!("ffmpeg stopped: {}", why));
                }
                Ok(None) => {}
                Err(e) => break Err(e.to_string()),
            }
            // Only possible once ffmpeg has opened the GPU, a moment after it starts.
            if !gpu_priority_set {
                let process = HANDLE(child.as_raw_handle());
                gpu_priority_set =
                    unsafe { raise_gpu_priority(process, cfg.gpu_priority, started.elapsed() > Duration::from_secs(10)) };
            }
            if encoder != Encoder::X264
                && started.elapsed() > GPU_ENCODER_TIMEOUT
                && s.ring.lock().unwrap().buffered().is_zero()
            {
                let _ = child.kill();
                let _ = child.wait();
                break Ok(End::EncoderFailed(encoder));
            }
            // When ffmpeg can't keep up (the GPU busy with a game), each frame
            // it falls behind makes the next gap longer, and video freezes on
            // one frame while audio piles up here. It never recovers by itself.
            if has_audio(cfg) {
                let lag = started.elapsed().as_secs_f64()
                    - audio_taken.load(Ordering::Relaxed) as f64 / AUDIO_BYTES_PER_SEC;
                if lag > MAX_LAG_SECS {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!("ffmpeg fell {:.0}s behind real time", lag));
                }
            }
            thread::sleep(Duration::from_millis(100));
        };
        // The scope joins the helper threads: audio sees this flag, the stdin
        // writer then runs dry, and the reader hits EOF from the dead process.
        s.pid.store(0, Ordering::Relaxed);
        audio_stop.store(true, Ordering::Relaxed);
        result
    })
}

/// Whether the recording has an audio track (desktop audio, microphone or both).
fn has_audio(cfg: &Config) -> bool {
    cfg.audio || cfg.mic
}

/// The video encoder's own options. Keyframes must stay real IDRs: clips can
/// only start on those. No B-frames, which also keeps GPU encoders' latency low.
fn encoder_args(cfg: &Config, encoder: Encoder) -> Vec<String> {
    let args: &[&str] = match encoder {
        // Slice threads instead of frame threads: ~half the RAM, lower latency,
        // and measured slightly less CPU than the default here.
        Encoder::X264 | Encoder::Auto => &["-c:v", "libx264", "-preset", &cfg.preset, "-tune", "zerolatency"],
        Encoder::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-rc", "cbr", "-bf", "0", "-forced-idr", "1"],
        Encoder::Amf => &["-c:v", "h264_amf", "-usage", "lowlatency", "-quality", "quality", "-rc", "cbr", "-bf", "0", "-forced_idr", "1"],
    };
    args.iter().map(|a| a.to_string()).collect()
}

/// Whether this session's part of ffmpeg.log has the encoder saying something
/// went wrong, rather than capture failing.
fn encoder_complained(dir: &Path, log_start: u64, encoder: Encoder) -> bool {
    let Ok(text) = fs::read(dir.join("ffmpeg.log")) else { return false };
    let session = String::from_utf8_lossy(text.get(log_start as usize..).unwrap_or(&[])).into_owned();
    session.contains(&format!("[h264_{} @", encoder.name()))
}

fn ffmpeg_args(cfg: &Config, t0_rtc_us: i64, encoder: Encoder) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "warning", "-nostats"].map(String::from).to_vec();
    if has_audio(cfg) {
        // Raw PCM needs no probing; without these ffmpeg reads ~5 s of audio
        // before it even starts capturing video.
        a.extend(
            ["-thread_queue_size", "1024", "-probesize", "32", "-analyzeduration", "0",
             "-itsoffset", AUDIO_SHIFT_SECS, "-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "pipe:0"]
            .map(String::from),
        );
    } else {
        a.push("-nostdin".into());
    }

    let scale = match cfg.resolution {
        Some((w, h)) => format!("scale_d3d11=width={}:height={}:format=nv12", w, h),
        None => "scale_d3d11=format=nv12".into(),
    };
    // ddagrab timestamps count from its first frame. On that first frame, setpts
    // measures how long after t0 it arrived (wall clock) and shifts every frame
    // by that amount, putting video on the same timeline as the audio.
    // ddagrab paced at the target fps only waits half a frame for new desktop
    // content, so on high-refresh monitors it often times out and repeats the
    // previous frame (~9% measured at 60 fps on 120 Hz). Polling faster and
    // letting fps= pick the freshest frame per slot fixes that. But every new
    // desktop frame it finds is a full-screen GPU copy plus a cursor draw, and
    // a moving cursor alone counts as one: polling at 500 Hz made ~120+ of them
    // a second in League of Legends to keep 30, each one interrupting the game,
    // which stuttered. Twice the frame rate keeps fps= fed with few of them.
    // Capture, frame selection, scaling and RGB->NV12 all stay on the GPU; only
    // the final NV12 frame is copied back for x264.
    let graph = format!(
        "ddagrab=output_idx={}:framerate={}:draw_mouse={},\
         setpts='if(eq(N,0),st(0,(RTCTIME-{})/(TB*1000000)-PTS));PTS+ld(0)',\
         fps={},{},hwdownload,format=nv12[v]",
        cfg.monitor, cfg.fps * CAPTURE_POLLS_PER_FRAME, cfg.draw_mouse as u8, t0_rtc_us, cfg.fps, scale
    );
    let fps = cfg.fps.to_string();
    let rate = format!("{}k", cfg.video_bitrate_kbps);
    a.extend([
        "-filter_complex".into(), graph,
        "-map".into(), "[v]".into(),
        "-fps_mode".into(), "cfr".into(),
        "-r".into(), fps,
    ]);
    a.extend(encoder_args(cfg, encoder));
    a.extend([
        "-b:v".into(), rate.clone(),
        "-maxrate".into(), rate,
        "-bufsize".into(), format!("{}k", cfg.video_bitrate_kbps * 2),
        // A keyframe every second: clips can start at any second boundary.
        "-g".into(), (cfg.fps * 2).to_string(),
        "-force_key_frames".into(), "expr:gte(t,n_forced*1)".into(),
    ]);
    if has_audio(cfg) {
        a.extend(["-map".into(), "0:a".into(), "-c:a".into(), "aac".into(), "-b:a".into(), format!("{}k", cfg.audio_bitrate_kbps)]);
    }
    a.extend(["-f", "mpegts", "-muxdelay", "0", "-muxpreload", "0", "pipe:1"].map(String::from));
    a
}

/// The reason ffmpeg gave for stopping: the last error line of ffmpeg.log,
/// without its "[filter @ 0x...]" prefix, so the window can show it.
fn last_ffmpeg_error(dir: &Path) -> Option<String> {
    let text = fs::read(dir.join("ffmpeg.log")).ok()?;
    let tail = String::from_utf8_lossy(&text[text.len().saturating_sub(4096)..]).into_owned();
    let lines: Vec<&str> = tail.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let line = lines.iter().rev().find(|l| l.starts_with('[') && l.contains("] ")).or(lines.last())?;
    let msg = line.split_once("] ").map_or(*line, |(_, m)| m);
    Some(msg.chars().take(200).collect())
}

pub fn resolve_ffmpeg(cfg: &Config, app_dir: &Path) -> Result<PathBuf, String> {
    let found = if let Some(p) = &cfg.ffmpeg_path {
        p.exists().then(|| p.clone()).ok_or_else(|| format!("ffmpeg_path {} does not exist", p.display()))?
    } else {
        // Next to flagra.exe, then the ffmpeg\ folder flagra comes with, then PATH.
        [app_dir.join("ffmpeg.exe"), app_dir.join("ffmpeg").join("ffmpeg.exe")]
            .into_iter()
            .find(|p| p.exists())
            .or_else(|| {
                std::env::var_os("PATH")
                    .and_then(|path| std::env::split_paths(&path).map(|d| d.join("ffmpeg.exe")).find(|p| p.exists()))
            })
            .ok_or("ffmpeg.exe not found: reinstall flagra (it comes with one), or set ffmpeg_path")?
    };
    Ok(unshim(found))
}

/// Scoop installs a launcher "shim" exe; killing it would leave the real ffmpeg
/// running, so run the target the shim points at instead.
fn unshim(exe: PathBuf) -> PathBuf {
    let Ok(text) = fs::read_to_string(exe.with_extension("shim")) else { return exe };
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "path")
        .map(|(_, v)| PathBuf::from(v.trim().trim_matches('"')))
        .filter(|p| p.exists())
        .unwrap_or(exe)
}

/// Lets ffmpeg's GPU work (grabbing, scaling, reading frames back) cut ahead of
/// a game's. It is a sliver of work, but at normal priority it waits behind
/// whole game frames: measured at 0.4x real time with a game holding a GT 1030
/// at 90%, and back to real time at REALTIME. REALTIME preempts the game in
/// the middle of its frames though, which made League of Legends stutter, so
/// high is the default. No admin rights needed, but it fails until the
/// process has opened the GPU. Returns true once done, or once `give_up` and
/// it still fails (logged).
unsafe fn raise_gpu_priority(process: HANDLE, wanted: GpuPriority, give_up: bool) -> bool {
    let tries: &[(&str, i32)] = match wanted {
        GpuPriority::Normal => return true,
        GpuPriority::High => &[("high", GPU_PRIORITY_HIGH)],
        GpuPriority::Realtime => &[("realtime", GPU_PRIORITY_REALTIME), ("high", GPU_PRIORITY_HIGH)],
    };
    let mut status = 0;
    for &(name, prio) in tries {
        status = D3DKMTSetProcessSchedulingPriorityClass(process, prio);
        if status == 0 {
            log!("capture: GPU priority {}", name);
            return true;
        }
    }
    if give_up {
        log!("capture: cannot raise GPU priority (NTSTATUS {:#x})", status as u32);
    }
    give_up
}

unsafe fn create_kill_job() -> isize {
    let Ok(job) = CreateJobObjectW(None, None) else { return 0 };
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let _ = SetInformationJobObject(
        job,
        JobObjectExtendedLimitInformation,
        &info as *const _ as *const _,
        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
    );
    job.0 as isize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoders_fall_back_to_x264() {
        assert_eq!(pick_encoder(Encoder::Auto, &[]), Encoder::Nvenc);
        assert_eq!(pick_encoder(Encoder::Auto, &[Encoder::Nvenc]), Encoder::Amf);
        assert_eq!(pick_encoder(Encoder::Auto, &[Encoder::Nvenc, Encoder::Amf]), Encoder::X264);
        assert_eq!(pick_encoder(Encoder::Amf, &[Encoder::Amf]), Encoder::X264);
        assert_eq!(pick_encoder(Encoder::X264, &[]), Encoder::X264);
    }

    #[test]
    fn last_ffmpeg_error_gives_the_reason_without_the_prefix() {
        let dir = std::env::temp_dir().join(format!("flagra-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("ffmpeg.log"),
            "[aist#0:0/pcm_f32le @ 000001] Guessed Channel Layout: stereo\n\
             [fc#0 @ 000001745f257fc0] Error applying option 'w' to filter 'scale_d3d11': Option not found\n\
             Error : Option not found\n\n",
        )
        .unwrap();
        let why = last_ffmpeg_error(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(why.as_deref(), Some("Error applying option 'w' to filter 'scale_d3d11': Option not found"));
    }
}
