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
  BANNERS, DOSE_REDACTED, PROHIBITED_NOTE, applyGuard, carriesDose, crisisReplaceBlock,
} from './guard.js';
import { detectMedication, detectRoute } from './detectors.mjs';
import { splitSentences } from './lookup-guard.js';
import { crisisRuleFor } from '../triage-turn.js';
import { CRISIS_LEADS_ROW, CRISIS_ONLY_ROW, receiptRows } from '../triage-confirm.js';

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

// ── I3: a wrong number beside the block that the detector does not read as a signpost ──

for (const reply of [
  'Call 999 now. You can also ring 116 132 any time to talk to someone.',
  'Call 999 now. You can also call 0800 068 4141 (HOPELINEUK) any time.',
]) {
  test(`I3: beside the block, a kept sentence with a phone-like number is cut: ${reply}`, () => {
    const v = appGuard(DISCLOSES, reply);
    assert.strictEqual(v.displayText, `${BLOCK}\n\nCall 999 now.`);
    assert.strictEqual(v.banner, 'emergency');
    assert.strictEqual(v.replyShown, true);
    assert.strictEqual(v.signpostsRemoved, 1);
  });
}

test('I3: 999, 111 and 112 are never phone-like; a kept direction with them stays', () => {
  const v = appGuard(DISCLOSES, 'Call 999 or 112 now. If you cannot, call 111.');
  assert.strictEqual(v.displayText, `${BLOCK}\n\nCall 999 or 112 now. If you cannot, call 111.`);
});

test('I3: with no disclosure, a phone-like number is untouched (the cut is only beside the block)', () => {
  const v = appGuard(CHEST, 'Call 999 now. You can also ring 116 132 any time to talk to someone.');
  assert.strictEqual(v.displayText, 'Call 999 now. You can also ring 116 132 any time to talk to someone.');
});

// ── M1: the de-duplication splitter and "e.g." ──────────────────────────────

test('M1: de-duplication does not split at "e.g." — no "landline.g. tonight"', () => {
  const v = appGuard(DISCLOSES, 'Call 999 now, e.g. from a landline. Ring the Samaritans on 116 124, e.g. tonight.');
  assert.strictEqual(v.displayText, `${BLOCK}\n\nCall 999 now, e.g. from a landline.`);
  assert.strictEqual(v.signpostsRemoved, 1);
});

// ── I5: the receipt never repeats a dose ────────────────────────────────────

test('I5: a removed lethal-quantity sentence is not repeated verbatim in the receipt', () => {
  const v = appGuard(CHEST, 'Anything over 24 tablets in a day can be fatal, so see a doctor today.');
  const rows = receiptRows(v);
  assert.ok(rows.length > 0);
  for (const r of rows) assert.strictEqual(carriesDose(r.split(DOSE_REDACTED).join(' ')), false, r);
  assert.strictEqual(rows.some((r) => /24 tablets/.test(r)), false, rows.join(' | '));
});

test('I5: no receipt row carries a dose, over a set of dose replies; diagnosis sentences stay verbatim', () => {
  for (const reply of [
    'Take 2.5 ml. Call 111.',
    'Take 500mg paracetamol every 4 hours. See your GP today.',
    'Take two tablets of ibuprofen three times a day. See your GP.',
    '### What to do\n1. Take 2 tablets of paracetamol.\n2. Call 999 now.',
  ]) {
    const v = appGuard(CHEST, reply);
    for (const r of receiptRows(v)) assert.strictEqual(carriesDose(r.split(DOSE_REDACTED).join(' ')), false, `${reply} -> ${r}`);
  }
  const dx = appGuard(CHEST, 'This sounds like angina. Call 999 now.');
  assert.ok(receiptRows(dx).includes('Removed: This sounds like angina.'), receiptRows(dx).join(' | '));
});

// ── M4: a kept reply that de-duplication emptied ────────────────────────────

test('M4: a kept reply emptied by de-duplication gets its own receipt rows, not "replaced the model\'s reply"', () => {
  const v = appGuard(DISCLOSES, 'Go to A&E now and call the Samaritans on 116 123.');
  assert.strictEqual(v.replyShown, false);
  assert.strictEqual(v.banner, 'emergency');
  const rows = receiptRows(v);
  assert.strictEqual(rows.some((r) => /replaced the model's reply/.test(r)), false, rows.join(' | '));
  assert.deepStrictEqual(rows, [
    'One sentence giving a crisis line was removed; the product\'s crisis support is the only one shown.',
    CRISIS_ONLY_ROW,
  ]);
  assert.ok(rows.includes(CRISIS_LEADS_ROW) === false, 'nothing is shown under the block');
});

test('M4: a hidden (not kept) reply still reads "replaced the model\'s reply" alone', () => {
  const v = appGuard(DISCLOSES, 'Rest and drink fluids.');
  assert.strictEqual(v.banner, null);
  assert.deepStrictEqual(receiptRows(v), ["The product's crisis support replaced the model's reply, which is not shown."]);
});

// ── I4: a disclaimer the product strips no longer hides the direction ───────

test('I4: "I cannot judge how serious this is. Call 999 now." on a disclosure keeps the direction under the block', () => {
  const v = appGuard(DISCLOSES, 'I cannot judge how serious this is. Call 999 now.');
  assert.strictEqual(v.route, 'UNCLEAR', 'the verdict route is still the raw route');
  assert.strictEqual(v.why, 'contradictory-out-of-scope-and-emergency');
  assert.strictEqual(v.replyShown, true);
  // The raw route is UNCLEAR, so the product's UNCLEAR line follows, exactly
  // as it does on the same reply with no disclosure.
  assert.strictEqual(v.displayText, `${BLOCK}\n\nCall 999 now. ${BANNERS.out_of_scope.line}`);
  assert.strictEqual(v.banner, 'out_of_scope', 'the raw route\'s banner, as with no disclosure');
  assert.deepStrictEqual(v.disclaimersRemoved, ['I cannot judge how serious this is.']);
});

test('I4: the no-disclosure display of the same reply is unchanged', () => {
  const v = appGuard(CHEST, 'I cannot judge how serious this is. Call 999 now.');
  assert.strictEqual(v.banner, 'out_of_scope');
  assert.strictEqual(detectRoute(v.displayText.replace(/ The model has no basis.*$/, '')).route, 'EMERGENCY');
});

test('I4: a disclaimer beside a SELF-CARE remainder is still hidden on a disclosure', () => {
  const v = appGuard(DISCLOSES, 'I cannot assess this properly. Rest and drink plenty of fluids.');
  assert.strictEqual(v.replyShown, false);
  assert.strictEqual(v.displayText, BLOCK);
  assert.strictEqual(v.banner, null);
});

test('I4: a disclaimer beside a referral (no-basis-to-triage-and-signposted) keeps the referral under the block', () => {
  const reply = 'I am not able to assess this. Please see your GP.';
  const v = appGuard(DISCLOSES, reply);
  assert.strictEqual(v.why, 'no-basis-to-triage-and-signposted', v.why);
  assert.strictEqual(v.replyShown, true);
  assert.strictEqual(v.banner, 'out_of_scope');
  assert.strictEqual(v.displayText, `${BLOCK}\n\nPlease see your GP.`);
});

test('I4: with the disclaimer strip switched off, the keep decision is the raw route again (hidden)', () => {
  const v = applyGuard({
    userText: DISCLOSES, replyText: 'I cannot judge how serious this is. Call 999 now.',
    crisisLine: ENTRY.crisisLine, ...crisisRuleFor(ENTRY), stripScopeDisclaimers: false,
  });
  assert.strictEqual(v.replyShown, false);
  assert.strictEqual(v.banner, null);
});
