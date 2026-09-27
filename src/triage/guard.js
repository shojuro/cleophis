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
//      Under `crisisRule: 'replace'` (Phase 1h M2; the catalog's registered
//      rule from Phase 1i MA1 — `applyGuard`'s own default stays append) the
//      disclosure instead puts the product's acknowledgement + signpost block
//      FIRST and shows the model's reply under it only when that reply carries
//      a route in `replaceKeepRoutes` — see `applyGuard`; a kept reply then
//      loses every sentence the frozen detector reads as a crisis signpost
//      (`dedupeSignposts`), so the product's block is the only crisis line on
//      screen;
//   (MA6, Phase 1i) between 3 and 4, on a KEPT reply only: every sentence
//      carrying a dose token is removed (`stripDoses`, PROHIBITED_NOTE once),
//      and on a reply whose direction is EMERGENCY or CLINICIAN every scope
//      disclaimer sentence is removed (`stripScopeDisclaimers`). Neither
//      touches the route: it was decided in step 1, from the raw reply;
//   5. the screen check — LAST, on the finished text, crisis block and all. The
//      block is product text and nothing above rewrites it, so a check that ran
//      before the append would be the one rule that looks for a time frame,
//      looking at text that is not what a person sees.
import {
  NUMBER_WORDS, ROUTE, URGENCY, detectCrisisResponse, detectCrisisStatement,
  detectMedication, detectNamedDiagnosis, detectRoute, detectScopeDisclaimer,
} from './detectors.mjs';
// The lookup guard's OWN tokeniser (MA6): its splitter, list-marker rule,
// normaliser and DOSE_UNITS dose tokens. Never a second tokeniser.
import {
  doseTokensIn, normaliseDoseText, splitSentences, stripListMarker,
} from './lookup-guard.js';
// The pin as a GENERATED ES module, not as JSON. `src/index.html` loads
// `app.js` with no bundler and `app.js` imports this file at module scope, so
// `import … with { type: 'json' }` would put a Chromium-123 floor on the whole
// front end: on a minSdk-24 device with an un-updated System WebView `app.js`
// fails to PARSE and neither build boots — the tutor's included. Both pin files
// are written by tools/sync-triage-detectors.sh from one set of values in one
// run, and detectors.test.mjs asserts they agree.
import pin from './detectors.pin.js';

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

// ── Phase 1i MA1: the replace-mode crisis line ──────────────────────────────
//
// Founder decision 4 (2026-09-28): under `replace` the block LEADS the display,
// and on a hidden reply there is no advice at all — so "the advice above" is
// wrong on both counts. The replace-mode line is CRISIS_BLOCK_DEFAULT with its
// final sentence removed and NOTHING ELSE changed: the registered numbers and
// the signposting are byte-identical. The append line stays as it was, because
// under append the block IS below the advice, and the append path's bytes are
// pinned (guard.transcripts.test.mjs's digest).
//
// catalog.triage.json's `med-triage` `crisisLine` equals this byte for byte
// (guard.test.mjs), and the frozen detectors read `crisisReplaceBlock()` as
// CRISIS_SUPPORT — acknowledged and signposted — and as no route.
export const CRISIS_LINE_REPLACE =
  'If you are having thoughts of harming yourself, please tell the health worker with you now, '
  + 'and you can also call the Samaritans free on 116 123 at any hour. If there is immediate danger, call 999.';

// ── Phase 1h M2: the replace rule's block ───────────────────────────────────
//
// Founder decision 2026-09-27: on a plain disclosure the crisis line is the
// PRODUCT's, not the model's. Under `crisisRule: 'replace'` the display leads
// with this acknowledgement followed by the entry's own crisis line (the
// region-specific signpost, never re-typed here), and the model's reply appears
// under it only when it carries a red-flag route.
//
// WORDED AGAINST THE FROZEN REPLY SCORER, not by ear. `detectCrisisResponse`
// (the vendored detectors.mjs) must read the block as CRISIS_SUPPORT —
// acknowledged AND signposted — and `detectRoute` must read it as UNCLEAR,
// because anything else scores the product's own text as "triaged it as a
// symptom". Three acknowledgement patterns fire on this sentence ("thank you
// for telling", "I'm really sorry", "you are not alone"), so one rewording
// cannot drop it below acknowledged; the signpost labels come from the crisis
// line. It names no clinician, no emergency direction and no time frame.
// guard.test.mjs asserts all of it against the detectors this module imports.
export const CRISIS_ACKNOWLEDGEMENT =
  "Thank you for telling me. I'm really sorry you are feeling this way, and you are not alone.";

/** The replace rule's fixed block: the acknowledgement, then the entry's crisis line. */
export function crisisReplaceBlock(crisisLine = CRISIS_LINE_REPLACE) {
  return `${CRISIS_ACKNOWLEDGEMENT} ${crisisLine}`;
}

export const CRISIS_RULES = Object.freeze(['append', 'replace']);
export const REPLACE_KEEP_ROUTES_DEFAULT = Object.freeze([ROUTE.EMERGENCY]);

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

// WHICH OF THE DETECTOR'S SIGNPOST LABELS COUNT AS "THE READER ALREADY HAS A
// ROUTE TO HELP". `detectCrisisResponse` returns `signposted` as a list of its
// own labels, and they are not equally strong evidence:
//
//   'crisis line number'   988, 116 123, 1737, 13 11 14 — a number to call
//   'crisis support'       "crisis line/team/service/helpline/counsellor"
//   'crisis text line'     "text SHOUT", "text HOME"
//   'suicide prevention'   the phrase itself
//   'named crisis service' Samaritans, Papyrus, Crisis Text Line — AND `\bshout\b`
//
// The last one is the defect. Its pattern carries a bare `\bshout\b`, so "give
// your neighbour a shout if it gets worse" is ordinary English that reads as a
// named crisis service, and a `signposted.length > 0` test lets it SUPPRESS the
// product's crisis block. That is the worst direction available: the one reply
// that most needs a crisis line is the one that gets none.
//
// So the ambiguous label does not count on its own. The cost of that choice is a
// reply naming the Samaritans WITHOUT a number getting the product's block as
// well — a redundant signpost, where the alternative is a missing one. R18's
// cost asymmetry decides it, and the ruling says the same: fail toward a second
// block.
//
// Widened from the detector's OWN label vocabulary, never from a second copy of
// its lexicon — the vendored file is not forked, only asked.
export const CRISIS_SIGNPOST_LABELS = Object.freeze([
  'crisis line number', 'crisis support', 'crisis text line', 'suicide prevention',
]);

/**
 * Has this text already given the reader a route to crisis help?
 *
 * NOT `handling`, deliberately, though `detectCrisisResponse` offers it. Its
 * CRISIS_SUPPORT and SIGNPOST_ONLY values are ranked BELOW `TRIAGED`, and every
 * reply this product shows is a triage reply — so a model that routes the
 * medical complaint AND signposts a crisis line comes back `TRIAGED`, and a
 * `handling`-based test would read a real signpost as no signpost at all. The
 * labels are the part of the detector that answers the question asked here.
 */
export function signpostsCrisisSupport(text) {
  const { signposted } = detectCrisisResponse(String(text ?? ''));
  return signposted.some((label) => CRISIS_SIGNPOST_LABELS.includes(label));
}

// A phone-number-like token: FIVE OR MORE DIGITS with at most one space or
// hyphen between each ("85258", "116 124", "116-123", "0800 068 4141"). Counts
// digits, not characters (fix round 2, N1): not "999", "111", "1953", "10-15"
// or "5 - 10" — see `isCrisisSignpostSentence`.
const PHONE_LIKE = /\d(?:[\s-]?\d){4,}/;

/**
 * Is this SENTENCE a crisis signpost the de-duplication must cut?
 *
 * Two ways in, both read from the frozen detector's own labels:
 *   1. `signpostsCrisisSupport(sentence)` — a registered crisis number, a
 *      "crisis line/team/helpline" phrase, a crisis text line, "suicide
 *      prevention". Every one of these is a signpost by itself.
 *   2. the ambiguous 'named crisis service' label AND a phone-number-like
 *      token in the same sentence — "ring the Samaritans on 116 124", "message
 *      SHOUT on 85258".
 *
 * NOT the ambiguous label alone. Its pattern carries a bare `\bshout\b` and
 * `\bpapyrus\b`, and in an EMERGENCY reply "shout" is a first-aid verb: "Gently
 * tap his shoulders and shout, 'Are you okay?'" is in two of the saved red-flag
 * replies, and cutting it removes the responsiveness check from the one reply
 * the block leaves on screen (fix round 1, F1). The cost of the narrowing is a
 * bare "the Samaritans are always there" — a redundant signpost with no number
 * — staying beside the product's block.
 */
function isCrisisSignpostSentence(sentence) {
  if (signpostsCrisisSupport(sentence)) return true;
  const { signposted } = detectCrisisResponse(String(sentence ?? ''));
  return signposted.includes('named crisis service') && PHONE_LIKE.test(sentence);
}

// Sentences for the de-duplication. SENTENCES plus any closing quote or bracket
// after the full stop, so `shout, "Are you okay?"` is ONE sentence and removing
// a neighbour never strands its `"`. Local, not a change to SENTENCES: that one
// is shared with `filterProhibited`, whose bytes the append digest pins.
const DEDUPE_SENTENCES = /[^.!?]+[.!?]+["'\u201d\u2019)\]]*|[^.!?]+$/g;

/**
 * Remove every sentence of a KEPT reply that is a crisis signpost
 * (`isCrisisSignpostSentence`), so that when the product's crisis block is on
 * screen it is the only crisis line: no number inside a sentence the frozen
 * detector reads as a crisis signpost reaches the reader (Phase 1i MA1). A
 * number in a sentence the detector does not read as a crisis signpost — "call
 * the free helpline on 0800 …", "ring 116 132" — is NOT removed; the triage
 * census defines its wrong-number floor the same way.
 *
 * Removal SPLICES the original text: kept sentences keep their bytes, their
 * paragraph breaks and abbreviations ("e.g."); only the cut spans go, and the
 * whitespace they leave is tidied. Text with nothing to remove is returned
 * UNTOUCHED, so a reply with no signpost is byte-identical with the rule on or
 * off. If what is left still signposts crisis support as a whole, the whole
 * reply part goes: fail toward showing less, and the block is still there.
 *
 * @returns {{text: string, removed: number}} `removed` is a COUNT; the removed
 *   sentences survive only in the verdict's `rawReply`, never as display text.
 */
export function dedupeSignposts(text) {
  const src = String(text ?? '');
  const spans = [...src.matchAll(DEDUPE_SENTENCES)]
    .filter((m) => m[0].trim())
    .map((m) => ({ start: m.index, end: m.index + m[0].length, text: m[0] }));
  const cut = spans.filter((sp) => isCrisisSignpostSentence(sp.text));
  if (!cut.length) return { text: src, removed: 0 };
  let out = '';
  let at = 0;
  for (const sp of cut) { out += src.slice(at, sp.start); at = sp.end; }
  out += src.slice(at);
  out = out
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n[ \t]+/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .replace(/[ \t]{2,}/g, ' ')
    .trim();
  if (signpostsCrisisSupport(out)) return { text: '', removed: spans.length };
  return { text: out, removed: cut.length };
}

// ── Phase 1i MA6: displayed doses and displayed disclaimers ─────────────────
//
// Both steps SPLICE, the way `dedupeSignposts` does: kept sentences keep their
// bytes, paragraph breaks and abbreviations; a cut sentence goes with the
// whitespace after it. Sentences come from the lookup guard's `splitSentences`
// (which knows "2.5 ml" is not a full stop), located in the text in order.
// A text with nothing to remove comes back UNTOUCHED.

function sentenceSpans(src) {
  const spans = [];
  let at = 0;
  for (const s of splitSentences(src)) {
    const start = src.indexOf(s.text, at);
    if (start < 0) continue;
    const end = start + s.text.length;
    const endWithSep = end + /^\s*/.exec(src.slice(end))[0].length;
    spans.push({ start, sepStart: end, end: endWithSep, text: s.text });
    at = end;
  }
  return spans;
}

function spliceOut(src, cut) {
  let out = '';
  let at = 0;
  // A cut sentence goes with the whitespace after it; where that whitespace
  // was the stronger break (a paragraph after a line), it replaces the break
  // before, so "A.\n\nB.\n\nC." without B is "A.\n\nC." and never "A.\nC.".
  const breaks = (w) => (w.match(/\n/g) ?? []).length;
  for (const sp of cut) {
    out += src.slice(at, sp.start);
    const after = src.slice(sp.sepStart, sp.end);
    const before = /\s*$/.exec(out)[0];
    if (breaks(after) > breaks(before)) out = out.slice(0, out.length - before.length) + after;
    at = sp.end;
  }
  out += src.slice(at);
  return out
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n[ \t]+/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .replace(/[ \t]{2,}/g, ' ')
    .trim();
}

/** Does this sentence carry a dose token, read with the lookup guard's normaliser and DOSE_UNITS? */
export function carriesDose(sentence) {
  return doseTokensIn(normaliseDoseText(stripListMarker(sentence))).length > 0;
}

/**
 * Remove every sentence carrying a dose token ("500mg", "8 tablets", "half a
 * tablet"; not "call 999", not "take a tablet"). If anything was removed,
 * PROHIBITED_NOTE is appended once — at the end, as `filterProhibited` places
 * it — unless the text already carries it.
 *
 * @returns {{text: string, removed: number}} a COUNT: the removed sentences
 *   survive only in the verdict's `rawReply`, never as display text.
 */
export function stripDoses(text) {
  const src = String(text ?? '');
  const cut = sentenceSpans(src).filter((sp) => carriesDose(sp.text));
  if (!cut.length) return { text: src, removed: 0 };
  const out = spliceOut(src, cut);
  const note = PROHIBITED_NOTE.trim();
  if (out.includes(note)) return { text: out, removed: cut.length };
  return { text: out ? `${out}${PROHIBITED_NOTE}` : note, removed: cut.length };
}

/**
 * Remove every sentence the vendored `detectScopeDisclaimer` matches. Not
 * route-aware by itself; `applyGuard` decides when it applies.
 *
 * @returns {{text: string, removed: number}}
 */
export function stripScopeDisclaimers(text) {
  const src = String(text ?? '');
  const cut = sentenceSpans(src).filter((sp) => detectScopeDisclaimer(sp.text).found);
  if (!cut.length) return { text: src, removed: 0 };
  return { text: spliceOut(src, cut), removed: cut.length };
}

const RED_FLAG_ROUTES = Object.freeze([ROUTE.EMERGENCY, ROUTE.CLINICIAN]);

/**
 * The disclaimer step's gate: strip only when what is LEFT still directs to
 * emergency care or a clinician.
 *
 * NOT the raw route, because under the r3 router a raw reply carrying a scope
 * disclaimer is never EMERGENCY or CLINICIAN: a disclaimer beside an emergency
 * direction routes UNCLEAR ("contradictory-out-of-scope-and-emergency"), beside
 * a referral OUT_OF_SCOPE. A raw-route gate would never fire. So the question is
 * asked of the reply with its disclaimer sentences set aside — and that also
 * guarantees the cut never takes the red-flag direction with it: a reply whose
 * only direction shares a sentence with the disclaimer ("I cannot judge this;
 * go to A&E now.") is left whole, because cutting it would leave the reader no
 * direction at all. SELF_CARE and UNCLEAR remainders are untouched: the floor
 * is disclaimers on red flags. The verdict's route and banner stay the raw
 * reply's either way.
 */
function stripRedFlagDisclaimers(text) {
  const d = stripScopeDisclaimers(text);
  if (!d.removed || !RED_FLAG_ROUTES.includes(detectRoute(d.text).route)) return { text: String(text ?? ''), removed: 0 };
  return d;
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
 *
 * UNDER `crisisRule: 'replace'` ONLY, seven more keys — so the default verdict
 * keeps its twelve-key shape, byte for byte (guard.transcripts.test.mjs pins a
 * digest of it from before the option existed):
 *
 *   crisisRule          'replace'
 *   keepRoutes          the routes whose reply is still shown under the block
 *   crisisReplaced      the disclosure fired and the block LEADS the display
 *   replyShown          some of the model's (filtered) reply is on screen
 *                       under it; true whenever the rule did not fire. False
 *                       when the route is not kept, AND when it is kept but
 *                       de-duplication cut every sentence
 *   routeDetected       detectRoute of the RAW reply — what decided whether the
 *                       route is kept
 *   dedupeSignposts     whether de-duplication ran (Phase 1i MA1; default true)
 *   signpostsRemoved    how many sentences of the KEPT reply were cut because
 *                       they signposted crisis help beside the product's block.
 *                       A count only: the sentences stay in `rawReply` and are
 *                       never display text. 0 when the reply is hidden
 *
 * Under `append`, `dedupeSignposts: true` must be asked for explicitly; the
 * verdict then carries those two keys as well (and only then).
 *
 * and when the route is NOT kept `banner` is null: no route banner is drawn
 * over text the model did not write. A KEPT route whose reply de-duplication
 * emptied keeps its banner with `replyShown` false: the banner is then the
 * route's only carrier on screen (fix round 1, F2). `crisisLineAppended` is false throughout, because
 * nothing is appended UNDER a reply.
 *
 * @param {object} opts
 * @param {'append'|'replace'} [opts.crisisRule='append']
 * @param {string[]} [opts.replaceKeepRoutes=['EMERGENCY']] read only by 'replace'
 * @param {string} [opts.crisisLine] the entry's crisis line; absent means
 *   CRISIS_BLOCK_DEFAULT under append and CRISIS_LINE_REPLACE under replace
 * @param {boolean} [opts.dedupeSignposts] default: true under replace, false under append
 * @param {boolean} [opts.stripDoses=true] MA6: remove every dose sentence of a kept reply
 * @param {boolean} [opts.stripScopeDisclaimers=true] MA6: remove the scope-disclaimer
 *   sentences of a kept reply whose remaining direction is EMERGENCY or CLINICIAN
 *
 * PHASE 1i MA6 KEYS, present for each switch that is on (so both off is the
 * historical verdict byte for byte), and always present under replace:
 *
 *   stripDoses            whether the dose strip ran
 *   dosesRemoved          how many sentences of the KEPT reply were cut for a
 *                         dose token; PROHIBITED_NOTE is on screen when > 0
 *   stripScopeDisclaimers whether the disclaimer strip ran
 *   disclaimersRemoved    how many scope-disclaimer sentences were cut
 *
 * Counts only, like `signpostsRemoved`: the cut sentences stay in `rawReply`
 * and are never display text. 0 when the replace rule hides the reply. The
 * route and banner are decided from the raw reply before either strip.
 */
export function applyGuard({
  userText = '', replyText = '', crisisLine,
  crisisRule = 'append', replaceKeepRoutes = REPLACE_KEEP_ROUTES_DEFAULT,
  dedupeSignposts: dedupe,
  stripDoses: stripDosesOpt, stripScopeDisclaimers: stripDisclaimersOpt,
} = {}) {
  // Refused loudly rather than defaulted: a probe run that asked for a rule and
  // silently measured the other would be a registered number about nothing.
  if (!CRISIS_RULES.includes(crisisRule)) {
    throw new TypeError(`applyGuard: unknown crisisRule ${JSON.stringify(crisisRule)}`);
  }
  if (dedupe !== undefined && typeof dedupe !== 'boolean') {
    throw new TypeError(`applyGuard: dedupeSignposts must be a boolean, got ${JSON.stringify(dedupe)}`);
  }
  for (const [name, value] of [['stripDoses', stripDosesOpt], ['stripScopeDisclaimers', stripDisclaimersOpt]]) {
    if (value !== undefined && typeof value !== 'boolean') {
      throw new TypeError(`applyGuard: ${name} must be a boolean, got ${JSON.stringify(value)}`);
    }
  }
  // MA6: both on by default under every rule; `false` switches each off.
  const dosesOn = stripDosesOpt ?? true;
  const disclaimersOn = stripDisclaimersOpt ?? true;
  const replace = crisisRule === 'replace';
  const keepRoutes = replace ? validKeepRoutes(replaceKeepRoutes) : null;
  // Each rule's own default line (MA1): the append line says "in addition to
  // the advice above" because under append it IS below the advice; the replace
  // block leads, so its default line does not.
  // `||`, not `??`: an empty line would leave a block with no number (M4).
  const line = crisisLine || (replace ? CRISIS_LINE_REPLACE : CRISIS_BLOCK_DEFAULT);
  // On by default under replace, off under append (MA1). An explicit value wins.
  const dedupeOn = dedupe ?? replace;

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

  // MA6: the two product-owned floors, on a KEPT reply only (a reply the
  // replace rule hides lends nothing to the screen and counts nothing). After
  // routing — `routing` is never recomputed — and before the banner and the
  // crisis rule, so the de-duplication and the screen check read the cleaned
  // reply. `crisisOnInput` reads only the user's words, so asking it here is
  // the same question asked below.
  const crisisOnInput = detectCrisisStatement(userText).found;
  const keptReply = !replace || !crisisOnInput || keepRoutes.includes(routing.route);
  let dosesRemoved = 0;
  let disclaimersRemoved = 0;
  if (keptReply && dosesOn) {
    const r = stripDoses(display);
    display = r.text;
    dosesRemoved = r.removed;
  }
  if (keptReply && disclaimersOn) {
    const r = stripRedFlagDisclaimers(display);
    display = r.text;
    disclaimersRemoved = r.removed;
  }
  const owned = {
    ...(replace || dosesOn ? { stripDoses: dosesOn, dosesRemoved } : {}),
    ...(replace || disclaimersOn ? { stripScopeDisclaimers: disclaimersOn, disclaimersRemoved } : {}),
  };

  if (routing.route === ROUTE.UNCLEAR) {
    display = tidy(`${display} ${BANNERS.out_of_scope.line}`);
  }

  // Task 4 owns WHAT the crisis block says and when it is due. The ORDER is
  // owned here and is an invariant rather than a preference: the block goes on
  // BEFORE the screen check below, because it is product text that nothing above
  // rewrites, and a check that ran before the append would be the only rule
  // looking for a time frame, looking at text that is not what a person sees.
  // Anything Task 4 adds to `display` belongs above that check for that reason.
  //
  // THE SUPPRESSION READS THE SCREEN, NOT THE RAW REPLY. The block is withheld
  // only because the reader has ALREADY been given a route to help — so the
  // question is about the text they will be given, and three rules above this
  // one can delete the model's signpost before it gets there: the prohibited
  // filter can take out the sentence that carried it, the note-alone fallback
  // takes out every sentence, and the time-frame fallback below discards the
  // model's text entirely. Deciding from `raw` answers a question about a reply
  // that no longer exists, and it fails in the one direction R15 says it must
  // not: no crisis line at the moment it matters.
  if (replace) {
    return replaceVerdict({
      raw, routing, banner, display, prohibited, timeframeStripped, crisisOnInput, crisisLine: line, keepRoutes, dedupeOn, owned,
    });
  }
  // Append with de-duplication asked for explicitly (off by default, so the
  // default verdict keeps its twelve keys and its pinned bytes): on a
  // disclosure the model's signposting sentences are cut FIRST, which makes the
  // product's line due below — the product's line replaces the model's.
  let signpostsRemoved = 0;
  if (dedupeOn && crisisOnInput) {
    const d = dedupeSignposts(display);
    display = d.text;
    signpostsRemoved = d.removed;
  }
  const withLine = (text) => (text ? `${text}\n\n${line}` : line);
  const crisisDue = () => crisisOnInput && !signpostsCrisisSupport(display);
  let crisisLineAppended = crisisDue();
  if (crisisLineAppended) display = withLine(display);

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
  // AND THE QUESTION IS ASKED AGAIN, because this fallback throws away the
  // model's text — including a signpost that had suppressed the block a moment
  // ago. Re-asking of the new display puts the product's block back: the only
  // reason to withhold it was a signpost that is no longer on the screen.
  //
  // TIME_FRAME_NOTE carries a leading space because it is written as a suffix;
  // `tidy` takes it off when it stands by itself.
  const timeframeUnlocated = unlocatedTimeFrame(routing.route, display);
  if (timeframeUnlocated) {
    display = tidy(TIME_FRAME_NOTE);
    if (crisisDue()) {
      display = withLine(display);
      crisisLineAppended = true;
    }
  }

  const verdict = {
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
  if (dedupeOn) Object.assign(verdict, { dedupeSignposts: true, signpostsRemoved });
  // MA6: present only for a switch that is on, so with both off the verdict is
  // the historical one byte for byte (guard.transcripts.test.mjs's digest).
  Object.assign(verdict, owned);
  return verdict;
}

function validKeepRoutes(routes) {
  const known = Object.values(ROUTE);
  if (!Array.isArray(routes) || routes.some((r) => !known.includes(r))) {
    throw new TypeError(`applyGuard: replaceKeepRoutes must be an array of ${known.join('|')}, got ${JSON.stringify(routes)}`);
  }
  return [...routes];
}

/**
 * The `replace` rule's tail of `applyGuard`: everything above it — route,
 * prohibited filter, time-frame strip, the UNCLEAR line — has already run on
 * the reply, so a reply kept under the block is exactly the reply `append`
 * would have shown, minus the appended block.
 *
 * No disclosure: the display is what `append` shows with no disclosure (the
 * append branch cannot fire without one), so the two rules agree on every
 * turn that does not disclose.
 *
 * The screen check still runs LAST on the finished text, block and all. If a
 * stated time frame survives on a kept CLINICIAN reply, the reply part is
 * replaced by TIME_FRAME_NOTE and the block stays — the block is never what a
 * later rule removes.
 */
function replaceVerdict({
  raw, routing, banner, display, prohibited, timeframeStripped, crisisOnInput, crisisLine, keepRoutes, dedupeOn, owned,
}) {
  const crisisReplaced = crisisOnInput;
  // TWO decisions, deliberately apart (fix round 1, F2). `kept` is the RULE:
  // the route is in the keep set, so its banner is drawn. `replyShown` is the
  // SCREEN: some of the model's text is actually under the block. They differ
  // only when de-duplication empties a kept reply ("Go to A&E now and call the
  // Samaritans on 116 123." is one sentence): the block stands alone, and the
  // banner STAYS, because once the sentence is gone the banner is the route's
  // only carrier — nulling it would lose the EMERGENCY direction entirely.
  const kept = !crisisReplaced || keepRoutes.includes(routing.route);
  const block = crisisReplaceBlock(crisisLine);

  // De-duplication (MA1): the block is on screen whenever the rule fired; the
  // only model text beside it is a KEPT reply, so that is what is cleaned. A
  // hidden reply lends nothing to the screen and counts nothing.
  let signpostsRemoved = 0;
  if (dedupeOn && crisisReplaced && kept) {
    const d = dedupeSignposts(display);
    display = d.text;
    signpostsRemoved = d.removed;
  }
  const replyShown = !crisisReplaced || (kept && display !== '');
  const compose = (replyPart) => {
    if (!crisisReplaced) return replyPart;
    return replyShown ? `${block}\n\n${replyPart}` : block;
  };

  let screen = compose(display);
  const timeframeUnlocated = unlocatedTimeFrame(routing.route, screen);
  // `signpostsRemoved` still counts what de-duplication cut even when this
  // fallback then replaces the reply part with the note: it is the count of
  // the rule's cuts, not of sentences missing from the final screen (M5).
  if (timeframeUnlocated) screen = compose(tidy(TIME_FRAME_NOTE));

  return {
    route: routing.route,
    why: routing.why,
    banner: crisisReplaced && !kept ? null : banner,
    displayText: screen,
    rawReply: raw,
    timeframeStripped,
    timeframeUnlocated,
    crisisOnInput,
    crisisLineAppended: false,
    prohibited: { medication: prohibited.medication, diagnosis: prohibited.diagnosis },
    prohibitedRemoved: prohibited.removed,
    detectorsSha: pin.sha256,
    crisisRule: 'replace',
    keepRoutes,
    crisisReplaced,
    replyShown,
    routeDetected: routing.route,
    dedupeSignposts: dedupeOn,
    signpostsRemoved,
    ...owned,
  };
}
