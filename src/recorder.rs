//! Runs ffmpeg (screen capture + encoding) and keeps its output in the replay buffer.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

use crate::config::Config;
use crate::ts::Ring;
use crate::util::{file_stamp, qpc_100ns, rtc_us};
use crate::{audio, log};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Extra seconds kept beyond clip_seconds, so a clip can always start on a keyframe.
const SLACK_SECS: u64 = 3;
/// How often ddagrab looks for a new desktop frame (see ffmpeg_args).
const CAPTURE_POLL_HZ: u32 = 500;
/// While recording, audio is stamped this much later than it really is, and
/// save_clip undoes it. ffmpeg pauses any input running >100 ms ahead of the
/// slowest one; audio is inherently ~100 ms behind (WASAPI + AAC framing), so
/// without this the video capture is the one that gets paused and drops frames.
const AUDIO_SHIFT_SECS: &str = "0.15";

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
        s.restart.store(false, Ordering::Relaxed);

        let started = Instant::now();
        let requested = match run_session(s, &cfg) {
            Ok(requested) => requested,
            Err(e) => {
                log!("capture: {}", e);
                false
            }
        };
        if s.stop.load(Ordering::Relaxed) {
            break;
        }
        if requested {
            backoff = Duration::from_secs(1);
            continue;
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

/// Returns Ok(true) if the session ended because a restart/stop was requested.
fn run_session(s: &Shared, cfg: &Config) -> Result<bool, String> {
    let ff = resolve_ffmpeg(cfg, &s.dir)?;
    // Audio sample 0 == this instant. Both clocks are read back to back, which
    // lets ffmpeg map its video timestamps onto the audio timeline.
    let t0_qpc = qpc_100ns();
    let t0_rtc = rtc_us();
    let args = ffmpeg_args(cfg, t0_rtc);

    let errlog = OpenOptions::new()
        .create(true)
        .append(true)
        .open(s.dir.join("ffmpeg.log"))
        .map_err(|e| format!("cannot open ffmpeg.log: {}", e))?;
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
    s.pid.store(child.id(), Ordering::Relaxed);

    let mut stdout = child.stdout.take().unwrap();
    let audio_stop = AtomicBool::new(false);
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
            scope.spawn(move || {
                for chunk in rx {
                    if stdin.write_all(&chunk).is_err() {
                        break;
                    }
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
                break Ok(true);
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let why = last_ffmpeg_error(&s.dir).unwrap_or_else(|| format!("{}, see ffmpeg.log", status));
                    break Err(format!("ffmpeg stopped: {}", why));
                }
                Ok(None) => {}
                Err(e) => break Err(e.to_string()),
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

fn ffmpeg_args(cfg: &Config, t0_rtc_us: i64) -> Vec<String> {
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
    // previous frame (~9% measured at 60 fps on 120 Hz). Polling at 500 Hz and
    // letting fps= pick the freshest frame per slot gave 0 repeats for +0.4% CPU.
    // Capture, frame selection, scaling and RGB->NV12 all stay on the GPU; only
    // the final NV12 frame is copied back for x264.
    let graph = format!(
        "ddagrab=output_idx={}:framerate={}:draw_mouse={},\
         setpts='if(eq(N,0),st(0,(RTCTIME-{})/(TB*1000000)-PTS));PTS+ld(0)',\
         fps={},{},hwdownload,format=nv12[v]",
        cfg.monitor, CAPTURE_POLL_HZ, cfg.draw_mouse as u8, t0_rtc_us, cfg.fps, scale
    );
    let fps = cfg.fps.to_string();
    let rate = format!("{}k", cfg.video_bitrate_kbps);
    a.extend([
        "-filter_complex".into(), graph,
        "-map".into(), "[v]".into(),
        "-fps_mode".into(), "cfr".into(),
        "-r".into(), fps,
        "-c:v".into(), "libx264".into(),
        "-preset".into(), cfg.preset.clone(),
        // Slice threads instead of frame threads: ~half the RAM, lower latency,
        // and measured slightly less CPU than the default here.
        "-tune".into(), "zerolatency".into(),
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
