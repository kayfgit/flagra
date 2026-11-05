// Clip sizes, bitrates, quality profiles and how they're shown. Kept free of
// the DOM so tests can import it.

// Quality profiles: a set of ordinary settings, so anything can still be tuned
// after picking one. What's set simply is the profile; change a value and it's custom.
// high matches ShadowPlay's High quality; high and ultra are meant for a GPU encoder.
export const PROFILES = {
  potato: {
    name: 'potato',
    heavy: false,
    values: { resolution: '1280x720', fps: 30, video_bitrate_kbps: 4000, preset: 'ultrafast' },
  },
  low: {
    name: 'low end',
    heavy: false,
    values: { resolution: '1280x720', fps: 60, video_bitrate_kbps: 8000, preset: 'ultrafast' },
  },
  mid: {
    name: 'mid range',
    heavy: false,
    values: { resolution: '1920x1080', fps: 60, video_bitrate_kbps: 15000, preset: 'superfast' },
  },
  high: {
    name: 'high end',
    heavy: true,
    values: { resolution: 'native', fps: 60, video_bitrate_kbps: 50000, preset: 'veryfast' },
  },
  ultra: {
    name: 'ultra',
    heavy: true,
    values: { resolution: 'native', fps: 60, video_bitrate_kbps: 100000, preset: 'veryfast' },
  },
};

export const mbps = kbps => (kbps >= 1000 ? `${+(kbps / 1000).toFixed(1)} Mbps` : `${kbps} kbps`);

export function clock(sec) {
  if (sec == null || !isFinite(sec)) return '–:––';
  const s = Math.max(0, Math.round(sec));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

export function size(bytes) {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(2)} GB` : `${(bytes / 1e6).toFixed(1)} MB`;
}

export const profileOf = c => Object.values(PROFILES).find(p => Object.entries(p.values).every(([k, v]) => c[k] === v));

export const audioKbps = c => (c.audio || c.mic ? c.audio_bitrate_kbps : 0);

/** Roughly how big a clip is and how much RAM the replay buffer takes. */
export function footprint(c) {
  const kbps = c.video_bitrate_kbps + audioKbps(c);
  const mb = s => Math.round(((kbps / 8) * s) / 1000);
  return { clip: mb(c.clip_seconds), ram: mb(c.clip_seconds + 3) };
}

// A clip can hold more than its length times the bitrate: it starts on a keyframe,
// up to 1 s early, and the encoder may spend up to 2 s worth of saved-up bits when
// a calm stretch turns busy (-bufsize). 9.7 Mbps measured 21 MB in a 15 s clip.
export const SIZE_SLACK_SECS = 3;
export const SIZE_MARGIN = 0.98; // the mp4's own overhead

/** The largest a clip can get, in MB. */
export const maxClipMb = c =>
  Math.ceil(((c.video_bitrate_kbps + audioKbps(c)) * (c.clip_seconds + SIZE_SLACK_SECS)) / 8000 / SIZE_MARGIN);

/** The video bitrate that keeps every clip under `mb`, or 0 if none can. */
export function bitrateForSize(c, mb) {
  const total = (mb * 8000 * SIZE_MARGIN) / (c.clip_seconds + SIZE_SLACK_SECS);
  const kbps = Math.floor((total - audioKbps(c)) / 100) * 100;
  return kbps < 100 ? 0 : Math.min(kbps, 200000);
}
