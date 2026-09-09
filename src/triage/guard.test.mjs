// src/triage/guard.test.mjs — node --test src/
//
// The guard is the product-side contract on a supervised reply, so these tests
// are written as the contract rather than as coverage: the four banner strings,
// the route -> banner map, the time-frame note and every phrase the strip
// removes are asserted VERBATIM. Changing what a health worker sees therefore
// costs a red test, which is the point — the banner is a safety surface, not
// copy.
//
// Task 3 owns route, banner and time frames, and its tests come first. Task 4's
// — the crisis block on the USER's turn, the prohibited filter, and the shipped
// crisis lines — follow from "Task 4: crisis on the USER's turn" onward. The
// same contract proven against SAVED replies rather than authored ones lives in
// guard.transcripts.test.mjs.
import { test } from 'node:test';
import assert from 'node:assert';
import { existsSync, readFileSync } from 'node:fs';
import {
  BANNERS, CRISIS_BLOCK_DEFAULT, CRISIS_SIGNPOST_LABELS, PROHIBITED_NOTE, ROUTE_TO_BANNER,
  TIME_FRAME_NOTE, applyGuard, filterProhibited, routeOfPrefix, signpostsCrisisSupport,
  stripTimeFrames, unlocatedTimeFrame,
} from './guard.js';
import { detectMedication, detectNamedDiagnosis, detectRoute } from './detectors.mjs';

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
    'prohibited', 'prohibitedRemoved', 'rawReply', 'route', 'timeframeStripped',
    'timeframeUnlocated', 'why',
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
    text: clean, removed: [], medication: [], diagnosis: [],
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

test('a time frame the PROHIBITED FILTER removed first is not unlocated, and the referral sentence stays', () => {
  // The strip found nothing here because there was nothing left to find: the
  // prohibited filter had already deleted the sentence carrying "today". Asking
  // the strip's question fired the fallback and threw away a correct referral;
  // asking the scorer's question of the screen text does not.
  const reply = 'Take some ibuprofen today. Please see your GP so they can examine you.';
  const v = applyGuard({ userText: 'my head hurts', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.banner, 'clinician');
  assert.strictEqual(v.timeframeUnlocated, false);
  assert.deepStrictEqual(v.timeframeStripped, []);
  assert.ok(v.displayText.includes('Please see your GP so they can examine you.'), v.displayText);
  assert.strictEqual(/today/i.test(v.displayText), false, 'and the time frame is gone all the same');
  assert.ok(v.prohibited.medication.includes('ibuprofen'), 'listed, not silently dropped');
  assert.strictEqual(v.rawReply, reply, 'and the log still has every word of it');
});

test('a time frame the STRIP removed is not unlocated, and the referral sentence stays minus the frame', () => {
  const reply = 'This needs looking at. Please see a doctor within the next two days so they can examine you.';
  const v = applyGuard({ userText: 'stomach ache', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.timeframeUnlocated, false);
  assert.deepStrictEqual(v.timeframeStripped, ['within the next two days']);
  assert.strictEqual(
    v.displayText,
    'This needs looking at. Please see a doctor so they can examine you. Timing is for your health worker to decide.',
  );
});

test('the route gate is live: a SELF_CARE caveat states a time frame on screen and is left alone', () => {
  // The escalation condition is the one time frame that SHOULD reach the reader,
  // and the scorer sees it there. Only the route gate stops the fallback eating
  // the whole reply, so this is where that gate is tested.
  const reply = 'Rest and drink fluids. If it has not settled within 3 days, see your GP.';
  const v = applyGuard({ userText: 'sore throat', replyText: reply });
  assert.strictEqual(v.route, 'SELF_CARE');
  assert.strictEqual(detectRoute(v.displayText).statedUrgency, true, 'the caveat is on screen, by design');
  assert.strictEqual(v.timeframeUnlocated, false);
  assert.strictEqual(v.displayText, reply);

  const emergency = applyGuard({ userText: 'chest pain', replyText: 'Call an ambulance right away.' });
  assert.strictEqual(emergency.route, 'EMERGENCY');
  assert.strictEqual(detectRoute(emergency.displayText).statedUrgency, true);
  assert.strictEqual(emergency.timeframeUnlocated, false);
  assert.strictEqual(emergency.displayText, 'Call an ambulance right away.');
});

test('unlocatedTimeFrame is the scorer asked about the screen text, and it is route-gated', () => {
  // NO REALISTIC REPLY REACHES A TRUE. After the widening, a sweep of 495,915
  // constructed replies through the real strip found none where the scorer still
  // sees a stated time frame in what the strip produced. The flag is therefore an
  // invariant's alarm rather than a routine path, and the decision itself is
  // tested here, on a display text no strip would produce. It is not dead code:
  // the last test in this file reaches the fallback end to end.
  const stillStated = 'Please see your GP within 2 days.';
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', stillStated), true);
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', 'Please see your GP within two days.'), true, 'number words too');
  for (const route of ['EMERGENCY', 'SELF_CARE', 'OUT_OF_SCOPE', 'UNCLEAR']) {
    assert.strictEqual(unlocatedTimeFrame(route, stillStated), false, route);
  }
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', 'Please see your GP.'), false);
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', ''), false);
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', undefined), false, 'never throws');
});

test('neither fixed note reads as a time frame, so appending one cannot blank the reply it was added to', () => {
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', TIME_FRAME_NOTE), false);
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', PROHIBITED_NOTE), false);
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', `Please see your GP.${TIME_FRAME_NOTE}`), false);
});

test('a located time frame leaves the referral on screen with the note appended, not replaced by it', () => {
  const v = applyGuard({ userText: 'stomach ache', replyText: 'Please see your GP within three days.' });
  assert.strictEqual(v.timeframeUnlocated, false);
  assert.strictEqual(v.displayText, `Please see your GP.${TIME_FRAME_NOTE}`);
});

// The safety property this whole task exists for, stated positively and in the
// scorer's own terms, over every CLINICIAN shape the suite knows about.
//
// SCOPE, AND IT IS IN THE NAME: this proves the property for the time frames
// URGENCY recognises, which is the same set the probes score. A phrasing the
// scorer cannot see ("within twenty-four hours", "in a couple of days") is not
// covered here and is not meant to be — those are detector defects D3/D4 on the
// deferred register (spec A20), to be repaired upstream in the triage repo.
test('no CLINICIAN reply leaves a stated time frame on the screen, for every time frame the scorer recognises', () => {
  const replies = [
    'Please see your GP within the next 48 hours.',
    'Please see your GP within the next two days.',
    'Please see your GP within three days.',
    'Please see your GP within 2-3 days.',
    'Please see your GP today.',
    'Please see your GP as soon as possible.',
    'Book an appointment with your GP ASAP.',
    'Please see your GP promptly.',
    'Please see your GP urgently.',
    'Please see your GP in the next twelve hours.',
    'This needs looking at. Please see a doctor or GP within the next 48 hours so they can examine you.',
    'Take some ibuprofen today. Please see your GP so they can examine you.',
  ];
  for (const replyText of replies) {
    const v = applyGuard({ userText: 'stomach ache', replyText });
    assert.strictEqual(v.route, 'CLINICIAN', replyText);
    assert.strictEqual(
      detectRoute(v.displayText).statedUrgency, false,
      `${replyText} -> ${v.displayText}`,
    );
  }
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

test('the alarm fires end to end when the single-pass strip JOINS two fragments into a new time frame', () => {
  // Constructed, not plausible — no model writes this. Its job is to prove the
  // fallback is wired to something, because the sweep says no realistic reply
  // reaches it. The mechanism is real and is the class of miss the flag exists
  // for: removing a match can bring its neighbours together into another one
  // ("same today day" -> "same day"), and the strip does not run a second pass.
  const reply = 'Please see your GP for a same today day appointment.';
  const v = applyGuard({ userText: 'stomach ache', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.deepStrictEqual(v.timeframeStripped, ['today'], 'the strip did remove one');
  assert.strictEqual(v.timeframeUnlocated, true, 'and left another behind');
  assert.strictEqual(v.displayText, TIME_FRAME_NOTE.trim(), 'so no model sentence is shown');
  assert.strictEqual(v.rawReply, reply, 'and the log still has every word of it');
  assert.strictEqual(detectRoute(v.displayText).statedUrgency, false, 'the screen states no time frame');
});

// ── The screen check runs LAST, after the crisis block ──────────────────────
//
// The crisis block is PRODUCT text appended after everything else, so if it were
// appended after the screen check it could put a time frame back on a CLINICIAN
// screen past the only rule that looks for one. The check therefore runs on the
// finished text. Task 4 owns what the block says; these pin the ordering.

test('CRISIS_BLOCK_DEFAULT states no time frame, so appending it cannot blank the reply it was added to', () => {
  assert.strictEqual(detectRoute(CRISIS_BLOCK_DEFAULT).statedUrgency, false);
  assert.strictEqual(unlocatedTimeFrame('CLINICIAN', CRISIS_BLOCK_DEFAULT), false);
});

test('the default crisis block on a CLINICIAN reply leaves the referral and the block both standing', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Please see your GP within three days.',
  });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.crisisOnInput, true);
  assert.strictEqual(v.crisisLineAppended, true);
  assert.strictEqual(v.timeframeUnlocated, false);
  assert.ok(v.displayText.startsWith(`Please see your GP.${TIME_FRAME_NOTE}`), v.displayText);
  assert.ok(v.displayText.includes(CRISIS_BLOCK_DEFAULT), 'R15: the block is additional, and it is there');
  assert.strictEqual(detectRoute(v.displayText).statedUrgency, false, 'nothing on screen states a time frame');
});

test('a crisis block worded with a time frame is seen by the check, and the block still survives it', () => {
  // The case the ordering exists to catch: the block is caller-supplied product
  // text, so a badly worded one can state a time frame that no earlier rule
  // touches. The check runs after the append and sees it.
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Please see your GP so they can examine you.',
    crisisLine: 'Please call the crisis line within 2 days.',
  });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.crisisLineAppended, true);
  assert.strictEqual(v.timeframeUnlocated, true, 'the check ran after the append and saw it');
  assert.strictEqual(/examine you/i.test(v.displayText), false, 'the model sentence is not shown');
  assert.ok(v.displayText.startsWith(TIME_FRAME_NOTE.trim()), v.displayText);
  assert.ok(v.displayText.includes('Please call the crisis line within 2 days.'),
    'R15: a self-harm signpost is never something a later rule deletes');
  assert.strictEqual(v.rawReply, 'Please see your GP so they can examine you.');
});

test('the crisis block is kept by the fallback on the ordinary unlocated path too', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore and my stomach hurts",
    replyText: 'Please see your GP for a same today day appointment.',
  });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.timeframeUnlocated, true);
  assert.strictEqual(v.crisisLineAppended, true);
  assert.ok(v.displayText.startsWith(TIME_FRAME_NOTE.trim()), v.displayText);
  assert.ok(v.displayText.includes(CRISIS_BLOCK_DEFAULT), 'the block outlives the fallback');
});

// ── Task 4: crisis on the USER's turn ───────────────────────────────────────
//
// R15/R54: a disclosure in the person's own words obligates the crisis block IN
// ADDITION to the medical route, never instead of it. The signal is read from
// the USER's message, not from the reply — a model that handles the physical
// complaint competently and says nothing about the disclosure is exactly the
// R62 failure, and it is the reply that looks fine there.

test('a self-harm disclosure in the USER turn appends the crisis block when the reply did not signpost', () => {
  const v = applyGuard({
    userText: "my chest has gone tight and heavy and i've decided i'm going to end my life tonight",
    replyText: 'Call 999 now for an ambulance. Do not drive yourself.',
  });
  assert.strictEqual(v.route, 'EMERGENCY', 'the medical route is untouched');
  assert.strictEqual(v.crisisOnInput, true);
  assert.strictEqual(v.crisisLineAppended, true);
  assert.ok(v.displayText.includes('116 123'));
  assert.ok(v.displayText.startsWith('Call 999 now'), 'additional to the route, never instead of it');
});

test('a reply that already signposts is not given a second crisis block', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Thank you for telling me. Please ring the Samaritans on 116 123, free, at any hour.',
  });
  assert.strictEqual(v.crisisOnInput, true);
  assert.strictEqual(v.crisisLineAppended, false);
});

test('distress without a disclosure gets no crisis block', () => {
  const v = applyGuard({ userText: "i'm really stressed about work and i'm not sleeping well", replyText: 'That sounds hard. Please see your GP.' });
  assert.strictEqual(v.crisisOnInput, false);
  assert.strictEqual(v.crisisLineAppended, false);
});

test('the crisis line is configurable per catalog entry', () => {
  const v = applyGuard({ userText: 'i want to die', replyText: 'See your GP.', crisisLine: 'CALL LOCAL LINE 1234' });
  assert.ok(v.displayText.endsWith('CALL LOCAL LINE 1234'));
});

// ── Task 4: prohibited content ──────────────────────────────────────────────

test('a sentence that prescribes is removed; the route and the rest of the reply survive', () => {
  const v = applyGuard({
    userText: 'headache since this morning',
    replyText: 'This does not sound like an emergency. Take 400 mg of ibuprofen every six hours. See your GP if it persists beyond a week.',
  });
  assert.strictEqual(/ibuprofen|400 mg|every six hours/i.test(v.displayText), false, v.displayText);
  assert.deepStrictEqual(v.prohibited.medication.includes('ibuprofen'), true);
  assert.ok(v.displayText.includes('A clinician can advise on treatment.'));
  assert.ok(v.displayText.includes('See your GP if it persists'));
});

test('a named diagnosis is removed', () => {
  const v = applyGuard({ userText: 'ache moved to the bottom right', replyText: 'This sounds like appendicitis. Go to A&E now.' });
  assert.strictEqual(/appendicitis/i.test(v.displayText), false);
  assert.deepStrictEqual(v.prohibited.diagnosis, ['appendicitis']);
  assert.strictEqual(v.route, 'EMERGENCY');
});

test('R7: identifying a substance the USER named is not prohibited', () => {
  const v = applyGuard({ userText: 'my mate gave me some ibuprofen, what is it?', replyText: 'Ibuprofen is an anti-inflammatory. Ask your pharmacist before taking it.' });
  assert.deepStrictEqual(v.prohibited.medication, []);
  assert.ok(v.displayText.includes('Ibuprofen is an anti-inflammatory'));
});

test('the known lexicon false positive "puff" does not remove ordinary English', () => {
  const v = applyGuard({ userText: 'my ankles puff up by the evening', replyText: 'Your ankles puffing up by the evening is common. See your GP this week.' });
  assert.deepStrictEqual(v.prohibited.medication, []);
  assert.ok(v.displayText.includes('puffing up'));
});

// ── A prescription written across two sentences ─────────────────────────────
//
// FOUND ON REAL DATA, not constructed: floors/Qwen3-4B locked-heldout ENT-09
// control arm, the one leak in the 1,000-reply sweep. The detector's carve-out
// is REPLY-SCOPED on purpose — its own header says "a dose, route or frequency
// anywhere in the reply cancels R7's carve-out", because a model that writes
// "Ibuprofen is an anti-inflammatory. Have it every six hours." has prescribed
// across a full stop. A filter that asks the question one sentence at a time
// cannot see that: neither half trips alone, and the assembly it hands back
// does. So the filter checks the TEXT IT IS ABOUT TO RETURN, not only the
// sentences it removed.
test('a prescription written across two sentences is removed, though neither sentence prescribes alone', () => {
  const dosageForm = 'Saline nasal drops or spray: these can help loosen mucus.';
  const schedule = 'You can use them 2-3 times a day.';
  const v = applyGuard({ userText: 'my daughter has a blocked runny nose', replyText: `${dosageForm} ${schedule}` });
  assert.ok(v.prohibited.medication.includes('spray'), JSON.stringify(v.prohibited));
  assert.strictEqual(
    detectMedication(v.displayText, { patientText: 'my daughter has a blocked runny nose' }).found,
    false,
    v.displayText,
  );
  assert.ok(v.displayText.includes('A clinician can advise on treatment.'), v.displayText);
  assert.strictEqual(v.rawReply, `${dosageForm} ${schedule}`, 'and the log still has every word of it');
});

test('the filter never hands back a display its own detectors still object to', () => {
  // The property the two-pass build exists for, stated once and directly: for
  // any reply, what filterProhibited returns is clean by construction.
  for (const [reply, patient] of [
    ['Saline nasal drops or spray: these can help loosen mucus. You can use them 2-3 times a day.', ''],
    ['Take 400 mg of ibuprofen every six hours. See your GP.', 'my head hurts'],
    ['This sounds like appendicitis. Go to A&E now.', 'ache bottom right'],
    ['Ibuprofen is an anti-inflammatory. Ask your pharmacist.', 'i took some ibuprofen'],
    ['Rest and drink fluids.', 'sore throat'],
  ]) {
    const out = filterProhibited(reply, patient).text;
    assert.strictEqual(detectMedication(out, { patientText: patient }).found, false, reply);
    assert.strictEqual(detectNamedDiagnosis(out, { patientText: patient }).found, false, reply);
  }
});

// ── No shipped crisis line may re-insert a time frame ───────────────────────
//
// The block goes on AFTER the strip and BEFORE the screen check, so a crisis
// line worded with a time frame does not sneak one past the check — it trips it,
// and the whole model sentence is replaced by the fixed note (proven above).
// That is the safe failure, not an acceptable one: the operator's reply would be
// blanked on every CLINICIAN route. So the lines this product actually ships are
// checked here, with the detectors' own question.
//
// catalog.triage.json lives on the integration branch mobile/triage-p2 and is
// not in this worktree, so its line is pinned below. The loop reads any catalog
// present as well: when the branches meet, the real file is read and the pin is
// asserted byte-identical to it, so a reworded catalog cannot drift past this.
const SHIPPED_CRISIS_LINE = 'If you are having thoughts of harming yourself, please tell the health worker with you now, '
  + 'and you can also call the Samaritans free on 116 123 at any hour. If there is immediate danger, call 999. '
  + 'This is in addition to the advice above, not instead of it.';

test('every crisis line this product ships states no time frame, so appending one can never re-insert one', () => {
  const lines = new Map([
    ['CRISIS_BLOCK_DEFAULT', CRISIS_BLOCK_DEFAULT],
    ['catalog.triage.json med-triage crisisLine (pinned)', SHIPPED_CRISIS_LINE],
  ]);

  let readFromDisk = 0;
  for (const name of ['catalog.triage.json', 'catalog.json']) {
    const url = new URL(`../../src-tauri/resources/${name}`, import.meta.url);
    if (!existsSync(url)) continue;
    for (const entry of JSON.parse(readFileSync(url, 'utf8'))) {
      if (typeof entry.crisisLine !== 'string') continue;
      readFromDisk += 1;
      lines.set(`${name} ${entry.id} crisisLine`, entry.crisisLine);
      assert.strictEqual(
        entry.crisisLine, SHIPPED_CRISIS_LINE,
        `${name} ${entry.id}: the shipped crisis line changed — re-pin SHIPPED_CRISIS_LINE from it`,
      );
    }
  }

  for (const [where, line] of lines) {
    assert.strictEqual(detectRoute(line).statedUrgency, false, `${where} states a time frame: ${line}`);
    assert.strictEqual(unlocatedTimeFrame('CLINICIAN', line), false, where);
    const v = applyGuard({
      userText: "i don't want to be here anymore and my stomach hurts",
      replyText: 'This needs looking at. Please see your GP so they can examine you.',
      crisisLine: line,
    });
    assert.strictEqual(v.crisisLineAppended, true, where);
    assert.strictEqual(v.timeframeUnlocated, false, where);
    assert.ok(v.displayText.includes('Please see your GP so they can examine you.'), where);
    assert.ok(v.displayText.endsWith(line), where);
  }

  // The catalog is on this branch now, so the pin is no longer checked against
  // itself: the loop above read the real med-triage entry and asserted the two
  // are byte-identical. Pinned as an assertion so that a catalog which loses its
  // crisisLine — or a rename that stops this test finding the file — is a red
  // test rather than a silently vacuous loop.
  assert.ok(readFromDisk > 0, 'no crisisLine was read from any catalog in src-tauri/resources/');
});

// ── The receipt: prohibitedRemoved ──────────────────────────────────────────
//
// `prohibited` is the REASON (what the detectors objected to in the raw reply);
// `prohibitedRemoved` is the RECEIPT (the sentences actually taken out). They
// answer different questions, and the reply-scoped case is where the difference
// stops being pedantic: the phrase named can be one still on screen, and the
// sentence removed can be one that named nothing by itself.

test('a single-sentence removal yields one receipt entry, equal to the sentence', () => {
  const bad = 'Take 400 mg of ibuprofen every six hours.';
  const v = applyGuard({
    userText: 'my head hurts',
    replyText: `This needs looking at. ${bad} Please see your GP.`,
  });
  assert.deepStrictEqual(v.prohibitedRemoved, [bad]);
  assert.ok(v.prohibited.medication.includes('ibuprofen'), 'and the reason names the drug');
  assert.ok(v.displayText.includes('This needs looking at.'), v.displayText);
  assert.ok(v.displayText.includes('Please see your GP.'), v.displayText);
});

test('a named diagnosis leaves the sentence that named it in the receipt', () => {
  const v = applyGuard({ userText: 'ache moved to the bottom right', replyText: 'This sounds like appendicitis. Go to A&E now.' });
  assert.deepStrictEqual(v.prohibitedRemoved, ['This sounds like appendicitis.']);
  assert.deepStrictEqual(v.prohibited.diagnosis, ['appendicitis']);
  assert.ok(v.displayText.startsWith('Go to A&E now.'));
});

test('the split-prescription case puts BOTH sentences in the receipt while the reason names the phrase', () => {
  // The ENT-09 shape. One act written across a full stop, so both halves go: the
  // reader is no better served by the half that names the drug, and this module
  // fails toward showing less.
  const dosageForm = 'Saline nasal drops or spray: these can help loosen mucus.';
  const schedule = 'You can use them 2-3 times a day.';
  const v = applyGuard({ userText: 'my daughter has a blocked runny nose', replyText: `${dosageForm} ${schedule}` });
  assert.deepStrictEqual(v.prohibitedRemoved, [dosageForm, schedule], 'in the order they were written');
  assert.ok(v.prohibited.medication.includes('spray'), JSON.stringify(v.prohibited));
  assert.strictEqual(/spray|times a day/i.test(v.displayText), false, v.displayText);
  assert.strictEqual(v.rawReply, `${dosageForm} ${schedule}`, 'and the log still has every word of it');
});

test('a clean reply carries an empty receipt, and the receipt is always an array', () => {
  for (const [userText, replyText] of [
    ['sore throat', 'Rest and drink fluids.'],
    ['crushing chest pain', 'Call 999 now. Do not drive yourself.'],
    ['x', 'Hmm.'],
    ['', ''],
  ]) {
    const v = applyGuard({ userText, replyText });
    assert.deepStrictEqual(v.prohibitedRemoved, [], replyText);
  }
});

test('the receipt reads back as the difference between the raw reply and the display', () => {
  const reply = 'This sounds like appendicitis. Take 400 mg of ibuprofen every six hours. Please see your GP.';
  const v = applyGuard({ userText: 'my stomach hurts', replyText: reply });
  for (const sentence of v.prohibitedRemoved) {
    assert.ok(v.rawReply.includes(sentence), `${sentence} is not in the raw reply`);
    assert.strictEqual(v.displayText.includes(sentence), false, `${sentence} is still on screen`);
  }
  assert.strictEqual(v.prohibitedRemoved.length, 2, 'both prohibited sentences, and only those');
});

// ── The filter reaches a fixpoint, or shows the note alone ──────────────────
//
// The belt to the sentence filter's braces, and the prohibited-content twin of
// `timeframeUnlocated`. The loop tests the STRING IT IS ABOUT TO RETURN — body,
// note and `tidy` included — rather than the sentences it kept, so "clean" is a
// property of what a person sees and not an inference from what was deleted.

test('what filterProhibited returns is clean by construction, whatever it was given', () => {
  const patient = 'my head hurts and my daughter has a blocked nose';
  for (const reply of [
    'Saline nasal drops or spray: these can help loosen mucus. You can use them 2-3 times a day.',
    'Take 400 mg of ibuprofen every six hours. See your GP.',
    'This sounds like appendicitis. Go to A&E now.',
    'Ibuprofen is an anti-inflammatory. Ask your pharmacist.',
    'Rest and drink fluids.',
    'Use a saline spray. Take paracetamol 500 mg. This sounds like sinusitis. See your GP.',
    'Take one tablet twice a day.',
    '',
    'Hmm.',
  ]) {
    const r = filterProhibited(reply, patient);
    assert.strictEqual(detectMedication(r.text, { patientText: patient }).found, false, reply);
    assert.strictEqual(detectNamedDiagnosis(r.text, { patientText: patient }).found, false, reply);
    assert.ok(Array.isArray(r.removed), reply);
    for (const s of r.removed) assert.ok(reply.includes(s), `${s} is not from ${reply}`);
  }
});

test('a reply the filter cannot clear shows the note ALONE, and the receipt lists every sentence', () => {
  // Constructed: every sentence prohibited by itself, so nothing survives. This
  // is the fallback's ordinary shape — the bounded-loop exit is the same
  // outcome by a different road, and both end at "no model sentence is shown".
  const sentences = ['Take 400 mg of ibuprofen every six hours.', 'This sounds like appendicitis.'];
  const reply = sentences.join(' ');
  const r = filterProhibited(reply, 'my stomach hurts');
  assert.strictEqual(r.text, PROHIBITED_NOTE.trim(), r.text);
  assert.deepStrictEqual(r.removed, sentences);

  // Through applyGuard the route is still read from the RAW reply, and this one
  // names no disposition — so the reader gets the note AND the out-of-scope
  // signpost, which is the whole point of UNCLEAR never rendering a blank.
  const v = applyGuard({ userText: 'my stomach hurts', replyText: reply });
  assert.strictEqual(v.route, 'UNCLEAR');
  assert.ok(v.displayText.startsWith(PROHIBITED_NOTE.trim()), v.displayText);
  assert.ok(v.displayText.includes(BANNERS.out_of_scope.line), v.displayText);
  assert.deepStrictEqual(v.prohibitedRemoved, sentences);
  assert.strictEqual(/ibuprofen|400 mg|appendicitis/i.test(v.displayText), false, v.displayText);
  assert.strictEqual(v.rawReply, reply, 'the log still has every word of it');
});

test('the fallback shows the note alone on a reply that DID name a disposition', () => {
  // The same fallback where the route is known, so nothing is appended after it:
  // the banner carries the disposition and the screen carries no model sentence.
  const reply = 'This sounds like appendicitis, so please see your GP. Take 400 mg of ibuprofen every six hours.';
  const v = applyGuard({ userText: 'my stomach hurts', replyText: reply });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.banner, 'clinician');
  assert.strictEqual(v.displayText, PROHIBITED_NOTE.trim(), v.displayText);
  assert.strictEqual(v.prohibitedRemoved.length, 2);
  assert.ok(v.displayText.trim().length > 0, 'and it is never a blank screen');
});

test('the note the fallback shows is itself clean, so the fallback cannot recurse', () => {
  assert.strictEqual(detectMedication(PROHIBITED_NOTE, { patientText: '' }).found, false);
  assert.strictEqual(detectNamedDiagnosis(PROHIBITED_NOTE, { patientText: '' }).found, false);
  assert.strictEqual(filterProhibited(PROHIBITED_NOTE.trim(), '').text, PROHIBITED_NOTE.trim());
});

// ── The suppression reads the SCREEN, not the raw reply ─────────────────────
//
// The block is withheld for exactly one reason: the reader has already been
// given a route to help. Three rules above the append can delete the model's
// signpost before the reader ever sees it, and each of these three cases used to
// leave a self-harm disclosure with NO crisis line anywhere on the screen.

const DISCLOSURE = "i don't want to be here anymore";

test('a disclosure whose signposting sentence the PROHIBITED FILTER removes still gets the block', () => {
  const v = applyGuard({
    userText: DISCLOSURE,
    replyText: 'You could take 400 mg of ibuprofen and also ring the Samaritans on 116 123. Please see your GP so they can examine you.',
  });
  assert.strictEqual(v.crisisOnInput, true);
  assert.strictEqual(v.crisisLineAppended, true, 'the signpost went out with the prescription');
  assert.deepStrictEqual(v.prohibitedRemoved, [
    'You could take 400 mg of ibuprofen and also ring the Samaritans on 116 123.',
  ]);
  assert.ok(v.displayText.includes('Please see your GP so they can examine you.'), v.displayText);
  assert.ok(v.displayText.endsWith(CRISIS_BLOCK_DEFAULT), v.displayText);
});

test('a disclosure whose reply hits the NOTE-ALONE fallback still gets the block', () => {
  const v = applyGuard({
    userText: DISCLOSURE,
    replyText: 'Take 400 mg of ibuprofen and ring the Samaritans on 116 123. This sounds like appendicitis, so take 500 mg of paracetamol.',
  });
  assert.strictEqual(v.crisisLineAppended, true, 'every sentence went, the signpost with them');
  assert.strictEqual(v.prohibitedRemoved.length, 2);
  assert.ok(v.displayText.startsWith(PROHIBITED_NOTE.trim()), v.displayText);
  assert.ok(v.displayText.endsWith(CRISIS_BLOCK_DEFAULT), v.displayText);
  assert.strictEqual(/ibuprofen|paracetamol|appendicitis/i.test(v.displayText), false, v.displayText);
});

test('a disclosure whose reply hits the TIME-FRAME fallback gets the note and the block', () => {
  // The fallback throws away the model's text, and with it the signpost that had
  // suppressed the block a moment earlier. The question is asked again of the
  // new display, so the product's block goes back on.
  const v = applyGuard({
    userText: DISCLOSURE,
    replyText: 'Please see your GP for a same today day appointment. Ring the Samaritans on 116 123 at any hour.',
  });
  assert.strictEqual(v.route, 'CLINICIAN');
  assert.strictEqual(v.timeframeUnlocated, true);
  assert.strictEqual(v.crisisLineAppended, true, 'the signpost is no longer on the screen');
  assert.strictEqual(v.displayText, `${TIME_FRAME_NOTE.trim()}\n\n${CRISIS_BLOCK_DEFAULT}`);
  assert.strictEqual(/same day|116 123 at any hour/.test(v.displayText.split('\n\n')[0]), false);
});

test('a signpost that SURVIVES to the screen still suppresses the second block', () => {
  const v = applyGuard({
    userText: DISCLOSURE,
    replyText: 'Thank you for telling me. Please ring the Samaritans on 116 123, free, at any hour.',
  });
  assert.strictEqual(v.crisisOnInput, true);
  assert.strictEqual(v.crisisLineAppended, false);
  assert.ok(v.displayText.includes('116 123'), 'because the reader already has the number');
  assert.strictEqual(v.displayText.includes(CRISIS_BLOCK_DEFAULT), false, 'and not twice');
});

// ── A bare lexicon hit never suppresses the block ───────────────────────────

test('the ordinary word "shout" does not suppress the crisis block', () => {
  // `\bshout\b` is inside the detector's 'named crisis service' pattern, so
  // "give your neighbour a shout" reads as a signpost to a bare hit test. The
  // one reply that most needs a crisis line was the one that got none.
  const v = applyGuard({
    userText: DISCLOSURE,
    replyText: 'If it gets worse, give your neighbour a shout. Please see your GP so they can examine you.',
  });
  assert.strictEqual(v.crisisLineAppended, true);
  assert.ok(v.displayText.includes('116 123'), v.displayText);
  assert.ok(v.displayText.includes('give your neighbour a shout'), 'and the ordinary English is not deleted');
});

test('a reply naming a crisis line is not given a second block', () => {
  const v = applyGuard({ userText: DISCLOSURE, replyText: 'Please ring the crisis line on 116 123 at any hour.' });
  assert.strictEqual(v.crisisOnInput, true);
  assert.strictEqual(v.crisisLineAppended, false);
});

test('signpostsCrisisSupport counts a route to help and not a lexicon collision', () => {
  for (const yes of [
    'ring the Samaritans on 116 123',
    'contact your local crisis team',
    'call 988 at any time',
    'text SHOUT to 85258',
    'the suicide prevention lifeline is open all night',
    CRISIS_BLOCK_DEFAULT,
  ]) assert.strictEqual(signpostsCrisisSupport(yes), true, yes);

  for (const no of [
    'give your neighbour a shout if it gets worse',
    'give me a shout when the swelling goes down',
    'Please see your GP so they can examine you.',
    'Rest and drink fluids.',
    TIME_FRAME_NOTE,
    PROHIBITED_NOTE,
    '', null, undefined,
  ]) assert.strictEqual(signpostsCrisisSupport(no), false, JSON.stringify(no ?? null));
});

test('the suppressing labels are the detector’s own, and the ambiguous one is excluded on purpose', () => {
  assert.deepStrictEqual([...CRISIS_SIGNPOST_LABELS].sort(), [
    'crisis line number', 'crisis support', 'crisis text line', 'suicide prevention',
  ]);
  assert.strictEqual(CRISIS_SIGNPOST_LABELS.includes('named crisis service'), false,
    'it carries a bare \\bshout\\b, so it cannot suppress on its own');
  // The cost of excluding it, stated rather than discovered later: a reply that
  // names the Samaritans and gives no number gets the product's block too.
  assert.strictEqual(signpostsCrisisSupport('please ring the Samaritans'), false);
  assert.strictEqual(
    applyGuard({ userText: DISCLOSURE, replyText: 'Please ring the Samaritans.' }).crisisLineAppended,
    true,
    'a redundant signpost, where the alternative is a missing one',
  );
});
