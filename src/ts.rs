//! In-memory replay buffer over ffmpeg's MPEG-TS output.
//!
//! The stream is split into chunks that each start at a video keyframe, so any
//! suffix of chunks (prefixed with the latest PAT/PMT) is a valid, playable TS file.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const PKT: usize = 188;

struct Chunk {
    start: Instant,
    data: Vec<u8>,
}

pub struct Ring {
    keep: Duration,
    chunks: VecDeque<Chunk>,
    carry: Vec<u8>,
    pat: Option<[u8; PKT]>,
    pmt: Option<[u8; PKT]>,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
}

impl Ring {
    pub fn new(keep: Duration) -> Self {
        Ring {
            keep,
            chunks: VecDeque::new(),
            carry: Vec::with_capacity(PKT),
            pat: None,
            pmt: None,
            pmt_pid: None,
            video_pid: None,
        }
    }

    /// Feeds raw bytes from ffmpeg's stdout; they need not be packet-aligned.
    pub fn push(&mut self, mut buf: &[u8]) {
        if !self.carry.is_empty() {
            let need = PKT - self.carry.len();
            let take = need.min(buf.len());
            self.carry.extend_from_slice(&buf[..take]);
            buf = &buf[take..];
            if self.carry.len() < PKT {
                return;
            }
            let pkt: [u8; PKT] = self.carry[..].try_into().unwrap();
            self.carry.clear();
            self.packet(&pkt);
        }
        while !buf.is_empty() {
            if buf[0] != 0x47 {
                // Lost sync: skip to the next sync byte.
                match buf.iter().position(|&b| b == 0x47) {
                    Some(i) => buf = &buf[i..],
                    None => return,
                }
                continue;
            }
            if buf.len() < PKT {
                self.carry.extend_from_slice(buf);
                return;
            }
            let pkt: [u8; PKT] = buf[..PKT].try_into().unwrap();
            self.packet(&pkt);
            buf = &buf[PKT..];
        }
    }

    fn packet(&mut self, p: &[u8; PKT]) {
        let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
        let pusi = p[1] & 0x40 != 0;
        let has_af = p[3] & 0x20 != 0;

        if pid == 0 && pusi {
            if let Some(pmt_pid) = parse_pat(p) {
                self.pat = Some(*p);
                self.pmt_pid = Some(pmt_pid);
            }
        } else if Some(pid) == self.pmt_pid && pusi {
            if let Some(vpid) = parse_pmt(p) {
                self.pmt = Some(*p);
                self.video_pid = Some(vpid);
            }
        }

        let random_access = has_af && p[4] > 0 && p[5] & 0x40 != 0;
        if pusi && random_access && Some(pid) == self.video_pid {
            let now = Instant::now();
            let cap = self.chunks.back().map_or(1 << 20, |c| c.data.len() + c.data.len() / 4);
            self.chunks.push_back(Chunk { start: now, data: Vec::with_capacity(cap) });
            // Drop chunks that lie entirely outside the window.
            while self.chunks.len() > 1 && now.duration_since(self.chunks[1].start) >= self.keep {
                self.chunks.pop_front();
            }
        }
        // Anything before the first keyframe is unusable and simply dropped.
        if let Some(last) = self.chunks.back_mut() {
            last.data.extend_from_slice(p);
        }
    }

    /// Changes how much is kept; takes effect as new keyframes arrive.
    pub fn set_keep(&mut self, keep: Duration) {
        self.keep = keep;
    }

    /// Seconds of video currently buffered.
    pub fn buffered(&self) -> Duration {
        self.chunks.front().map_or(Duration::ZERO, |c| c.start.elapsed())
    }

    /// A standalone TS stream covering at least the last `secs` (or all buffered data).
    pub fn snapshot(&self, secs: Duration) -> Option<Vec<u8>> {
        let (pat, pmt) = (self.pat?, self.pmt?);
        if self.chunks.is_empty() {
            return None;
        }
        let now = Instant::now();
        let first = (0..self.chunks.len())
            .rev()
            .find(|&i| now.duration_since(self.chunks[i].start) >= secs)
            .unwrap_or(0);
        let total: usize = self.chunks.iter().skip(first).map(|c| c.data.len()).sum();
        let mut out = Vec::with_capacity(total + 2 * PKT);
        out.extend_from_slice(&pat);
        out.extend_from_slice(&pmt);
        for c in self.chunks.iter().skip(first) {
            out.extend_from_slice(&c.data);
        }
        Some(out)
    }
}

/// Start of the PSI section inside a packet that carries one (pointer field honoured).
fn section(p: &[u8; PKT]) -> Option<&[u8]> {
    let mut off = 4;
    if p[3] & 0x20 != 0 {
        off += 1 + p[4] as usize;
    }
    let pointer = *p.get(off)? as usize;
    p.get(off + 1 + pointer..)
}

fn parse_pat(p: &[u8; PKT]) -> Option<u16> {
    let s = section(p)?;
    if *s.first()? != 0x00 {
        return None;
    }
    let len = ((s.get(1)? & 0x0f) as usize) << 8 | *s.get(2)? as usize;
    let end = (3 + len).checked_sub(4)?.min(s.len());
    let mut i = 8;
    while i + 4 <= end {
        let program = (s[i] as u16) << 8 | s[i + 1] as u16;
        let pid = ((s[i + 2] & 0x1f) as u16) << 8 | s[i + 3] as u16;
        if program != 0 {
            return Some(pid);
        }
        i += 4;
    }
    None
}

fn parse_pmt(p: &[u8; PKT]) -> Option<u16> {
    let s = section(p)?;
    if *s.first()? != 0x02 {
        return None;
    }
    let len = ((s.get(1)? & 0x0f) as usize) << 8 | *s.get(2)? as usize;
    let end = (3 + len).checked_sub(4)?.min(s.len());
    let info_len = ((s.get(10)? & 0x0f) as usize) << 8 | *s.get(11)? as usize;
    let mut i = 12 + info_len;
    while i + 5 <= end {
        let stream_type = s[i];
        let pid = ((s[i + 1] & 0x1f) as u16) << 8 | s[i + 2] as u16;
        let es_len = ((s[i + 3] & 0x0f) as usize) << 8 | s[i + 4] as usize;
        // MPEG-2, H.264, HEVC video
        if matches!(stream_type, 0x02 | 0x1b | 0x24) {
            return Some(pid);
        }
        i += 5 + es_len;
    }
    None
}
