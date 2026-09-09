// src/triage/guard.js — the product-side contract on a supervised reply.
//
// The model routes; the PRODUCT decides what reaches the screen. Every rule
// here is one the probes already score (src/triage/detectors.mjs is the
// probes' own code, vendored), applied in the one place a finished reply is
// shown and persisted. Pure: no DOM, no Tauri, so `node --test src/` covers it
// against the saved transcripts (guard.transcripts.test.mjs).
//
// Precedence, and why it is this order:
//   1. route — detectRoute on the raw reply, before any rewrite, so the banner
//      reflects what the model said, not what we left of it;
//   2. prohibited content — sentences naming a medication, dose, route,
//      frequency or diagnosis are removed (fail toward showing less);
//   3. time frames — on a CLINICIAN route only, every URGENCY match is removed
//      (R64/R65: the stated time frame is what made a confident referral worse
//      than a decline). EMERGENCY keeps "now"; SELF_CARE keeps the caveat's
//      "within 3 days", which is the escalation condition, not a referral.
//      If a stated time frame survives all of that on the finished screen text,
//      the reply is replaced by the fixed note — see `unlocatedTimeFrame`;
//   4. crisis — if the USER's words disclose self-harm and the reply does not
//      signpost, the product's own crisis block is appended (R15: additional to
//      the route, never instead of it).
import {
  NUMBER_WORDS, ROUTE, URGENCY, detectCrisisResponse, detectCrisisStatement,
  detectMedication, detectNamedDiagnosis, detectRoute,
} from './detectors.mjs';
import pin from './detectors.pin.json' with { type: 'json' };

export const BANNERS = Object.freeze({
  emergency: { title: 'EMERGENCY', line: 'Emergency services now. Do not let them drive themselves.' },
  clinician: { title: 'SEE A CLINICIAN', line: 'Needs assessment. The health worker decides when.' },
  self_care: { title: 'SELF-CARE', line: 'Manage at home. Watch for the change that means seeking care.' },
  out_of_scope: { title: 'CANNOT JUDGE FROM THIS', line: 'The model has no basis to route this. Health worker to assess.' },
});

export const ROUTE_TO_BANNER = Object.freeze({
  [ROUTE.EMERGENCY]: 'emergency',
  [ROUTE.CLINICIAN]: 'clinician',
  [ROUTE.SELF_CARE]: 'self_care',
  [ROUTE.OUT_OF_SCOPE]: 'out_of_scope',
  [ROUTE.UNCLEAR]: 'out_of_scope',
});

export const TIME_FRAME_NOTE = ' Timing is for your health worker to decide.';
export const PROHIBITED_NOTE = ' A clinician can advise on treatment.';
export const CRISIS_BLOCK_DEFAULT =
  'If you are having thoughts of harming yourself, please tell the health worker with you now, '
  + 'and you can also call the Samaritans free on 116 123 at any hour. If there is immediate danger, call 999. '
  + 'This is in addition to the advice above, not instead of it.';

// URGENCY is defined without the global flag; a global copy is needed to
// remove every match, not the first. The `i` is needed too: the detectors only
// ever apply URGENCY to `normaliseReply`'d (lower-cased) text, and the guard
// applies it to the reply AS WRITTEN, where "Today" is the common spelling.
//
// THE SAME ASYMMETRY COSTS THE NUMERIC BRANCH ITS NUMBER WORDS, and that one is
// not cosmetic. By the time the detectors test URGENCY, `normaliseReply` has
// turned "two" into "2" and `\d+` sees it; the guard, matching what a person
// will actually read, does not. So "see your GP within the next two days" kept
// its stated time frame on a CLINICIAN route while the scorer counted one, and
// `timeframeStripped` came back empty — indistinguishable from a reply that
// stated no time frame at all. That is the R64/R65 failure with the alarm off.
//
// Widened from the detectors' OWN exported list rather than from a second word
// list written here: one definition of a number word, no fork. No entry in
// NUMBER_WORDS is a prefix of another, so the alternation order is the list's.
const NUMBER_WORD_ALT = NUMBER_WORDS.map(([word]) => word).join('|');
const URGENCY_G = new RegExp(
  URGENCY.source.replaceAll('\\d+', `(?:\\d+|${NUMBER_WORD_ALT})`),
  'gi',
);
const SENTENCES = /[^.!?]+[.!?]+|[^.!?]+$/g;

function tidy(text) {
  return text
    .replace(/\s+([,.;:!?])/g, '$1')   // "GP ." -> "GP."
    .replace(/,\s*([.!?])/g, '$1')     // "possible,." -> "possible."
    .replace(/\(\s*\)/g, '')
    .replace(/[ \t]{2,}/g, ' ')
    .replace(/\s+\n/g, '\n')
    .trim();
}

/**
 * Remove every stated time frame, matching the reply as written — digits or
 * number words. Returns the phrases removed, in order and in their original
 * spelling, so the verdict can list what a reader no longer sees.
 */
export function stripTimeFrames(text) {
  const stripped = [];
  const out = String(text).replace(URGENCY_G, (m) => { stripped.push(m); return ''; });
  return { text: tidy(out), stripped };
}

/**
 * Drop every sentence that names a medication (dose, route, frequency or an
 * introduced drug) or a diagnosis. Sentence-level on purpose: the detectors
 * report findings, not character offsets, and a whole sentence is the
 * smallest unit whose removal cannot leave half a prescription behind.
 */
export function filterProhibited(text, patientText = '') {
  const med = detectMedication(text, { patientText });
  const dx = detectNamedDiagnosis(text, { patientText });
  if (!med.found && !dx.found) return { text: String(text), medication: [], diagnosis: [] };
  const kept = [];
  let removed = 0;
  for (const sentence of String(text).match(SENTENCES) ?? []) {
    const s = sentence.trim();
    if (!s) continue;
    const bad = detectMedication(s, { patientText }).found || detectNamedDiagnosis(s, { patientText }).found;
    if (bad) removed += 1; else kept.push(s);
  }
  let out = kept.join(' ');
  if (removed) out = tidy(`${out}${PROHIBITED_NOTE}`);
  return {
    text: out,
    medication: [...med.drugs, ...med.classes, ...med.doses, ...med.routes, ...med.frequencies],
    diagnosis: dx.names,
  };
}

/** The route the streamed prefix resolves to, for the provisional banner. */
export function routeOfPrefix(text) {
  if (!String(text ?? '').trim()) return ROUTE.UNCLEAR;
  return detectRoute(text).route;
}

/**
 * Does the text about to be shown STILL state a time frame it should not?
 *
 * Asked in the scorer's terms, of the SCREEN's text. Not "did the strip remove
 * something" — that is a different question, and the difference is not academic:
 * the prohibited filter can delete the sentence carrying the time frame before
 * the strip ever sees it, and the strip's version of the question then answers
 * "nothing removed" and hides a correct referral behind the fixed note. This one
 * answers "nothing left", which is the property the rule actually protects.
 *
 * `statedUrgency` is `detectRoute`'s own URGENCY test over normalised text, so
 * the alarm cannot drift from what the probes count.
 *
 * Route-gated, because the two routes that keep a time frame keep it on purpose:
 * EMERGENCY's "now" IS the disposition, and SELF_CARE's "within 3 days" is the
 * escalation condition, the one time frame that should reach the reader.
 */
export function unlocatedTimeFrame(route, displayText) {
  return route === ROUTE.CLINICIAN && detectRoute(displayText).statedUrgency === true;
}

/**
 * Apply the product's contract to one finished reply.
 *
 * @returns {object} GuardVerdict, read by Task 4 (crisis), Task 6 (send path)
 *   and Task 7 (render). Every key, and what it is for:
 *
 *   route               the detectors' route for the RAW reply, decided before
 *                       anything is removed, so the banner says what was said
 *   why                 detectRoute's reason code, for the log
 *   banner              which of BANNERS to show; never absent, never blank
 *   displayText         what reaches the screen
 *   rawReply            what the model said, whatever was removed for display
 *   timeframeStripped   every time-frame phrase removed, as written
 *   timeframeUnlocated  a stated time frame survived the strip and the
 *                       prohibited filter and is still on the screen text, on a
 *                       CLINICIAN route. displayText is then TIME_FRAME_NOTE
 *                       alone: a referral whose timing this module could not
 *                       remove shows no sentence at all. Fail toward showing
 *                       less. See `unlocatedTimeFrame`.
 *   crisisOnInput       the USER's words disclosed self-harm (Task 4)
 *   crisisLineAppended  the product's crisis block was added (Task 4)
 *   prohibited          {medication, diagnosis} — everything removed, listed
 *   detectorsSha        the pin all of the above was decided by
 */
export function applyGuard({ userText = '', replyText = '', crisisLine = CRISIS_BLOCK_DEFAULT } = {}) {
  const raw = String(replyText ?? '');
  const routing = detectRoute(raw);
  const banner = ROUTE_TO_BANNER[routing.route];

  const prohibited = filterProhibited(raw, userText);
  let display = prohibited.text;

  let timeframeStripped = [];
  if (routing.route === ROUTE.CLINICIAN) {
    const r = stripTimeFrames(display);
    display = r.text;
    timeframeStripped = r.stripped;
    if (timeframeStripped.length) display = tidy(`${display}${TIME_FRAME_NOTE}`);
  }

  // Belt to the strip's braces, and asked of the FINISHED screen text rather
  // than of the strip's own result — see `unlocatedTimeFrame` for why those are
  // different questions. True means a stated time frame survived everything
  // above on a CLINICIAN route, so no model sentence is shown at all and the
  // fixed note stands alone; `rawReply` keeps every word for the log.
  //
  // TIME_FRAME_NOTE carries a leading space because it is written as a suffix;
  // `tidy` takes it off when it stands by itself.
  const timeframeUnlocated = unlocatedTimeFrame(routing.route, display);
  if (timeframeUnlocated) display = tidy(TIME_FRAME_NOTE);

  if (routing.route === ROUTE.UNCLEAR) {
    display = tidy(`${display} ${BANNERS.out_of_scope.line}`);
  }

  const crisisOnInput = detectCrisisStatement(userText).found;
  let crisisLineAppended = false;
  if (crisisOnInput && detectCrisisResponse(raw).signposted.length === 0) {
    display = `${display}\n\n${crisisLine}`;
    crisisLineAppended = true;
  }

  return {
    route: routing.route,
    why: routing.why,
    banner,
    displayText: display,
    rawReply: raw,
    timeframeStripped,
    timeframeUnlocated,
    crisisOnInput,
    crisisLineAppended,
    prohibited: { medication: prohibited.medication, diagnosis: prohibited.diagnosis },
    detectorsSha: pin.sha256,
  };
}
