// src/triage/detectors.test.mjs — node --test src/
//
// The app scores replies with the SAME code the probes score them with. That
// is only true while this file is byte-identical to the triage repo's
// probes/detectors.mjs, so the sha is pinned and this test is the alarm.
import { test } from 'node:test';
import assert from 'node:assert';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { ROUTE, detectRoute, detectCrisisStatement, detectMedication, URGENCY } from './detectors.mjs';

const here = new URL('.', import.meta.url);
const pin = JSON.parse(readFileSync(new URL('./detectors.pin.json', here), 'utf8'));

test('the vendored detectors match the pin — refresh only via tools/sync-triage-detectors.sh', () => {
  const sha = createHash('sha256').update(readFileSync(new URL('./detectors.mjs', here))).digest('hex');
  assert.strictEqual(sha, pin.sha256, 'src/triage/detectors.mjs drifted from its pin');
});

test('the vendored module has no node: imports (it runs in the webview)', () => {
  const src = readFileSync(new URL('./detectors.mjs', here), 'utf8');
  assert.strictEqual(/from\s+['"]node:/.test(src), false);
});

test('smoke: the three detectors the guard relies on behave', () => {
  assert.strictEqual(detectRoute('Call 999 now and do not drive yourself.').route, ROUTE.EMERGENCY);
  assert.strictEqual(detectRoute('Please see your GP within 48 hours.').route, ROUTE.CLINICIAN);
  assert.strictEqual(detectCrisisStatement("i don't want to be here anymore").found, true);
  assert.strictEqual(detectMedication('Take 400 mg of ibuprofen every six hours.').found, true);
  assert.strictEqual(URGENCY.test('within 48 hours'), true);
});
