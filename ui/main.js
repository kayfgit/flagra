'use strict';

const $ = id => document.getElementById(id);
const el = {
  rec: $('rec'), usage: $('usage'),
  player: $('player'), video: $('video'), clock: $('clock'), playname: $('playname'),
  list: $('list'), help: $('help'),
  live: $('live'), input: $('input'), preview: $('preview'),
  mode: $('mode'), info: $('info'), hints: $('hints'),
};

// ---------------------------------------------------------------- backend

const api = window.__TAURI__ ? tauriApi() : browserApi();

function tauriApi() {
  const { invoke, convertFileSrc } = window.__TAURI__.core;
  return {
    ready: () => invoke('ready'),
    close: () => invoke('close'),
    quit: () => invoke('quit'),
    minimize: () => invoke('minimize'),
    moveWindow: (dx, dy) => invoke('move_window', { dx, dy }),
    resizeWindow: (dw, dh) => invoke('resize_window', { dw, dh }),
    savePlacement: what => invoke('save_placement', { what }),
    resetPlacement: what => invoke('reset_placement', { what }),
    fullscreen: () => invoke('toggle_fullscreen'),
    load: () => invoke('load'),
    status: () => invoke('status'),
    levels: () => invoke('levels'),
    testPopup: () => invoke('test_popup'),
    set: (key, value) => invoke('set_setting', { key, value: String(value) }),
    clips: () => invoke('list_clips'),
    saveClip: () => invoke('save_clip'),
    remove: path => invoke('delete_clip', { path }),
    rename: (path, name) => invoke('rename_clip', { path, name }),
    copy: path => invoke('copy_clip', { path }),
    reveal: path => invoke('reveal', { path }),
    play: path => invoke('play', { path }),
    open: what => invoke('open_special', { what }),
    start: () => invoke('start_recorder'),
    stop: () => invoke('stop_recorder'),
    src: path => convertFileSrc(path),
  };
}

// Lets the UI run in a plain browser (open ui/index.html) to iterate on it without rebuilding.
function browserApi() {
  const now = Date.now();
  const fake = (n, ago, dur) => ({
    name: `flagra_2026-10-27_${n}.mp4`, path: `C:\\clips\\flagra_${n}.mp4`,
    size: Math.round(dur * 1.27e6), modified: now - ago * 1000, duration: dur,
  });
  let clips = [fake('21-14-02', 40, 20.2), fake('20-56-28', 1100, 10.03), fake('20-41-20', 2500, 8.8),
    fake('2026-10-26_23-02-11', 86400 + 3000, 20.5), fake('2026-10-20_18-40-55', 7 * 86400, 31.1)];
  let config = {
    hotkey: 'Alt+F10', open_hotkey: 'Alt+F9', clip_seconds: 20, fps: 60, resolution: 'native', video_bitrate_kbps: 10000,
    preset: 'ultrafast', audio: true, desktop_volume: 100, mic: false, mic_volume: 100, audio_bitrate_kbps: 160, monitor: 0, draw_mouse: true,
    priority: 'below_normal', output_dir: 'C:\\Users\\you\\Videos\\flagra', output_dir_custom: false,
    ffmpeg_path: null, beep: true, popup: true, popup_position: 'top-right', notify: false, start_with_windows: true, keep_window_ready: false,
  };
  const capture = new Set(['fps', 'resolution', 'video_bitrate_kbps', 'preset', 'audio', 'audio_bitrate_kbps',
    'monitor', 'draw_mouse', 'priority', 'ffmpeg_path']);
  return {
    ready: async () => {}, close: async () => {}, quit: async () => {}, minimize: async () => {},
    moveWindow: async () => {}, resizeWindow: async () => {}, savePlacement: async () => {}, resetPlacement: async () => {},
    fullscreen: async () => !document.fullscreenElement,
    load: async () => ({ config, warnings: [], cpus: 6 }),
    status: async () => ({ running: true, recording: true, buffered: 22.8, cpu: 9 + Math.random() * 3, mem_mb: 113 }),
    testPopup: async () => {},
    levels: async () => [Math.random() * 0.6, config.mic ? Math.random() * 1.1 : 0],
    set: async (key, value) => {
      const bool = typeof config[key] === 'boolean';
      config = { ...config, [key]: bool ? value === 'true' : /^\d+$/.test(value) ? Number(value) : value };
      return { config, restarted: capture.has(key) };
    },
    clips: async () => clips,
    saveClip: async () => { clips = [fake(`21-${Date.now() % 60}`, 0, 20), ...clips]; },
    remove: async path => { clips = clips.filter(c => c.path !== path); },
    rename: async (path, name) => {
      const p = `C:\\clips\\${name}.mp4`;
      clips = clips.map(c => (c.path === path ? { ...c, name: `${name}.mp4`, path: p } : c));
      return p;
    },
    copy: async () => {}, reveal: async () => {}, play: async () => {}, open: async () => {},
    start: async () => {}, stop: async () => {},
    src: () => '',
  };
}

// ---------------------------------------------------------------- state

let config = null;
let clips = [];
let clipsSig = '';
let view = 'clips';              // 'clips' | 'settings'
const sel = { clips: 0, settings: 0 };
let playing = null;              // path of the clip in the player
let commandMode = false;
let hotkeyMode = null;           // which setting a combo is being captured for: 'hotkey' | 'open_hotkey'
let windowMode = null;           // 'move' | 'resize' while placing the window with hjkl
let fullscreen = false;
let helpOpen = false;
let deleteArmed = 0;             // time of the first d of dd
let awaitingClip = 0;            // time we asked the recorder for a clip
let lastStatus = null;

const CAPTURE_KEYS = new Set(['fps', 'resolution', 'video_bitrate_kbps', 'preset',
  'audio_bitrate_kbps', 'monitor', 'draw_mouse', 'priority', 'ffmpeg_path']);
const CORNERS = ['top-left', 'top-right', 'bottom-left', 'bottom-right'];
const PRESETS = ['ultrafast', 'superfast', 'veryfast', 'faster', 'fast', 'medium'];
const PRIORITIES = { idle: 'idle', below: 'below_normal', below_normal: 'below_normal', normal: 'normal',
  above: 'above_normal', above_normal: 'above_normal' };
const RESOLUTIONS = { native: 'native', '480p': '854x480', '540p': '960x540', '720p': '1280x720',
  '900p': '1600x900', '1080p': '1920x1080', '1440p': '2560x1440', '2160p': '3840x2160', '4k': '3840x2160' };

// ---------------------------------------------------------------- formatting

const errText = err => String(err?.message ?? err);
const stem = name => name.replace(/\.mp4$/i, '');
const nativeRes = () => `${Math.round(screen.width * devicePixelRatio)}x${Math.round(screen.height * devicePixelRatio)}`;
const resLabel = r => (r === 'native' ? nativeRes() : r);
const shortRes = r => {
  const [w, h] = resLabel(r).split('x').map(Number);
  return h && w / h > 1.7 && w / h < 1.8 ? `${h}p` : resLabel(r);
};
const mbps = kbps => (kbps >= 1000 ? `${+(kbps / 1000).toFixed(1)} Mbps` : `${kbps} kbps`);
const priorityLabel = p => p.replace('_', ' ');

function clock(sec) {
  if (sec == null || !isFinite(sec)) return '–:––';
  const s = Math.max(0, Math.round(sec));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

function size(bytes) {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(2)} GB` : `${(bytes / 1e6).toFixed(1)} MB`;
}

function when(ms) {
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return `today ${time}`;
  if (d.toDateString() === yesterday.toDateString()) return `yesterday ${time}`;
  return `${d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })} ${time}`;
}

/** Roughly how big a clip is and how much RAM the replay buffer takes. */
function footprint(c) {
  const kbps = c.video_bitrate_kbps + (c.audio ? c.audio_bitrate_kbps : 0);
  const mb = s => Math.round(kbps / 8 * s / 1000);
  return { clip: mb(c.clip_seconds), ram: mb(c.clip_seconds + 3) };
}

function h(tag, cls, ...children) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  node.append(...children.filter(c => c != null && c !== false));
  return node;
}

// ---------------------------------------------------------------- settings rows

const SETTINGS = [
  { key: 'hotkey', label: 'clip hotkey', cmd: 'hotkey', value: c => c.hotkey },
  { key: 'open_hotkey', label: 'open hotkey', cmd: 'openkey', value: c => c.open_hotkey ?? ['off', ' · :openkey to set one'] },
  { key: 'clip_seconds', label: 'clip length', cmd: 'len', value: c => `${c.clip_seconds} s` },
  { key: 'fps', label: 'frame rate', cmd: 'fps', value: c => `${c.fps} fps` },
  {
    key: 'resolution', label: 'resolution', cmd: 'res',
    value: c => (c.resolution === 'native' ? ['native', ` (${nativeRes()})`] : c.resolution),
  },
  {
    key: 'video_bitrate_kbps', label: 'video bitrate', cmd: 'bitrate',
    value: c => {
      const f = footprint(c);
      return [mbps(c.video_bitrate_kbps), ` · ~${f.clip} MB per clip, ~${f.ram} MB of RAM`];
    },
  },
  { key: 'preset', label: 'x264 preset', cmd: 'preset', value: c => c.preset },
  // The mixer: both sources go into one track. h l change the volume live.
  {
    key: 'audio', label: 'desktop audio', cmd: 'desktop', bool: true, volume: 'desktop_volume', meter: 'desktop',
    value: c => (c.audio ? ['on', ` · ${c.desktop_volume}%`] : 'off'),
  },
  {
    key: 'mic', label: 'microphone', cmd: 'mic', bool: true, volume: 'mic_volume', meter: 'mic',
    value: c => (c.mic ? ['on', ` · ${c.mic_volume}%`] : ['off', ' · ↵ to record your voice too']),
  },
  {
    key: 'audio_bitrate_kbps', label: 'audio bitrate', cmd: 'abitrate',
    value: c => [`${c.audio_bitrate_kbps} kbps`, c.audio || c.mic ? ' · AAC' : ' · no audio is recorded'],
  },
  { key: 'monitor', label: 'monitor', cmd: 'monitor', value: c => String(c.monitor) },
  { key: 'draw_mouse', label: 'mouse cursor', cmd: 'mouse', bool: true, value: c => (c.draw_mouse ? 'shown' : 'hidden') },
  { key: 'priority', label: 'encoder priority', cmd: 'priority', value: c => priorityLabel(c.priority) },
  {
    key: 'output_dir', label: 'clips folder', cmd: 'dir',
    value: c => [c.output_dir, c.output_dir_custom ? '' : ' (default)'],
  },
  { key: 'beep', label: 'beep on save', cmd: 'beep', bool: true, value: c => (c.beep ? 'on' : 'off') },
  {
    key: 'popup', label: 'popup on save', cmd: 'popup', bool: true,
    value: c => (c.popup ? ['on', ` · ${c.popup_position.replace('-', ' ')} · :popup test shows it`] : 'off'),
  },
  { key: 'notify', label: 'notification', cmd: 'notify', bool: true, value: c => (c.notify ? ['on', ' · a Windows one'] : 'off') },
  {
    key: 'start_with_windows', label: 'start with Windows', cmd: 'autostart', bool: true,
    value: c => (c.start_with_windows ? ['on', ' · records from the moment you log in'] : 'off'),
  },
  {
    key: 'keep_window_ready', label: 'open instantly', cmd: 'keepready', bool: true,
    value: c => (c.keep_window_ready
      ? ['on', ' · kept ready in the background (WebView2 in low-memory mode)']
      : ['off', ' · opens in about half a second, uses no memory while closed']),
  },
  {
    key: 'ffmpeg_path', label: 'ffmpeg', cmd: 'ffmpeg',
    value: c => c.ffmpeg_path ?? ['auto', ' (the one that comes with flagra)'],
  },
];

/** What the command line is prefilled with when changing a setting from the list. */
function settingArg(s) {
  const c = config;
  switch (s.key) {
    case 'resolution': return c.resolution === 'native' ? 'native' : c.resolution;
    case 'video_bitrate_kbps': return `${c.video_bitrate_kbps / 1000}m`;
    case 'output_dir': return c.output_dir_custom ? c.output_dir : '';
    case 'ffmpeg_path': return c.ffmpeg_path ?? '';
    default: return String(c[s.key]);
  }
}

// ---------------------------------------------------------------- rendering

function render() {
  document.body.classList.toggle('clips', view === 'clips');
  document.body.classList.toggle('settings', view === 'settings');
  el.list.hidden = helpOpen;
  el.help.hidden = !helpOpen;
  if (!helpOpen) el.list.replaceChildren(...(view === 'clips' ? clipRows() : settingRows()));
  markSelection();
  setMode();
  setHints();
}

function clipRows() {
  if (!clips.length) {
    const hk = h('b', null, config?.hotkey ?? 'the hotkey');
    return [h('li', 'empty', 'No clips yet. Press ', hk, ' in a game (or ', h('b', null, 'c'),
      ' here) to save the last ', `${config?.clip_seconds ?? 20} seconds.`)];
  }
  return clips.map((c, i) => {
    const li = h('li', 'row' + (c.path === playing ? ' playing' : ''),
      h('span', 'n', `[${i + 1}]`),
      h('span', 'name', stem(c.name)),
      h('span', 'dur', clock(c.duration)),
      h('span', 'size', size(c.size)),
      h('span', 'when', when(c.modified)));
    li.dataset.i = i;
    return li;
  });
}

function settingRows() {
  if (!config) return [h('li', 'empty', 'Loading settings…')];
  return SETTINGS.map((s, i) => {
    const v = s.value(config);
    const [main, note] = Array.isArray(v) ? v : [v, ''];
    const li = h('li', 'row',
      h('span', 'label', s.label),
      h('span', 'value', main, note && h('span', s.meter ? 'note vol' : 'note', note),
        s.meter && config[s.key] && h('span', 'meter', h('span'))),
      h('span', 'cmd', s.volume ? `↵ on/off · h l volume · :${s.cmd}`
        : s.bool ? `↵ toggles · :${s.cmd}` : s.key.endsWith('hotkey') ? '↵ press a new combo' : `:${s.cmd}`));
    const meter = li.querySelector('.meter');
    if (meter) meter.dataset.meter = s.meter;
    li.dataset.i = i;
    return li;
  });
}

function count() {
  return view === 'clips' ? clips.length : SETTINGS.length;
}

function markSelection() {
  const i = sel[view];
  for (const row of el.list.children) {
    const on = Number(row.dataset.i) === i;
    row.classList.toggle('sel', on);
    if (on) row.scrollIntoView({ block: 'nearest' });
  }
}

function move(delta, absolute = false) {
  const n = count();
  if (!n) return;
  sel[view] = absolute ? (delta < 0 ? n - 1 : 0) : Math.min(n - 1, Math.max(0, sel[view] + delta));
  deleteArmed = 0;
  markSelection();
}

function setMode() {
  el.mode.textContent = windowMode ? windowMode.toUpperCase() : hotkeyMode ? 'HOTKEY' : commandMode ? 'COMMAND' : helpOpen ? 'HELP'
    : view === 'settings' ? 'SETTINGS' : playing ? 'PLAYING' : 'CLIPS';
}

const HINTS = {
  clips: [['↵', 'play'], ['y', 'copy'], ['r', 'rename'], ['dd', 'delete'], ['f', 'folder'], ['c', 'clip now'],
    ['tab', 'settings'], [':', 'command'], ['?', 'help']],
  player: [['space', 'pause'], ['h l', '5 s'], [', .', 'frame'], ['y', 'copy'], ['o', 'player app'], ['esc', 'close']],
  settings: [['↵', 'change'], ['h l', 'volume'], ['tab', 'clips'], [':', 'command'], ['?', 'help'], ['esc', 'close']],
  command: [['↵', 'run'], ['tab', 'complete'], ['esc', 'cancel']],
  hotkey: [['press', 'the new combo'], ['esc', 'cancel']],
  help: [['j k', 'scroll'], ['g G', 'top / bottom'], ['esc', 'back']],
  window: [['h j k l', 'step'], ['shift', 'fine'], ['ctrl', 'big'], ['↵ esc', 'done']],
};

function setHints() {
  const which = windowMode ? 'window' : hotkeyMode ? 'hotkey' : commandMode ? 'command' : helpOpen ? 'help'
    : view === 'settings' ? 'settings' : playing ? 'player' : 'clips';
  const hints = [...HINTS[which]];
  const draw = () => el.hints.replaceChildren(...hints.map(([k, label]) => h('span', null, h('b', null, k), label)));
  draw();
  // Drop hints from the end rather than squeezing the message out.
  const message = Math.min(480, Math.max(220, el.info.textContent ? el.info.scrollWidth : 0));
  const room = () => el.hints.parentElement.clientWidth - el.mode.offsetWidth - message;
  while (hints.length > 1 && el.hints.offsetWidth > room()) {
    hints.pop();
    draw();
  }
}

let infoTimer = 0;
function say(text, kind = '') {
  clearTimeout(infoTimer);
  el.info.textContent = text;
  el.info.className = kind;
  setHints();
  if (kind === 'done') infoTimer = setTimeout(() => say(''), 6000);
}

function renderStatus(s) {
  lastStatus = s;
  const sep = () => h('span', 'sep', '·');
  el.rec.classList.toggle('off', !s.recording);
  if (!s.running) {
    el.rec.replaceChildren(h('span', 'dot', '○'), h('b', null, 'not running'), sep(), ':start begins recording');
  } else if (!s.recording) {
    el.rec.replaceChildren(h('span', 'dot', '○'), h('b', null, 'capture down'), sep(),
      s.error ?? 'starting…');
  } else if (config) {
    const audio = config.audio ? 'audio' : 'no audio';
    el.rec.replaceChildren(h('span', 'dot', '●'), h('b', null, 'REC'), sep(),
      `${shortRes(config.resolution)}${config.fps} · ${mbps(config.video_bitrate_kbps)} · ${audio}`, sep(),
      `${Math.min(s.buffered, config.clip_seconds).toFixed(0)} of ${config.clip_seconds} s buffered`);
  }
  el.usage.replaceChildren(...(s.recording && s.cpu != null
    ? ['ffmpeg ', h('b', null, `${s.cpu.toFixed(0)}%`), ' CPU', sep(), h('b', null, `${Math.round(s.mem_mb ?? 0)} MB`)]
    : []));
}

// ---------------------------------------------------------------- data

async function refreshStatus() {
  try { renderStatus(await api.status()); } catch {}
}

async function refreshClips() {
  let list;
  try { list = await api.clips(); } catch (err) { say(errText(err), 'error'); return; }
  const sig = list.map(c => c.path + c.modified).join('|');
  if (sig === clipsSig) return;
  const selected = clips[sel.clips]?.path;
  const fresh = list[0] && clipsSig && !clips.some(c => c.path === list[0].path) ? list[0] : null;
  clips = list;
  clipsSig = sig;
  if (fresh && (awaitingClip || sel.clips === 0)) {
    sel.clips = 0;
    say(`saved ${stem(fresh.name)} (${clock(fresh.duration)})`, 'done');
    awaitingClip = 0;
  } else {
    const i = clips.findIndex(c => c.path === selected);
    sel.clips = i >= 0 ? i : Math.max(0, Math.min(sel.clips, clips.length - 1));
  }
  if (playing && !clips.some(c => c.path === playing)) stopPlayer();
  if (view === 'clips' && !helpOpen) render();
}

const current = () => clips[sel.clips];

// ---------------------------------------------------------------- clip actions

async function saveClip() {
  try {
    await api.saveClip();
    awaitingClip = Date.now();
    say(`saving the last ${config.clip_seconds} s…`);
    setTimeout(refreshClips, 700);
  } catch (err) {
    say(errText(err), 'error');
  }
}

async function copyClip(c = current()) {
  if (!c) return;
  try {
    await api.copy(c.path);
    say(`${stem(c.name)} copied · paste it into Discord, a chat or a folder`, 'done');
  } catch (err) { say(errText(err), 'error'); }
}

async function deleteClip(c = current()) {
  if (!c) return;
  try {
    if (c.path === playing) stopPlayer();
    await api.remove(c.path);
    say(`${stem(c.name)} moved to the Recycle Bin`, 'done');
    await refreshClips();
  } catch (err) { say(errText(err), 'error'); }
}

async function renameClip(c, name) {
  try {
    const path = await api.rename(c.path, name);
    if (playing === c.path) playing = path;
    say(`renamed to ${stem(name)}`, 'done');
    await refreshClips();
    const i = clips.findIndex(x => x.path === path);
    if (i >= 0) { sel.clips = i; markSelection(); }
  } catch (err) { say(errText(err), 'error'); }
}

function playClip(c) {
  if (!c) return;
  if (playing === c.path) { togglePause(); return; }
  const src = api.src(c.path);
  if (!src) { say('playback needs the app (o opens it in your player)', 'error'); return; }
  playing = c.path;
  el.player.hidden = false;
  el.playname.textContent = stem(c.name);
  el.video.src = src;
  el.video.play().catch(() => {});
  render();
}

function stopPlayer() {
  if (!playing) return;
  el.video.pause();
  // Dropping the source frees the decoder and its memory right away.
  el.video.removeAttribute('src');
  el.video.load();
  el.player.hidden = true;
  playing = null;
  render();
}

function togglePause() {
  if (el.video.paused) el.video.play().catch(() => {}); else el.video.pause();
}

function seek(delta) {
  const v = el.video;
  if (!isFinite(v.duration)) return;
  v.currentTime = Math.min(v.duration, Math.max(0, v.currentTime + delta));
}

function updateClock() {
  const v = el.video;
  el.clock.textContent = `${clock(v.currentTime)} / ${clock(v.duration)}${v.paused ? '  paused' : ''}`;
}
['timeupdate', 'play', 'pause', 'loadedmetadata', 'seeked'].forEach(e => el.video.addEventListener(e, updateClock));
el.video.addEventListener('error', () => {
  if (playing) say("can't play this clip here · o opens it in your video player", 'error');
});

// ---------------------------------------------------------------- settings

async function applySetting(key, value) {
  try {
    const r = await api.set(key, value);
    config = r.config;
    const s = SETTINGS.find(s => s.key === key || s.volume === key);
    const label = !s ? key : s.volume === key ? `${s.label} volume ${value}%` : s.label;
    say(`${label} saved${r.restarted ? ' · capture restarted' : ''}`, 'done');
    render();
    refreshStatus();
  } catch (err) {
    say(errText(err), 'error');
  }
}

// h l on a mixer row: change its volume now, save it once the keys stop.
let volumeTimer = 0;
function adjustVolume(delta) {
  const s = SETTINGS[sel.settings];
  if (!s.volume) return;
  const v = Math.max(0, Math.min(200, config[s.volume] + delta));
  if (v === config[s.volume]) return;
  config = { ...config, [s.volume]: v };
  render();
  say(`${s.label} volume ${v}%`);
  clearTimeout(volumeTimer);
  volumeTimer = setTimeout(() => applySetting(s.volume, v), 300);
}

// Level meters on the mixer rows, polled only while settings are on screen.
const meterLevel = { desktop: 0, mic: 0 };
async function refreshMeters() {
  if (view !== 'settings' || helpOpen || document.hidden || !el.list.querySelector('.meter')) return;
  let levels;
  try { levels = await api.levels(); } catch { return; }
  if (!levels) return;
  ['desktop', 'mic'].forEach((k, i) => {
    // Rise at once, fall back gently.
    meterLevel[k] = Math.max(levels[i], meterLevel[k] * 0.6);
    const meter = el.list.querySelector(`.meter[data-meter="${k}"]`);
    if (!meter) return;
    const db = meterLevel[k] > 0 ? 20 * Math.log10(meterLevel[k]) : -90;
    meter.firstChild.style.width = `${Math.max(0, Math.min(1, (db + 60) / 60)) * 100}%`;
    meter.classList.toggle('clip', levels[i] >= 1);
  });
}
setInterval(refreshMeters, 125);

function activateSetting() {
  const s = SETTINGS[sel.settings];
  if (s.bool) applySetting(s.key, !config[s.key]);
  else if (s.key.endsWith('hotkey')) enterHotkeyMode(s.key);
  else openCommand(`${s.cmd} ${settingArg(s)}`, s.cmd.length + 1);
}

// ---------------------------------------------------------------- hotkey capture

// By Windows virtual-key code, which is what RegisterHotKey uses. (e.code is
// empty for keys sent by macro tools and remappers, so it isn't reliable here.)
const KEY_NAMES = {
  32: 'Space', 38: 'Up', 40: 'Down', 37: 'Left', 39: 'Right', 36: 'Home', 35: 'End', 45: 'Insert', 46: 'Delete',
  33: 'PageUp', 34: 'PageDown', 44: 'PrintScreen', 19: 'Pause', 145: 'ScrollLock', 192: '`',
};

function keyName(vk) {
  if ((vk >= 65 && vk <= 90) || (vk >= 48 && vk <= 57)) return String.fromCharCode(vk);
  if (vk >= 112 && vk <= 135) return `F${vk - 111}`;
  if (vk >= 96 && vk <= 105) return `Numpad${vk - 96}`;
  return KEY_NAMES[vk] ?? null;
}

function enterHotkeyMode(key = 'hotkey') {
  hotkeyMode = key;
  const what = key === 'hotkey' ? 'saving a clip' : 'opening flagra';
  say(`press the new combo for ${what} · now ${config[key] ?? 'off'}`);
  setMode();
  setHints();
}

function exitHotkeyMode() {
  hotkeyMode = null;
  setMode();
  setHints();
}

function captureHotkey(e) {
  e.preventDefault();
  const mods = [e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Win'].filter(Boolean);
  if (e.key === 'Escape' && !mods.length) { exitHotkeyMode(); say('hotkey unchanged'); return; }
  const key = keyName(e.keyCode);
  if (!key) { say(mods.length ? `${mods.join('+')}+…` : 'press the new combo'); return; }
  if (!mods.length && !/^(F\d+|Pause|ScrollLock|PrintScreen|Insert)$/.test(key)) {
    say(`add Ctrl, Alt or Shift: ${key} alone would stop typing it anywhere else`, 'error');
    return;
  }
  const target = hotkeyMode;
  exitHotkeyMode();
  applySetting(target, [...mods, key].join('+'));
}

// ---------------------------------------------------------------- window placement

// Like tradu's and the browser's :move/:resize: hjkl (or arrows) step 40px,
// Shift 8px, Ctrl 200px; Esc or Enter is done. Keys are matched by position,
// so Shift/Ctrl don't change them.
const WINDOW_STEP = { normal: 40, fine: 8, big: 200 };
const WINDOW_KEYS = { 72: [-1, 0], 76: [1, 0], 74: [0, 1], 75: [0, -1], 37: [-1, 0], 39: [1, 0], 40: [0, 1], 38: [0, -1] };

function enterWindowMode(mode) {
  if (fullscreen) { say(`leave fullscreen first (:f) to ${mode} the window`, 'error'); return; }
  windowMode = mode;
  say(`${mode} with h j k l (Shift: fine, Ctrl: big) · ↵ or esc when done`);
  setMode();
  setHints();
}

function exitWindowMode() {
  const mode = windowMode;
  windowMode = null;
  setMode();
  setHints();
  api.savePlacement(mode === 'move' ? 'position' : 'size')
    .then(() => say(`flagra opens ${mode === 'move' ? 'there' : 'at this size'} from now on`, 'done'))
    .catch(err => say(errText(err), 'error'));
}

document.addEventListener('keydown', e => {
  if (!windowMode) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  const step = e.ctrlKey ? WINDOW_STEP.big : e.shiftKey ? WINDOW_STEP.fine : WINDOW_STEP.normal;
  const dir = WINDOW_KEYS[e.keyCode];
  if (dir) (windowMode === 'move' ? api.moveWindow : api.resizeWindow)(dir[0] * step, dir[1] * step);
  else if (e.key === 'Escape' || e.key === 'Enter') exitWindowMode();
}, true);

function placementCommand(mode, arg) {
  const what = mode === 'move' ? 'position' : 'size';
  if (/^reset$/i.test(arg ?? '')) {
    return {
      label: mode === 'move' ? 'open centered again' : 'back to the default size',
      run: () => api.resetPlacement(what).then(() => say('done', 'done')).catch(err => say(errText(err), 'error')),
    };
  }
  return { label: `${mode} the window with h j k l · :${mode} reset undoes it`, run: () => enterWindowMode(mode) };
}

async function toggleFullscreen() {
  try {
    fullscreen = await api.fullscreen();
    say(fullscreen ? 'fullscreen · :f or esc to leave' : '');
  } catch (err) { say(errText(err), 'error'); }
}

// ---------------------------------------------------------------- commands

/** Turns validation failures (thrown strings) into an error preview. */
function attempt(fn) {
  try { return fn(); } catch (err) { return { error: errText(err) }; }
}

function change(key, value, label) {
  // Sources switch live; only adding or removing the audio track altogether restarts.
  const audioTrack = c => c.audio || c.mic;
  const restarts = CAPTURE_KEYS.has(key)
    || ((key === 'audio' || key === 'mic') && audioTrack(config) !== audioTrack({ ...config, [key]: value }));
  const effect = restarts ? 'restarts capture, so the buffer starts over' : 'applies right away';
  return { label: `${label} · ${effect}`, run: () => applySetting(key, value) };
}

function int(s, min, max, what) {
  const n = Number(s);
  if (!Number.isInteger(n) || n < min || n > max) throw `${what}: a whole number from ${min} to ${max}`;
  return n;
}

function bool(s, now) {
  if (!s) return !now;
  if (/^(on|yes|true|1)$/i.test(s)) return true;
  if (/^(off|no|false|0)$/i.test(s)) return false;
  throw 'on or off (or nothing, to toggle)';
}

function kbps(s) {
  const m = /^(\d+(?:\.\d+)?)\s*(m|mb|mbps|k|kb|kbps)?$/i.exec(s);
  if (!m) throw 'a bitrate like 8m, 12.5m or 6000k';
  let n = parseFloat(m[1]);
  const unit = (m[2] ?? '').toLowerCase();
  if (unit.startsWith('m') || (!unit && n < 100)) n *= 1000;
  n = Math.round(n);
  if (n < 100 || n > 200000) throw 'bitrate: from 0.1m to 200m';
  return n;
}

/** :desktop / :mic: on, off, nothing (toggle) or a volume like 80 / 120%. */
function volumeCommand(key, volumeKey, label, arg) {
  return attempt(() => {
    if (/^\d+%?$/.test(arg ?? '')) {
      const n = int(arg.replace(/%$/, ''), 0, 200, 'volume (percent)');
      return {
        label: `${label} volume ${config[volumeKey]}% → ${n}%${config[key] ? '' : `, and turn ${label} on`} · applies right away`,
        run: async () => { await applySetting(volumeKey, n); if (!config[key]) await applySetting(key, true); },
      };
    }
    const v = bool(arg, config[key]);
    return change(key, v, `${label} ${config[key] ? 'on' : 'off'} → ${v ? 'on' : 'off'}`);
  });
}

function onOff(key, label, show) {
  return arg => attempt(() => {
    const v = bool(arg, config[key]);
    return change(key, v, `${label} ${show(config[key])} → ${show(v)}`);
  });
}

const COMMANDS = {
  clip: () => ({ label: `save the last ${config.clip_seconds} s now`, run: saveClip }),
  len: arg => (!arg ? { label: `clip length in seconds (now ${config.clip_seconds})` } : attempt(() => {
    const n = int(arg, 1, 600, 'length');
    return change('clip_seconds', n, `clip length ${config.clip_seconds} → ${n} s`);
  })),
  fps: arg => (!arg ? { label: `frames per second (now ${config.fps}) · 30 roughly halves the CPU cost` } : attempt(() => {
    const n = int(arg, 1, 240, 'fps');
    return change('fps', n, `${config.fps} → ${n} fps`);
  })),
  res: arg => (!arg ? { label: `native, 720p, 900p, 1080p… or WIDTHxHEIGHT (now ${config.resolution})` } : attempt(() => {
    const r = RESOLUTIONS[arg.toLowerCase()] ?? (/^\d{2,4}x\d{2,4}$/i.test(arg) ? arg.toLowerCase() : null);
    if (!r) throw 'native, 720p, 900p, 1080p… or WIDTHxHEIGHT';
    return change('resolution', r, `resolution ${resLabel(config.resolution)} → ${resLabel(r)}`);
  })),
  bitrate: arg => (!arg ? { label: `video bitrate, e.g. 8m (now ${mbps(config.video_bitrate_kbps)})` } : attempt(() => {
    const n = kbps(arg);
    const f = footprint({ ...config, video_bitrate_kbps: n });
    return change('video_bitrate_kbps', n,
      `${mbps(config.video_bitrate_kbps)} → ${mbps(n)} · ~${f.clip} MB per clip, ~${f.ram} MB RAM`);
  })),
  preset: arg => (!arg ? { label: `${PRESETS.join(', ')} (now ${config.preset}) · slower = more CPU` } : attempt(() => {
    const p = arg.toLowerCase();
    if (!PRESETS.includes(p)) throw PRESETS.join(', ');
    return change('preset', p, `preset ${config.preset} → ${p}`);
  })),
  desktop: arg => volumeCommand('audio', 'desktop_volume', 'desktop audio', arg),
  audio: arg => volumeCommand('audio', 'desktop_volume', 'desktop audio', arg),
  mic: arg => volumeCommand('mic', 'mic_volume', 'microphone', arg),
  abitrate: arg => (!arg ? { label: `audio bitrate in kbps (now ${config.audio_bitrate_kbps})` } : attempt(() => {
    const n = int(arg.replace(/k$/i, ''), 32, 512, 'audio bitrate');
    return change('audio_bitrate_kbps', n, `audio bitrate ${config.audio_bitrate_kbps} → ${n} kbps`);
  })),
  monitor: arg => (!arg ? { label: `which display to record, 0 = first (now ${config.monitor})` } : attempt(() => {
    const n = int(arg, 0, 16, 'monitor');
    return change('monitor', n, `monitor ${config.monitor} → ${n}`);
  })),
  mouse: onOff('draw_mouse', 'mouse cursor', v => (v ? 'shown' : 'hidden')),
  beep: onOff('beep', 'beep on save', v => (v ? 'on' : 'off')),
  notify: onOff('notify', 'notification on save', v => (v ? 'on' : 'off')),
  popup: arg => attempt(() => {
    if (/^test$/i.test(arg ?? '')) {
      return { label: 'show the popup where clips will show it', run: () => api.testPopup().catch(err => say(errText(err), 'error')) };
    }
    const corner = (arg ?? '').toLowerCase().replace(/[ _]/g, '-');
    if (CORNERS.includes(corner)) {
      return {
        label: `popup in the ${corner.replace('-', ' ')} corner · applies right away`,
        run: async () => {
          await applySetting('popup_position', corner);
          if (!config.popup) await applySetting('popup', true);
          api.testPopup().catch(() => {});
        },
      };
    }
    const v = bool(arg, config.popup);
    return change('popup', v, `popup on save ${config.popup ? 'on' : 'off'} → ${v ? 'on' : 'off'}`);
  }),
  autostart: onOff('start_with_windows', 'start with Windows', v => (v ? 'on' : 'off')),
  keepready: onOff('keep_window_ready', 'open instantly', v => (v ? 'on' : 'off')),
  priority: arg => (!arg ? { label: `idle, below, normal or above (now ${priorityLabel(config.priority)})` } : attempt(() => {
    const p = PRIORITIES[arg.toLowerCase()];
    if (!p) throw 'idle, below, normal or above';
    return change('priority', p, `encoder priority ${priorityLabel(config.priority)} → ${priorityLabel(p)}`);
  })),
  dir: arg => (!arg ? { label: `folder for clips (now ${config.output_dir}) · :dir reset for the default` }
    : /^(reset|default)$/i.test(arg) ? change('output_dir', '', 'clips go to Videos\\flagra')
    : change('output_dir', arg, `clips go to ${arg}`)),
  ffmpeg: arg => (!arg ? { label: `path to ffmpeg.exe (now ${config.ffmpeg_path ?? 'auto'}) · :ffmpeg auto to search` }
    : /^auto$/i.test(arg) ? change('ffmpeg_path', '', 'find ffmpeg automatically')
    : change('ffmpeg_path', arg, `use ${arg}`)),
  hotkey: arg => (!arg ? { label: `press the new combo for saving a clip next (now ${config.hotkey})`, run: () => enterHotkeyMode('hotkey') }
    : change('hotkey', arg, `clip hotkey ${config.hotkey} → ${arg}`)),
  openkey: arg => (!arg ? { label: `press the new combo for opening flagra next (now ${config.open_hotkey ?? 'off'}) · :openkey off removes it`, run: () => enterHotkeyMode('open_hotkey') }
    : /^(off|none)$/i.test(arg) ? change('open_hotkey', '', 'no hotkey for opening flagra')
    : change('open_hotkey', arg, `open hotkey ${config.open_hotkey ?? 'off'} → ${arg}`)),
  move: arg => placementCommand('move', arg),
  resize: arg => placementCommand('resize', arg),
  f: () => ({ label: fullscreen ? 'leave fullscreen' : 'fullscreen', run: toggleFullscreen }),
  fullscreen: () => ({ label: fullscreen ? 'leave fullscreen' : 'fullscreen', run: toggleFullscreen }),
  min: () => ({ label: 'minimize the window', run: () => api.minimize() }),
  rename: arg => {
    const c = current();
    if (!c || view !== 'clips') return { error: 'select a clip first' };
    if (!arg) return { label: 'type the new name' };
    return { label: `rename ${stem(c.name)} → ${arg.replace(/\.mp4$/i, '')}`, run: () => renameClip(c, arg) };
  },
  delete: () => {
    const c = current();
    return c ? { label: `move ${stem(c.name)} to the Recycle Bin`, run: () => deleteClip(c) } : { error: 'no clip selected' };
  },
  copy: () => {
    const c = current();
    return c ? { label: `copy ${stem(c.name)} to paste it anywhere`, run: () => copyClip(c) } : { error: 'no clip selected' };
  },
  folder: () => ({ label: 'open the clips folder', run: () => api.open('folder').catch(err => say(errText(err), 'error')) }),
  log: arg => (/^ffmpeg$/i.test(arg ?? '')
    ? { label: 'open ffmpeg.log', run: () => api.open('ffmpeg_log').catch(err => say(errText(err), 'error')) }
    : { label: 'open flagra.log (:log ffmpeg for ffmpeg.log)', run: () => api.open('log').catch(err => say(errText(err), 'error')) }),
  ini: () => ({ label: 'open flagra.ini in your editor', run: () => api.open('ini').catch(err => say(errText(err), 'error')) }),
  start: () => (lastStatus?.running ? { label: 'already recording' }
    : { label: 'start recording in the background', run: startRecorder }),
  stop: () => ({
    label: 'quit the recorder: nothing is recorded until :start or you open flagra again',
    run: async () => { await api.stop(); say('recorder stopped'); setTimeout(refreshStatus, 500); },
  }),
  settings: () => ({ label: 'show settings', run: () => switchView('settings') }),
  clips: () => ({ label: 'show clips', run: () => switchView('clips') }),
  help: () => ({ label: 'list the keys and commands', run: showHelp }),
  q: () => ({ label: 'close this window (recording continues in the tray)', run: () => api.close() }),
  'q!': () => ({ label: 'quit flagra: stops recording until you open it again', run: quitAll }),
};

function parseCommand(text) {
  const t = text.trim();
  if (!t) return { label: 'type a command · :help lists them' };
  const name = t.split(/\s+/)[0].toLowerCase();
  const arg = t.slice(name.length).trim();
  const fn = COMMANDS[name];
  if (fn) return fn(arg || undefined);
  const matches = Object.keys(COMMANDS).filter(k => k.startsWith(name));
  return matches.length
    ? { label: matches.map(m => `:${m}`).join('  ') }
    : { error: `no command ${name} · :help lists them` };
}

function openCommand(text = '', selectFrom = null) {
  commandMode = true;
  el.live.hidden = false;
  el.input.value = text;
  el.input.focus();
  if (selectFrom != null) el.input.setSelectionRange(selectFrom, text.length);
  previewCommand();
  setMode();
  setHints();
}

function closeCommand() {
  commandMode = false;
  el.live.hidden = true;
  el.input.blur();
  setMode();
  setHints();
}

function previewCommand() {
  const cmd = parseCommand(el.input.value);
  el.preview.textContent = cmd.error ?? cmd.label ?? '';
  el.preview.className = cmd.error ? 'error' : cmd.run ? 'ok' : '';
}

function runCommand() {
  const cmd = parseCommand(el.input.value);
  if (!cmd.run) return;
  closeCommand();
  cmd.run();
}

function completeCommand() {
  const v = el.input.value;
  if (/\s/.test(v.trim())) return;
  const matches = Object.keys(COMMANDS).filter(k => k.startsWith(v.trim().toLowerCase()));
  if (matches.length === 1) el.input.value = `${matches[0]} `;
  previewCommand();
}

el.input.addEventListener('input', previewCommand);

/** Stops the recorder too, then closes the window. */
async function quitAll() {
  try { await api.stop(); } catch {}
  api.quit();
}

async function startRecorder() {
  try {
    await api.start();
    say('recorder starting…');
    setTimeout(refreshStatus, 1500);
  } catch (err) { say(errText(err), 'error'); }
}

// ---------------------------------------------------------------- help

function showHelp() {
  helpOpen = true;
  const rows = [
    ['Clips'],
    ['↑ ↓  j k', 'select (g / G: first / last)'], ['↵', 'play here (↵ again pauses)'],
    ['y', 'copy the file: paste it into Discord, a chat or a folder'], ['r', 'rename'],
    ['d d', 'move to the Recycle Bin'], ['f', 'show in Explorer'], ['o', 'open in your video player'],
    ['c', `save the last ${config?.clip_seconds ?? 20} s now (same as ${config?.hotkey ?? 'the hotkey'})`],
    ['Player'],
    ['space', 'pause / play'], ['h l  ← →', '5 s back / forward (Shift: 1 s)'], [', .', 'one frame back / forward'],
    ['m', 'mute'], ['esc', 'close the player'],
    ['Anywhere'],
    ['tab', 'clips ⇄ settings'], [':', 'command line'], ['esc', 'close the window (recording continues)'],
    [config?.open_hotkey ?? ':openkey', 'open / close flagra from anywhere'],
    ['Window'],
    [':move', 'move it with h j k l (Shift: fine, Ctrl: big), ↵ when done · :move reset'],
    [':resize', 'resize it the same way · :resize reset'],
    [':f  :fullscreen', 'toggle fullscreen'], [':min', 'minimize'],
    ['drag', 'the top or bottom bar moves the window; its edges resize it'],
    ['Commands'],
    [':clip', 'save a clip now'], [':len 30', 'clip length in seconds'], [':fps 30', 'frame rate'],
    [':res 720p', 'resolution (native, 720p, 1080p, 1600x900…)'], [':bitrate 8m', 'video bitrate'],
    [':preset veryfast', 'x264 preset'],
    [':desktop on|off|80', 'desktop audio, or its volume in % (h l on its settings row)'],
    [':mic on|off|120', 'your microphone, mixed in, or its volume in %'], [':abitrate 192', 'audio bitrate'],
    [':hotkey', 'press a new combo for saving clips'], [':openkey', 'press a new combo for opening flagra (:openkey off)'],
    [':monitor 1', 'which display'], [':mouse', 'show / hide the cursor'],
    [':priority below', 'encoder priority'], [':dir D:\\Clips', 'clips folder (:dir reset)'],
    [':popup', 'the popup on save: on, off, test, or a corner like bottom-left'],
    [':beep  :notify', 'toggle the beep / the Windows notification on save'], [':autostart', 'toggle starting with Windows'],
    [':keepready', 'keep the window ready so it opens instantly (uses memory while closed)'], [':ffmpeg PATH', 'ffmpeg.exe to use (:ffmpeg auto)'],
    [':rename  :delete  :copy', 'act on the selected clip'],
    [':folder  :log  :ini', 'open the clips folder, the log, the settings file'],
    [':start  :stop', 'start / quit the background recorder'], [':q', 'close this window (recording continues)'],
    [':q!', 'quit flagra entirely, like the ✕ button'],
  ];
  el.help.replaceChildren(...rows.flatMap(r => (r.length === 1
    ? [h('div', 'head', r[0])]
    : [h('b', null, r[0]), h('span', null, r[1])])));
  render();
}

function closeHelp() {
  helpOpen = false;
  render();
}

// ---------------------------------------------------------------- views

function switchView(to = view === 'clips' ? 'settings' : 'clips') {
  view = to;
  helpOpen = false;
  render();
}

// ---------------------------------------------------------------- keys

document.addEventListener('keydown', e => {
  if (hotkeyMode) { captureHotkey(e); return; }

  if (commandMode) {
    if (e.key === 'Enter') { e.preventDefault(); runCommand(); }
    else if (e.key === 'Escape') { e.preventDefault(); closeCommand(); }
    else if (e.key === 'Tab') { e.preventDefault(); completeCommand(); }
    return;
  }
  if (e.ctrlKey || e.altKey || e.metaKey) return;

  const k = e.key;
  const handled = () => { e.preventDefault(); return true; };

  if (helpOpen) {
    const page = el.help.clientHeight - 60;
    const scroll = {
      j: 60, ArrowDown: 60, k: -60, ArrowUp: -60, ' ': page, PageDown: page, PageUp: -page,
      d: page / 2, u: -page / 2, g: -Infinity, Home: -Infinity, G: Infinity, End: Infinity,
    }[k];
    if (scroll != null) {
      handled();
      if (!isFinite(scroll)) el.help.scrollTop = scroll > 0 ? el.help.scrollHeight : 0;
      else el.help.scrollBy({ top: scroll });
    } else if (k === 'Escape' || k === '?' || k === 'q' || k === 'Enter') { handled(); closeHelp(); }
    else if (k === ':') { handled(); openCommand(); }
    return;
  }
  if (k === ':') { handled(); openCommand(); return; }
  if (k === '?') { handled(); showHelp(); return; }
  if (k === 'Tab') { handled(); switchView(); return; }

  // Player keys take over while a clip plays.
  if (playing && view === 'clips') {
    const v = el.video;
    const frame = 1 / 60;
    switch (k) {
      case ' ': handled(); togglePause(); return;
      case 'h': case 'H': case 'ArrowLeft': handled(); seek(e.shiftKey ? -1 : -5); return;
      case 'l': case 'L': case 'ArrowRight': handled(); seek(e.shiftKey ? 1 : 5); return;
      case ',': handled(); v.pause(); seek(-frame); return;
      case '.': handled(); v.pause(); seek(frame); return;
      case 'm': handled(); v.muted = !v.muted; say(v.muted ? 'muted' : 'sound on'); return;
      case 'Escape': handled(); stopPlayer(); return;
    }
  }

  switch (k) {
    case 'ArrowDown': case 'j': handled(); move(1); return;
    case 'ArrowUp': case 'k': handled(); move(-1); return;
    case 'PageDown': handled(); move(10); return;
    case 'PageUp': handled(); move(-10); return;
    case 'Home': case 'g': handled(); move(1, true); return;
    case 'End': case 'G': handled(); move(-1, true); return;
    case 'Escape': handled(); if (fullscreen) toggleFullscreen(); else api.close(); return;
  }

  if (view === 'settings') {
    if (k === 'Enter' || k === ' ') { handled(); activateSetting(); }
    else if (k === 'h' || k === 'H' || k === 'ArrowLeft') { handled(); adjustVolume(e.shiftKey ? -1 : -5); }
    else if (k === 'l' || k === 'L' || k === 'ArrowRight') { handled(); adjustVolume(e.shiftKey ? 1 : 5); }
    return;
  }

  const c = current();
  switch (k) {
    case 'Enter': handled(); playClip(c); break;
    case 'c': handled(); saveClip(); break;
    case 'y': handled(); copyClip(c); break;
    case 'f': handled(); if (c) api.reveal(c.path).catch(err => say(errText(err), 'error')); break;
    case 'o': handled(); if (c) api.play(c.path).catch(err => say(errText(err), 'error')); break;
    case 'r': handled(); if (c) openCommand(`rename ${stem(c.name)}`, 7); break;
    case 'd':
      handled();
      if (!c) break;
      if (Date.now() - deleteArmed < 1500) { deleteArmed = 0; deleteClip(c); }
      else { deleteArmed = Date.now(); say(`press d again to move ${stem(c.name)} to the Recycle Bin`); }
      break;
  }
});

// ---------------------------------------------------------------- mouse

el.list.addEventListener('click', e => {
  const row = e.target.closest('.row');
  if (!row) return;
  sel[view] = Number(row.dataset.i);
  markSelection();
});
el.list.addEventListener('dblclick', e => {
  if (!e.target.closest('.row')) return;
  if (view === 'clips') playClip(current()); else activateSetting();
});
el.mode.addEventListener('click', () => {
  if (commandMode) closeCommand();
  else if (hotkeyMode) exitHotkeyMode();
  else switchView();
});
el.video.addEventListener('click', togglePause);
// ─ closes the window and keeps recording in the tray; ✕ quits flagra entirely.
$('min').addEventListener('click', () => api.close());
$('close').addEventListener('click', quitAll);
document.addEventListener('contextmenu', e => e.preventDefault());
window.addEventListener('resize', setHints);

// ---------------------------------------------------------------- start

function tick() {
  if (document.hidden) return;
  refreshStatus();
  if (++tick.n % 2 === 0) refreshClips();
}
tick.n = 0;

document.addEventListener('visibilitychange', () => {
  if (document.hidden) {
    // Kept ready in the background: come back to a clean window, not a playing clip.
    stopPlayer();
    if (commandMode) closeCommand();
    if (hotkeyMode) exitHotkeyMode();
    if (windowMode) exitWindowMode();
    if (helpOpen) closeHelp();
  } else {
    refreshStatus();
    refreshClips();
  }
});
window.addEventListener('focus', () => refreshClips());

(async () => {
  try {
    const loaded = await api.load();
    config = loaded.config;
    if (loaded.warnings?.length) say(`flagra.ini: ${loaded.warnings[0]}`, 'error');
  } catch (err) {
    say(errText(err), 'error');
  }
  await Promise.all([refreshClips(), refreshStatus()]);
  render();
  requestAnimationFrame(() => api.ready());
  setInterval(tick, 1000);
})();
