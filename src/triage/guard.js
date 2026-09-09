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
//      "within 3 days", which is the escalation condition, not a referral;
//   4. crisis — if the USER's words disclose self-harm and the reply does not
//      signpost, the product's own crisis block is appended (R15: additional to
//      the route, never instead of it).
import {
  ROUTE, URGENCY, detectCrisisResponse, detectCrisisStatement, detectMedication,
  detectNamedDiagnosis, detectRoute,
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
const URGENCY_G = new RegExp(URGENCY.source, 'gi');
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

/** Remove every stated time frame. Returns the phrases removed, in order. */
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
    crisisOnInput,
    crisisLineAppended,
    prohibited: { medication: prohibited.medication, diagnosis: prohibited.diagnosis },
    detectorsSha: pin.sha256,
  };
}
