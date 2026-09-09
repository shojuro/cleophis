// src/triage/guard.test.mjs — node --test src/
//
// The guard is the product-side contract on a supervised reply, so these tests
// are written as the contract rather than as coverage: the four banner strings,
// the route -> banner map, the time-frame note and every phrase the strip
// removes are asserted VERBATIM. Changing what a health worker sees therefore
// costs a red test, which is the point — the banner is a safety surface, not
// copy.
//
// Task 3 owns route, banner and time frames. The crisis block and the full
// prohibited filter are Task 4's; the hooks are asserted here only to the
// extent that they exist and are inert on a clean reply.
import { test } from 'node:test';
import assert from 'node:assert';
import { readFileSync } from 'node:fs';
import {
  BANNERS, CRISIS_BLOCK_DEFAULT, PROHIBITED_NOTE, ROUTE_TO_BANNER, TIME_FRAME_NOTE,
  applyGuard, filterProhibited, routeOfPrefix, stripTimeFrames,
} from './guard.js';

const pin = JSON.parse(readFileSync(new URL('./detectors.pin.json', import.meta.url), 'utf8'));

test('every route maps to exactly one banner and UNCLEAR renders out_of_scope, never nothing', () => {
  assert.deepStrictEqual(Object.keys(BANNERS).sort(), ['clinician', 'emergency', 'out_of_scope', 'self_care']);
  for (const r of ['EMERGENCY', 'CLINICIAN', 'SELF_CARE', 'OUT_OF_SCOPE', 'UNCLEAR']) {
    assert.ok(ROUTE_TO_BANNER[r] in BANNERS, r);
  }
  assert.strictEqual(ROUTE_TO_BANNER.UNCLEAR, 'out_of_scope');
});

test('an EMERGENCY reply keeps its urgency words and gets the emergency banner', () => {
  const v = applyGuard({ userText: 'crushing chest pain going down my arm', replyText: 'Call 999 now. Do not drive yourself.' });
  assert.strictEqual(v.route, 'EMERGENCY');
  assert.strictEqual(v.banner, 'emergency');
  assert.strictEqual(v.displayText, 'Call 999 now. Do not drive yourself.');
  assert.deepStrictEqual(v.timeframeStripped, []);
  assert.strictEqual(v.detectorsSha, pin.sha256);
  assert.strictEqual(v.rawReply, 'Call 999 now. Do not drive yourself.');
});

test('a CLINICIAN reply has its stated time frame removed and the note appended once', () => {
  const v = applyGuard({
    userText: 'the ache moved to the bottom right and i have been sick twice',
    replyText: 'This needs looking at. Please see a doctor or GP within the next 48 hours so they can examine you.',
  });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.banner, 'clinician');
  assert.deepStrictEqual(v.timeframeStripped, ['within the next 48 hours']);
  assert.strictEqual(/within the next 48 hours/i.test(v.displayText), false);
  assert.ok(v.displayText.endsWith(TIME_FRAME_NOTE), v.displayText);
  assert.strictEqual(v.displayText.split(TIME_FRAME_NOTE).length, 2, 'the note appears once');
});

test('stripTimeFrames removes every URGENCY match and tidies the sentence', () => {
  const r = stripTimeFrames('See your GP today. Book an appointment as soon as possible, ideally this week.');
  assert.deepStrictEqual(r.stripped, ['today', 'as soon as possible', 'this week']);
  assert.strictEqual(r.text, 'See your GP. Book an appointment, ideally.');
});

test('a SELF_CARE reply keeps the time frame inside its escalation caveat', () => {
  const reply = 'Rest and drink fluids. If it has not settled within 3 days, see your GP.';
  const v = applyGuard({ userText: 'sore throat', replyText: reply });
  assert.strictEqual(v.route, 'SELF_CARE');
  assert.strictEqual(v.displayText, reply);
});

test('UNCLEAR renders the out_of_scope banner with the signposting line, never a blank', () => {
  const v = applyGuard({ userText: 'x', replyText: 'Hmm.' });
  assert.strictEqual(v.route, 'UNCLEAR');
  assert.strictEqual(v.banner, 'out_of_scope');
  assert.ok(v.displayText.includes(BANNERS.out_of_scope.line));
});

test('routeOfPrefix resolves a growing prefix without throwing', () => {
  assert.strictEqual(routeOfPrefix(''), 'UNCLEAR');
  assert.strictEqual(routeOfPrefix('Call'), 'UNCLEAR');
  assert.strictEqual(routeOfPrefix('Call 999 now'), 'EMERGENCY');
});

// ── The banner is fixed text, and the verdict is a fixed shape ──────────────
//
// The tests above read BANNERS.out_of_scope.line rather than the literal, so
// they would stay green if the wording changed under them. This one is the
// alarm for that: the strings a health worker reads are asserted here, once.

test('the four banner strings and the two notes are fixed text', () => {
  assert.deepStrictEqual(BANNERS.emergency, {
    title: 'EMERGENCY',
    line: 'Emergency services now. Do not let them drive themselves.',
  });
  assert.deepStrictEqual(BANNERS.clinician, {
    title: 'SEE A CLINICIAN',
    line: 'Needs assessment. The health worker decides when.',
  });
  assert.deepStrictEqual(BANNERS.self_care, {
    title: 'SELF-CARE',
    line: 'Manage at home. Watch for the change that means seeking care.',
  });
  assert.deepStrictEqual(BANNERS.out_of_scope, {
    title: 'CANNOT JUDGE FROM THIS',
    line: 'The model has no basis to route this. Health worker to assess.',
  });
  assert.strictEqual(TIME_FRAME_NOTE, ' Timing is for your health worker to decide.');
  assert.strictEqual(PROHIBITED_NOTE, ' A clinician can advise on treatment.');
});

test('the verdict carries every key Tasks 4, 6 and 7 read, on every route', () => {
  const expected = [
    'banner', 'crisisLineAppended', 'crisisOnInput', 'detectorsSha', 'displayText',
    'prohibited', 'rawReply', 'route', 'timeframeStripped', 'timeframeUnlocated', 'why',
  ];
  const replies = [
    'Call 999 now. Do not drive yourself.',
    'Please see your GP within 48 hours.',
    'Rest and drink fluids.',
    'Hmm.',
  ];
  for (const replyText of replies) {
    const v = applyGuard({ userText: 'sore throat', replyText });
    assert.deepStrictEqual(Object.keys(v).sort(), expected, replyText);
    assert.deepStrictEqual(Object.keys(v.prohibited).sort(), ['diagnosis', 'medication'], replyText);
    assert.strictEqual(v.rawReply, replyText, 'the raw reply is always kept for the log');
    assert.ok(v.banner in BANNERS, replyText);
    assert.strictEqual(typeof v.why, 'string');
    assert.strictEqual(v.detectorsSha, pin.sha256);
    assert.ok(Array.isArray(v.timeframeStripped), replyText);
  }
});

// ── Degenerate input ────────────────────────────────────────────────────────

test('applyGuard never throws on empty, missing or null input, and still routes the reader somewhere', () => {
  for (const args of [undefined, {}, { userText: '', replyText: '' }, { userText: null, replyText: null }]) {
    const v = applyGuard(args);
    assert.strictEqual(v.route, 'UNCLEAR', JSON.stringify(args ?? null));
    assert.strictEqual(v.banner, 'out_of_scope');
    assert.strictEqual(v.rawReply, '');
    assert.ok(v.displayText.includes(BANNERS.out_of_scope.line), v.displayText);
    assert.deepStrictEqual(v.prohibited, { medication: [], diagnosis: [] });
    assert.strictEqual(v.crisisLineAppended, false);
  }
});

test('routeOfPrefix never throws on a partial word or a partial number', () => {
  for (const prefix of [undefined, null, '', '   ', 'C', 'Cal', 'Call', 'Call 9', 'Call 99']) {
    assert.strictEqual(routeOfPrefix(prefix), 'UNCLEAR', JSON.stringify(prefix ?? null));
  }
  assert.strictEqual(routeOfPrefix('Call 999'), 'EMERGENCY');
  assert.strictEqual(routeOfPrefix('Call 999 now. Do not drive'), 'EMERGENCY');
});

// ── The strip removes time frames, and only time frames ─────────────────────

test('stripTimeFrames leaves a reply that states no time frame byte-identical', () => {
  const clean = 'Rest and drink fluids, and keep an eye on the rash.';
  assert.deepStrictEqual(stripTimeFrames(clean), { text: clean, stripped: [] });
});

test('stripTimeFrames lists a single-word time frame and tidies the space it left', () => {
  const r = stripTimeFrames('Please be seen urgently.');
  assert.deepStrictEqual(r.stripped, ['urgently']);
  assert.strictEqual(r.text, 'Please be seen.');
});

test('the strip is scoped to CLINICIAN: an EMERGENCY reply keeps "straight away"', () => {
  const reply = 'Call an ambulance straight away. Do not drive yourself.';
  const v = applyGuard({ userText: 'crushing chest pain', replyText: reply });
  assert.strictEqual(v.route, 'EMERGENCY');
  assert.strictEqual(v.displayText, reply);
  assert.deepStrictEqual(v.timeframeStripped, []);
});

// ── Task 4's hooks: present, and inert on a clean reply ─────────────────────

test('filterProhibited is a no-op on a reply that names no medication and no condition', () => {
  const clean = 'Please see a doctor so they can examine you.';
  assert.deepStrictEqual(filterProhibited(clean, 'my stomach hurts'), {
    text: clean, medication: [], diagnosis: [],
  });
});

test('CRISIS_BLOCK_DEFAULT is present for Task 4 and says the block is additional to the route', () => {
  assert.strictEqual(typeof CRISIS_BLOCK_DEFAULT, 'string');
  assert.ok(CRISIS_BLOCK_DEFAULT.includes('116 123'), CRISIS_BLOCK_DEFAULT);
  assert.ok(CRISIS_BLOCK_DEFAULT.includes('999'), CRISIS_BLOCK_DEFAULT);
  assert.ok(/in addition to the advice above, not instead of it/.test(CRISIS_BLOCK_DEFAULT));
});

test('a CLINICIAN reply that stated no time frame is shown unchanged, with no note', () => {
  const reply = 'This needs looking at. Please see your GP so they can examine you.';
  const v = applyGuard({ userText: 'the ache moved to the bottom right', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.deepStrictEqual(v.timeframeStripped, []);
  assert.strictEqual(v.displayText, reply, 'the note is appended only when something was stripped');
  assert.strictEqual(v.displayText.includes(TIME_FRAME_NOTE.trim()), false);
});

test('stripTimeFrames matches a time frame however it is capitalised', () => {
  // The detectors only ever apply URGENCY to lower-cased text; the guard applies
  // it to the reply as written, where "ASAP" and "Today" are the usual spellings.
  const r = stripTimeFrames('Book an appointment ASAP.');
  assert.deepStrictEqual(r.stripped, ['ASAP']);
  assert.strictEqual(r.text, 'Book an appointment.');
});

test('the route is read from the RAW reply, before anything is stripped from it', () => {
  // Stripping first would demote this to CLINICIAN: "straight away" is what makes
  // it an emergency direction, and the banner must say what the model said.
  const reply = 'Straight away go to your nearest walk-in centre.';
  const v = applyGuard({ userText: 'my chest is tight', replyText: reply });
  assert.strictEqual(v.route, 'EMERGENCY');
  assert.strictEqual(v.banner, 'emergency');
  assert.strictEqual(v.displayText, reply, 'an EMERGENCY route strips nothing');
  assert.deepStrictEqual(v.timeframeStripped, []);
});

// Task 4 owns what the prohibited filter removes. What Task 3 owns is that
// applyGuard runs it at all, and that a removal is LISTED rather than silent.
test('applyGuard routes the reply through the prohibited filter and lists what it removed', () => {
  const reply = 'This needs looking at. Take 400 mg of ibuprofen every six hours. Please see your GP.';
  const v = applyGuard({ userText: 'my head hurts', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.ok(v.prohibited.medication.includes('ibuprofen'), JSON.stringify(v.prohibited));
  assert.strictEqual(/ibuprofen/i.test(v.displayText), false, 'the drug never reaches the screen');
  assert.strictEqual(/400 mg/i.test(v.displayText), false, 'nor does its dose');
  assert.ok(v.displayText.includes(PROHIBITED_NOTE.trim()), v.displayText);
  assert.strictEqual(v.rawReply, reply, 'the log still has what the model actually said');
});

test("the user's own words reach the filter, so a drug they raised is not treated as one the model introduced", () => {
  // R7/R16's carve-out lives in the detectors; what is asserted here is only
  // that applyGuard hands them the patient text. Without it, identifying a drug
  // the person themselves named reads as prescribing, and a correct answer is
  // deleted from the screen.
  const userText = 'i already took some ibuprofen earlier, is that ok';
  const reply = 'Ibuprofen is an anti-inflammatory. Please see your GP so they can examine you.';
  const v = applyGuard({ userText, replyText: reply });
  assert.deepStrictEqual(v.prohibited.medication, []);
  assert.strictEqual(v.displayText, reply);
  assert.deepStrictEqual(filterProhibited(reply, '').medication, ['ibuprofen'], 'and it does read as an introduction with no patient text');
});

// ── A time frame written as a number word is still a time frame ─────────────
//
// URGENCY's numeric branch is \d+-only. The detectors never notice, because they
// match against normaliseReply's output, where NUMBER_WORDS has already turned
// "two" into "2". The guard matches the reply AS WRITTEN — it has to, it removes
// text a person will read — so it widens that one branch with the detectors' own
// exported list. One definition of a number word, not two.

test('a time frame written as a number word is stripped and listed exactly as written', () => {
  const r = stripTimeFrames('Please see your GP within the next two days.');
  assert.deepStrictEqual(r.stripped, ['within the next two days']);
  assert.strictEqual(r.text, 'Please see your GP.');
});

test('the number word need not follow "the next"', () => {
  const r = stripTimeFrames('Please see your GP within three days.');
  assert.deepStrictEqual(r.stripped, ['within three days']);
  assert.strictEqual(r.text, 'Please see your GP.');
});

test('widening the branch did not cost the digit form, the range form or either mixed', () => {
  for (const [reply, phrase] of [
    ['Please see your GP within the next 48 hours.', 'within the next 48 hours'],
    ['Please see your GP within 2-3 days.', 'within 2-3 days'],
    ['Please see your GP within two-three days.', 'within two-three days'],
    ['Please see your GP Within Two Days.', 'Within Two Days'],
  ]) {
    const r = stripTimeFrames(reply);
    assert.deepStrictEqual(r.stripped, [phrase], reply);
    assert.strictEqual(r.text, 'Please see your GP.', reply);
  }
});

test('a number word that is not a time frame is left alone', () => {
  for (const clean of [
    'Two of the symptoms you describe need looking at. Please see your GP.',
    'One side of the rash is spreading. Please see your GP.',
    'Both of you should rest for a bit.',
  ]) {
    assert.deepStrictEqual(stripTimeFrames(clean), { text: clean, stripped: [] }, clean);
  }
});

test('a CLINICIAN reply stating a number-word time frame loses it and gets the note once', () => {
  const v = applyGuard({
    userText: 'the ache moved to the bottom right',
    replyText: 'This needs looking at. Please see a doctor within the next two days.',
  });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.deepStrictEqual(v.timeframeStripped, ['within the next two days']);
  assert.strictEqual(/two days/i.test(v.displayText), false);
  assert.ok(v.displayText.endsWith(TIME_FRAME_NOTE), v.displayText);
  assert.strictEqual(v.displayText.split(TIME_FRAME_NOTE).length, 2);
  assert.strictEqual(v.timeframeUnlocated, false, 'it was located, so the fallback stays off');
});

// The invariant the widening buys, pinned so a later URGENCY change cannot take
// it back silently: on the phrasings the scorer counts, the strip finds one.
test('every phrasing the scorer counts as a stated time frame is one the strip can locate', () => {
  const phrasings = [
    'See your GP within the next 48 hours.', 'See your GP within the next two days.',
    'See your GP within three days.', 'See your GP within 2-3 days.',
    'See your GP within two-three days.', 'See your GP within one week.',
    'See your GP today.', 'See your GP tomorrow.', 'See your GP this week.',
    'See your GP as soon as possible.', 'Book an appointment ASAP.',
    'See your GP promptly.', 'See your GP urgently.', 'Ask for a same-day appointment.',
  ];
  for (const reply of phrasings) {
    assert.strictEqual(applyGuard({ replyText: reply }).timeframeUnlocated, false, reply);
    assert.ok(stripTimeFrames(reply).stripped.length > 0, reply);
  }
});

// ── timeframeUnlocated: the belt to the strip's braces ──────────────────────

test('timeframeUnlocated is false on every ordinary route', () => {
  for (const replyText of [
    'Call 999 now. Do not drive yourself.',
    'Call an ambulance straight away. Do not drive yourself.',
    'Please see your GP within the next 48 hours.',
    'This needs looking at. Please see your GP so they can examine you.',
    'Rest and drink fluids. If it has not settled within 3 days, see your GP.',
    'Hmm.',
    '',
  ]) {
    assert.strictEqual(applyGuard({ userText: 'sore throat', replyText }).timeframeUnlocated, false, JSON.stringify(replyText));
  }
});

test('a stated time frame the strip cannot locate collapses the reply to the note alone', () => {
  // Reachable without contriving the reply: the prohibited filter deletes the
  // sentence carrying "today" before the strip ever runs, so the scorer sees a
  // stated time frame on the raw reply and the strip finds none in what is left.
  const reply = 'Take some ibuprofen today. Please see your GP so they can examine you.';
  const v = applyGuard({ userText: 'my head hurts', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.banner, 'clinician');
  assert.strictEqual(v.timeframeUnlocated, true);
  assert.deepStrictEqual(v.timeframeStripped, []);
  assert.strictEqual(v.displayText, TIME_FRAME_NOTE.trim(), 'the model sentence is not shown at all');
  assert.strictEqual(v.rawReply, reply, 'and the log still has every word of it');
});

test('the fallback needs all three conditions: CLINICIAN, a stated time frame, and nothing stripped', () => {
  // EMERGENCY with a stated time frame: route is wrong for it, so it stays off
  // and the reply is shown in full.
  const emergency = applyGuard({ userText: 'chest pain', replyText: 'Call an ambulance right away.' });
  assert.strictEqual(emergency.route, 'EMERGENCY');
  assert.strictEqual(emergency.timeframeUnlocated, false);
  assert.strictEqual(emergency.displayText, 'Call an ambulance right away.');

  // CLINICIAN with no stated time frame at all: nothing to be unlocated.
  const quiet = applyGuard({ userText: 'stomach ache', replyText: 'Please see your GP so they can examine you.' });
  assert.strictEqual(quiet.route, 'CLINICIAN');
  assert.strictEqual(quiet.timeframeUnlocated, false);
  assert.strictEqual(quiet.displayText, 'Please see your GP so they can examine you.');

  // CLINICIAN with a stated time frame the strip did locate.
  const located = applyGuard({ userText: 'stomach ache', replyText: 'Please see your GP within three days.' });
  assert.strictEqual(located.timeframeUnlocated, false);
  assert.ok(located.displayText.endsWith(TIME_FRAME_NOTE));
});

test('the other numeric slot is widened too: "in the next twelve" is located, not left standing', () => {
  // URGENCY has two \d+ slots and both are widened. This one is the `in the next
  // N` branch, which carries no unit of its own — so the strip leaves the unit
  // orphaned ("... your GP hours."). That is URGENCY's shape, not this module's,
  // it predates the widening on the digit form, and it is written up in the
  // report. What is asserted here is the safety property: the time frame is
  // located, listed as written, and gone from the screen.
  for (const [reply, phrase] of [
    ['Please see your GP in the next twelve hours.', 'in the next twelve'],
    ['Please see your GP in the next 12 hours.', 'in the next 12'],
  ]) {
    const v = applyGuard({ userText: 'stomach ache', replyText: reply });
    assert.deepStrictEqual(v.timeframeStripped, [phrase], reply);
    assert.strictEqual(/in the next/i.test(v.displayText), false, reply);
    assert.strictEqual(v.timeframeUnlocated, false, reply);
    assert.strictEqual(v.rawReply, reply);
  }
});
