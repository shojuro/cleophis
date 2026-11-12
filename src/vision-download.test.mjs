import { test } from 'node:test';
import assert from 'node:assert/strict';

import { VISION_DOWNLOAD_PROMPT, visionAvailability, visionDownloadPlan } from './vision-download.js';
import { formatBytes } from './download-policy.js';

const sha = (c) => c.repeat(64);
const VBASE = { path: 'models/granite-docling-258M-Q8_0.gguf', sha256: sha('1'), size: 177918144, kind: 'vision_base', base_model: 'granite-docling-258M' };
const VPROJ = { path: 'models/mmproj-granite-docling-258M-Q8_0.preproc512.gguf', sha256: sha('2'), size: 103769792, kind: 'mmproj', base_model: 'granite-docling-258M' };
const VARIANT = { baseModel: 'granite-docling-258M', sha256: sha('1'), mmprojSha256: sha('2') };

test('the prompt names the one-time size through formatBytes', () => {
  assert.equal(VISION_DOWNLOAD_PROMPT(281687936), `Photo input needs a one-time ${formatBytes(281687936)} download.`);
  assert.match(VISION_DOWNLOAD_PROMPT(281687936), /^Photo input needs a one-time \d+ MB download\.$/);
});

test('no variant is unavailable whatever the device reports', () => {
  assert.equal(visionAvailability({ variant: null, visionState: { declared: true, installed: true } }), 'unavailable');
  assert.equal(visionAvailability({ variant: undefined, visionState: { declared: true, installed: false } }), 'unavailable');
});

test('a null vision state (desktop) is unavailable even with a variant', () => {
  assert.equal(visionAvailability({ variant: VARIANT, visionState: null }), 'unavailable');
  assert.equal(visionAvailability({ variant: VARIANT, visionState: undefined }), 'unavailable');
});

test('declared and not installed needs a download', () => {
  assert.equal(visionAvailability({ variant: VARIANT, visionState: { declared: true, installed: false, coresident: false } }), 'needs-download');
});

test('installed is ready', () => {
  assert.equal(visionAvailability({ variant: VARIANT, visionState: { declared: true, installed: true, coresident: false } }), 'ready');
  assert.equal(visionAvailability({ variant: VARIANT, visionState: { declared: false, installed: true, coresident: false } }), 'ready');
});

test('neither declared nor installed is unavailable', () => {
  assert.equal(visionAvailability({ variant: VARIANT, visionState: { declared: false, installed: false, coresident: false } }), 'unavailable');
});

test('visionDownloadPlan totals the base and projector sizes', () => {
  const plan = visionDownloadPlan([VBASE, VPROJ], VARIANT);
  assert.equal(plan.base, VBASE);
  assert.equal(plan.mmproj, VPROJ);
  assert.equal(plan.totalBytes, 177918144 + 103769792);
});

test("visionDownloadPlan surfaces pickVisionArtifacts's readable error when the projector is missing", () => {
  assert.throws(() => visionDownloadPlan([VBASE], VARIANT), /projector/);
  assert.throws(() => visionDownloadPlan([], VARIANT), /isn't published yet/);
});

test('visionDownloadPlan refuses a missing or invalid size instead of coercing to 0', () => {
  const msg = /lists no size for the vision files/;
  assert.throws(() => visionDownloadPlan([{ ...VBASE, size: undefined }, VPROJ], VARIANT), msg);
  assert.throws(() => visionDownloadPlan([VBASE, { ...VPROJ, size: '100' }], VARIANT), msg);
  assert.throws(() => visionDownloadPlan([VBASE, { ...VPROJ, size: -1 }], VARIANT), msg);
  assert.throws(() => visionDownloadPlan([VBASE, { ...VPROJ, size: NaN }], VARIANT), msg);
});
