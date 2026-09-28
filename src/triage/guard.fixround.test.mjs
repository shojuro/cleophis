// src/triage/guard.fixround.test.mjs — node --test src/
//
// Phase 1i, the mobile whole-branch review's fix round (final-review-mobile.md,
// I1-I5, I7, M1, M4). Every case is the review's reproduced input, run at the
// SHIPPED call path: `applyGuard` with exactly the options the app passes
// (`src/app.js`: `crisisLine: entry.crisisLine || undefined,
// ...crisisRuleFor(entry)` over the triage catalog's registered entry), and the
// confirm receipt read from that verdict with `receiptRows`.
import { test } from 'node:test';
import assert from 'node:assert';
import { readFileSync } from 'node:fs';
import {
  DOSE_REDACTED, PROHIBITED_NOTE, applyGuard, carriesDose, crisisReplaceBlock,
} from './guard.js';
import { detectMedication, detectRoute } from './detectors.mjs';
import { splitSentences } from './lookup-guard.js';
import { crisisRuleFor } from '../triage-turn.js';
import { CRISIS_LEADS_ROW, receiptRows } from '../triage-confirm.js';

const CATALOG = JSON.parse(readFileSync(new URL('../../src-tauri/resources/catalog.triage.json', import.meta.url), 'utf8'));
const ENTRY = CATALOG.find((e) => e.id === 'med-triage');

/** The app's literal call (src/app.js finishStream). */
const appGuard = (userText, replyText) => applyGuard({
  userText, replyText, crisisLine: ENTRY.crisisLine || undefined, ...crisisRuleFor(ENTRY),
});

const CHEST = 'chest pain for an hour, sweaty, arm aches';
const DISCLOSES = 'I want to kill myself';
const BLOCK = crisisReplaceBlock(ENTRY.crisisLine);
const NOTE = PROHIBITED_NOTE.trim();
const doseOnScreen = (text) => splitSentences(text.split(DOSE_REDACTED).join(' ')).some((s) => carriesDose(s.text));

test('the registered entry is the replace rule over {EMERGENCY, CLINICIAN} (what every case below runs under)', () => {
  assert.deepStrictEqual(crisisRuleFor(ENTRY), { crisisRule: 'replace', replaceKeepRoutes: ['EMERGENCY', 'CLINICIAN'] });
});

// ── I1: spec A34's own example, at the shipped path (option (b)) ──────────
//
// The frozen `detectMedication` reads "Give 2 puffs" as a dose, so the
// prohibited filter (step 2) cuts the sentence whole before the MA6 dose strip
// ever sees it, and the EMERGENCY banner is the route's carrier. Redacting it
// instead cannot help: the redacted sentence still trips the detector (a drug
// class in an administration frame, and "give [dose removed]" reads as a
// dose), which is also what the census's displayed no-dose reading runs.

test('I1: "Give 2 puffs of the blue inhaler and call 999." shows the note alone under the EMERGENCY banner (spec A34 corrected)', () => {
  const v = appGuard(CHEST, 'Give 2 puffs of the blue inhaler and call 999.');
  assert.strictEqual(v.displayText, NOTE);
  assert.strictEqual(v.banner, 'emergency');
  assert.strictEqual(v.route, 'EMERGENCY');
  assert.strictEqual(v.dosesRemoved, 0, 'cut by the prohibited filter, not the dose strip');
  assert.deepStrictEqual(v.prohibitedRemoved, ['Give 2 puffs of the blue inhaler and call 999.']);
  assert.strictEqual(detectMedication(`Give ${DOSE_REDACTED} of the blue inhaler and call 999.`).found, true,
    'a redacted copy would still trip the frozen detector');
});

test('I1: the in-place redaction reaches a dose-bearing route sentence the detectors do not cut', () => {
  const v = appGuard(CHEST, 'Go to A&E now and take no more than 2 tablets.');
  assert.strictEqual(v.displayText, `Go to A&E now and take no more than ${DOSE_REDACTED}. ${NOTE}`);
  assert.strictEqual(v.banner, 'emergency');
  assert.strictEqual(v.dosesRemoved, 1);
  assert.deepStrictEqual(v.prohibitedRemoved, []);
  assert.strictEqual(detectMedication(v.displayText).found, false, 'the display is clean to the frozen detector');
});

test('I1: a non-route-bearing dose sentence is still cut whole', () => {
  const v = appGuard(CHEST, 'Give 2 puffs of the blue inhaler. Call 999 now.');
  assert.strictEqual(v.displayText, `Call 999 now. ${NOTE}`);
});

// ── I2: decimals and list markers in the prohibited filter ──────────────────

test('I2: "Take 2.5 ml. Call 111." never leaves "Take 2." on screen', () => {
  const v = appGuard(CHEST, 'Take 2.5 ml. Call 111.');
  assert.strictEqual(v.displayText, `Call 111. ${NOTE}`);
  assert.strictEqual(v.banner, 'clinician');
  assert.strictEqual(/Take 2/.test(v.displayText), false);
});

test('I2: a cut list item takes its marker with it — no orphaned "1. 2."', () => {
  const v = appGuard(CHEST, '### What to do\n1. Take 2 tablets of paracetamol.\n2. Call 999 now.');
  assert.strictEqual(v.displayText, `### What to do\n2. Call 999 now. ${NOTE}`);
  assert.strictEqual(v.banner, 'emergency');
});

