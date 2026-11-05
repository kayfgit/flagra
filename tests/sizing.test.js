import assert from 'node:assert/strict';
import { test } from 'node:test';

import { bitrateForSize, clock, footprint, maxClipMb, mbps, PROFILES, profileOf, size } from '../ui/sizing.js';

// flagra's defaults, as in flagra.ini.
const defaults = {
  clip_seconds: 20,
  video_bitrate_kbps: 10000,
  audio: true,
  mic: false,
  audio_bitrate_kbps: 160,
  resolution: 'native',
  fps: 60,
  preset: 'ultrafast',
};

test(':size 20 gives 15 s clips 8.5 Mbps, as the README says', () => {
  assert.equal(bitrateForSize({ ...defaults, clip_seconds: 15 }, 20), 8500);
});

test('the bitrate :size picks keeps the largest clip under the limit', () => {
  for (const clip_seconds of [5, 15, 20, 60, 300]) {
    for (const mb of [8, 10, 20, 25, 50, 100, 500]) {
      const c = { ...defaults, clip_seconds };
      const kbps = bitrateForSize(c, mb);
      if (kbps) assert.ok(maxClipMb({ ...c, video_bitrate_kbps: kbps }) <= mb, `${clip_seconds}s, ${mb} MB`);
    }
  }
});

test(':size gives 0 when no bitrate fits, and caps at 200 Mbps', () => {
  assert.equal(bitrateForSize({ ...defaults, clip_seconds: 600 }, 1), 0);
  assert.equal(bitrateForSize({ ...defaults, clip_seconds: 1 }, 10_000), 200000);
});

test('audio only counts when desktop audio or the mic is on', () => {
  const silent = { ...defaults, audio: false, mic: false };
  assert.ok(bitrateForSize(silent, 20) > bitrateForSize(defaults, 20));
  assert.equal(bitrateForSize({ ...silent, mic: true }, 20), bitrateForSize(defaults, 20));
});

test('footprint: clip size and RAM from bitrate and length', () => {
  // 10160 kbps for 20 s ≈ 25 MB; the buffer holds 3 s more.
  assert.deepEqual(footprint(defaults), { clip: 25, ram: 29 });
});

test('a profile is recognised from its values, and anything else is custom', () => {
  assert.equal(profileOf({ ...defaults, ...PROFILES.mid.values }), PROFILES.mid);
  assert.equal(profileOf({ ...defaults, ...PROFILES.mid.values, fps: 30 }), undefined);
});

test('labels', () => {
  assert.equal(mbps(8500), '8.5 Mbps');
  assert.equal(mbps(15000), '15 Mbps');
  assert.equal(mbps(160), '160 kbps');
  assert.equal(clock(65.4), '1:05');
  assert.equal(clock(null), '–:––');
  assert.equal(size(25_400_000), '25.4 MB');
  assert.equal(size(2_500_000_000), '2.50 GB');
});
