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
//      frequency or diagnosis are removed (fail toward showing less), and what
//      is left is re-asked as a whole, because the detectors' question is about
//      the whole reply and a prescription can be written across a full stop;
//   3. time frames — on a CLINICIAN route only, every URGENCY match is removed
//      (R64/R65: the stated time frame is what made a confident referral worse
//      than a decline). EMERGENCY keeps "now"; SELF_CARE keeps the caveat's
//      "within 3 days", which is the escalation condition, not a referral.
//      If a stated time frame survives all of that on the finished screen text,
//      the reply is replaced by the fixed note — see `unlocatedTimeFrame`;
//   4. crisis — if the USER's words disclose self-harm and the reply does not
//      signpost, the product's own crisis block is appended (R15: additional to
//      the route, never instead of it);
//   5. the screen check — LAST, on the finished text, crisis block and all. The
//      block is product text and nothing above rewrites it, so a check that ran
//      before the append would be the one rule that looks for a time frame,
//      looking at text that is not what a person sees.
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

// The filter iterates, so it needs a stop. Four is a bound, not a budget: the
// 1,000-reply sweep settles inside two, and a text that still trips after four
// is one this module has stopped understanding — which is the case the note-alone
// fallback exists for, not a case to keep grinding at.
const MAX_FILTER_PASSES = 4;

/**
 * Drop every sentence that names a medication (dose, route, frequency or an
 * introduced drug) or a diagnosis. Sentence-level on purpose: the detectors
 * report findings, not character offsets, and a whole sentence is the
 * smallest unit whose removal cannot leave half a prescription behind.
 *
 * IT ITERATES, BECAUSE THE DETECTOR'S QUESTION IS ABOUT THE WHOLE REPLY.
 * `detectMedication` cancels R7's carve-out on a dose, route or frequency found
 * ANYWHERE in the reply — its own header gives the reason: "Ibuprofen is an
 * anti-inflammatory. Have it every six hours." prescribes across a full stop,
 * and a clause-local rule excuses the name and then never sees the schedule.
 * A filter that asks the question one sentence at a time inherits that hole
 * exactly: neither sentence trips alone, and the text it hands back does.
 *
 * So each pass removes (a) every sentence prohibited by itself and then, if what
 * survives STILL trips as a whole, (b) BOTH HALVES of every pair that trips
 * together. Both halves, not the cheaper one: a prescription split across a full
 * stop is one act, the reader is no better served by the half of it that names
 * the drug, and this module's rule is to fail toward showing less.
 *
 * THE LOOP TESTS THE STRING IT IS ABOUT TO RETURN, not the sentences it kept.
 * That is the difference between clean by construction and clean by inference:
 * the returned text is `kept` joined AND `PROHIBITED_NOTE` appended AND run
 * through `tidy`, so the only question worth asking is of that string. If it is
 * clean, it is returned; if not, another pass runs. If four passes do not settle
 * it, or a pass stops making progress, the display becomes the note ALONE and
 * the receipt lists every sentence — no model text reaches a screen this module
 * could not clear.
 *
 * @returns {{text: string, removed: string[], medication: string[], diagnosis: string[]}}
 *   `removed` is the RECEIPT: the exact sentences taken out of the display, in
 *   the order they were written. `medication`/`diagnosis` are the REASON: what
 *   the detectors objected to in the whole raw reply. The two are different
 *   views on purpose — see the verdict's `prohibited` note in `applyGuard`.
 *
 * Found on real data: floors/Qwen3-4B locked-heldout ENT-09, the single leak in
 * the 1,000-reply sweep, where a dosage form in one sentence was un-excused by
 * "2-3 times a day" in another.
 */
export function filterProhibited(text, patientText = '') {
  const med = detectMedication(text, { patientText });
  const dx = detectNamedDiagnosis(text, { patientText });
  const reason = {
    medication: [...med.drugs, ...med.classes, ...med.doses, ...med.routes, ...med.frequencies],
    diagnosis: dx.names,
  };
  if (!med.found && !dx.found) return { text: String(text), removed: [], ...reason };

  const trips = (s) => detectMedication(s, { patientText }).found
    || detectNamedDiagnosis(s, { patientText }).found;

  const sentences = (String(text).match(SENTENCES) ?? []).map((s) => s.trim()).filter(Boolean);
  // One mask over the ORIGINAL sentences, so the receipt comes out in the order
  // a reader would have met them however many passes it took to get there.
  const cut = sentences.map(() => false);
  const liveIdx = () => sentences.map((_, i) => i).filter((i) => !cut[i]);
  const cutCount = () => cut.filter(Boolean).length;
  const receipt = () => sentences.filter((_, i) => cut[i]);
  const assemble = () => {
    const body = sentences.filter((_, i) => !cut[i]).join(' ');
    return cutCount() ? tidy(`${body}${PROHIBITED_NOTE}`) : body;
  };

  for (let pass = 0; pass < MAX_FILTER_PASSES; pass += 1) {
    const before = cutCount();
    for (const i of liveIdx()) if (trips(sentences[i])) cut[i] = true;

    const idx = liveIdx();
    if (idx.length > 1 && trips(idx.map((i) => sentences[i]).join(' '))) {
      for (const a of idx) {
        for (const b of idx) {
          if (a === b) continue;
          if (trips(`${sentences[a]} ${sentences[b]}`)) { cut[a] = true; cut[b] = true; }
        }
      }
    }

    const out = assemble();
    if (!trips(out)) return { text: out, removed: receipt(), ...reason };
    // A pass that removed nothing will remove nothing next time either: the
    // finding is one no pair reaches, so iterating again is not the answer.
    if (cutCount() === before) break;
  }

  cut.fill(true);
  return { text: tidy(PROHIBITED_NOTE), removed: receipt(), ...reason };
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
 *   timeframeUnlocated  a stated time frame survived everything above and is
 *                       still on the FINISHED screen text — crisis block and
 *                       all — on a CLINICIAN route. displayText is then
 *                       TIME_FRAME_NOTE, plus the crisis block if one was due:
 *                       a referral whose timing this module could not remove
 *                       shows no model sentence at all. Fail toward showing
 *                       less. See `unlocatedTimeFrame`.
 *   crisisOnInput       the USER's words disclosed self-harm (Task 4)
 *   crisisLineAppended  the product's crisis block was added (Task 4)
 *   prohibited          {medication, diagnosis} — the REASON: what the detectors
 *                       objected to in the RAW reply, which is why anything was
 *                       removed. A phrase listed here can still be on screen
 *                       (the reply-scoped case cancels a finding by deleting a
 *                       different sentence), so it does not answer "what was
 *                       taken out" — `prohibitedRemoved` does
 *   prohibitedRemoved   the RECEIPT: the exact sentences taken out of the
 *                       display, in the order they were written. Task 8 renders
 *                       this list; the pair reads as "removed THESE sentences
 *                       BECAUSE the detectors found THAT". Empty when nothing
 *                       was removed, and every sentence when the filter had to
 *                       fall back to showing PROHIBITED_NOTE alone
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

  if (routing.route === ROUTE.UNCLEAR) {
    display = tidy(`${display} ${BANNERS.out_of_scope.line}`);
  }

  // Task 4 owns WHAT the crisis block says and when it is due. The ORDER is
  // owned here and is an invariant rather than a preference: the block goes on
  // BEFORE the screen check below, because it is product text that nothing above
  // rewrites, and a check that ran before the append would be the only rule
  // looking for a time frame, looking at text that is not what a person sees.
  // Anything Task 4 adds to `display` belongs above that check for that reason.
  const crisisOnInput = detectCrisisStatement(userText).found;
  const crisisLineAppended = crisisOnInput && detectCrisisResponse(raw).signposted.length === 0;
  const withCrisisBlock = (text) => (crisisLineAppended ? `${text}\n\n${crisisLine}` : text);
  display = withCrisisBlock(display);

  // THE SCREEN CHECK, LAST, on the finished text — see `unlocatedTimeFrame` for
  // why it asks about the screen rather than about what the strip removed. True
  // means a stated time frame survived everything above on a CLINICIAN route, so
  // the model's sentence is not shown at all; `rawReply` keeps every word.
  //
  // The fallback re-attaches the crisis block rather than replacing it. R15 makes
  // that block additional to the route and never instead of it, so it is not
  // something a later rule may silently delete — and if the BLOCK's own wording
  // is what the check saw, the flag stays true of what is shown, which is the
  // operator's signal to reword their crisis line. This module does not solve a
  // bad crisis line by removing a self-harm signpost.
  //
  // TIME_FRAME_NOTE carries a leading space because it is written as a suffix;
  // `tidy` takes it off when it stands by itself.
  const timeframeUnlocated = unlocatedTimeFrame(routing.route, display);
  if (timeframeUnlocated) display = withCrisisBlock(tidy(TIME_FRAME_NOTE));

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
    prohibitedRemoved: prohibited.removed,
    detectorsSha: pin.sha256,
  };
}
