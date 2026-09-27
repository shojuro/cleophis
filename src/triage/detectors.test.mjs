// src/triage/detectors.test.mjs — node --test src/
//
// The app scores replies with the SAME code the probes score them with. That
// is only true while this file is byte-identical to the triage repo's
// probes/detectors.mjs, so the sha is pinned and this test is the alarm.
import { test } from 'node:test';
import assert from 'node:assert';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { ROUTE, detectRoute, detectCrisisStatement, detectMedication, URGENCY } from './detectors.mjs';
import pinModule from './detectors.pin.js';

const here = new URL('.', import.meta.url);
const pin = JSON.parse(readFileSync(new URL('./detectors.pin.json', here), 'utf8'));

/** Every .js/.mjs/.html file under src/, so a rule can be asserted of all of them. */
function sourceFiles(dir = new URL('../', here), out = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const child = new URL(e.name + (e.isDirectory() ? '/' : ''), dir);
    if (e.isDirectory()) sourceFiles(child, out);
    else if (/\.(js|mjs|html)$/.test(e.name)) out.push(child);
  }
  return out;
}

test('the vendored detectors match the pin — refresh only via tools/sync-triage-detectors.sh', () => {
  const sha = createHash('sha256').update(readFileSync(new URL('./detectors.mjs', here))).digest('hex');
  assert.strictEqual(sha, pin.sha256, 'src/triage/detectors.mjs drifted from its pin');
});

test('the two pin files agree — one sync run writes both', () => {
  assert.deepStrictEqual(pinModule, pin, 'detectors.pin.js and detectors.pin.json disagree; re-run tools/sync-triage-detectors.sh');
});

/**
 * Source with `//` and block comments removed.
 *
 * The rule below is about CODE, and the files that carry it also have to be
 * able to say in prose which syntax is banned and why — the first version of
 * this test failed on its own explanation. Stripping first cannot hide a real
 * offender: the only way it could is an import attribute written after a `//`
 * inside a string literal on the same line, which is not a thing.
 */
function codeOf(url) {
  return readFileSync(url, 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/\/\/[^\n]*/g, ' ');
}

// The front end is loaded as plain ES modules with no bundler, so an import
// attribute anywhere in `app.js`'s graph is a Chromium-123 floor on the WHOLE
// app: an un-updated System WebView on a minSdk-24 device fails to PARSE the
// module and neither build boots — the tutor's included. Asserted over every
// source file rather than over `guard.js` alone, because the rule is about the
// module graph, and the next file to reach for a JSON import will not be this
// one. Both spellings, and both the static and the dynamic form: `with` is
// current, `assert` is what older code wrote and Chromium dropped in 126.
test('no file under src/ imports JSON with an import attribute', () => {
  const offenders = sourceFiles()
    .filter((u) => /\b(with|assert)\s*:?\s*\{\s*type\s*:\s*['"]json['"]/.test(codeOf(u)))
    .map((u) => u.pathname);
  assert.deepStrictEqual(offenders, [], 'JSON import attributes need Chromium >= 123; import a generated .js module instead');
});

// The lock-step alarm. `detectorsSha` keeps the audit trail honest whatever
// happens here, but a re-pin in the research repo (D3/D4 land there next) must
// not leave `npm test` green while the product scores replies with older code.
// Skipped by name, not silently, when the research repo is not on this box.
const TRIAGE_PIN = '/home/penguinzyue/cleophas-triage/artifacts/detectors-pin.json';
test('the product pin matches the triage repo\'s own pin', { skip: existsSync(TRIAGE_PIN) ? false : `no ${TRIAGE_PIN} on this machine` }, () => {
  const upstream = JSON.parse(readFileSync(TRIAGE_PIN, 'utf8'));
  assert.strictEqual(pin.sha256, upstream.sha256, `the triage repo re-pinned ${upstream.sha256}; run tools/sync-triage-detectors.sh`);
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
