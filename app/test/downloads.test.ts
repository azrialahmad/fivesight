// node --test test/downloads.test.ts (Node 22.18+ runs TypeScript directly)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { missingModels, startsDownload, VAD_BYTES } from '../src/lib/downloads.ts';
import type { SetupInfo } from '../src/lib/api.ts';

function pc(present: string[], vadPresent: boolean, supported = true): SetupInfo {
  const model = (id: string, bytes: number) => ({ id, label: id, note: '', vramMb: 1000, bytes, present: present.includes(id) });
  return {
    system: { cpu: 'Test CPU', cores: 8, ramMb: 16_384, gpus: [], freeDiskMb: 100_000 },
    recommendation: { supported, speechModel: supported ? 'large-v3-turbo' : null, summary: '', warnings: [], localLlmHint: '' },
    speechModels: [model('large-v3-turbo', 1_624_555_275), model('small-q5_1', 190_085_487)],
    vadPresent,
    downloadDir: 'C:\\models'
  };
}

test('a PC without models needs the voice detection model and the picked speech model', () => {
  const m = missingModels(pc([], false), ['large-v3-turbo']);
  assert.ok(m.vad && m.any);
  assert.deepEqual(m.speech.map((s) => s.id), ['large-v3-turbo']);
  assert.equal(m.bytes, 1_624_555_275 + VAD_BYTES);
});

test('moving on in setup starts the download when something is missing', () => {
  assert.ok(startsDownload(pc([], false), ['large-v3-turbo'], false));
  // Only the voice detection model missing, or only the speech model.
  assert.ok(startsDownload(pc(['large-v3-turbo'], false), ['large-v3-turbo'], false));
  assert.ok(startsDownload(pc(['small-q5_1'], true), ['large-v3-turbo'], false));
});

test('nothing to start when everything is there, a download runs, or the PC cannot transcribe', () => {
  assert.ok(!startsDownload(pc(['large-v3-turbo'], true), ['large-v3-turbo'], false));
  assert.ok(!startsDownload(pc([], false), ['large-v3-turbo'], true));
  assert.ok(!startsDownload(pc([], false, false), ['large-v3-turbo'], false));
  assert.ok(!startsDownload(null, ['large-v3-turbo'], false));
});
