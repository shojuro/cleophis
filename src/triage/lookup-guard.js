// src/triage/lookup-guard.js — the LOOKUP guard: rule `dose-cite-v1`.
//
// Phase 1h M6. The reference lookup lets the model state a dose only when the
// dose is CITED from the bundled reference pack. This module is the product's
// check on that, applied to a finished grounded reply before anything reaches
// the screen. It is NOT `applyGuard`: a lookup reply has no route and gets no
// triage banner. Pure: no DOM, no Tauri.
//
// THIS FILE IS THE CANONICAL RULE. The triage repo's `probes/dose-cite.mjs`
// (Task T4) vendors it and asserts identity over the fixtures in
// `src/triage/fixtures/lookup-guard/`. Change the rule here, re-run the
// fixtures, re-vendor. Never fork it.
//
// The rule, per sentence of the reply:
//   1. every `[n]` must name a source in range (1..sources.length). An
//      out-of-range or unparseable citation withholds the sentence;
//   2. every DOSE TOKEN (an amount with a dose unit, or a frequency form) must
//      sit in a sentence that cites, and must be present verbatim, after
//      `normaliseDoseText`, in the text of one of the sources that sentence
//      cites. Otherwise the whole sentence is withheld;
//   3. a sentence citing a source whose section path matches
//      `OVERDOSE_SECTION_PATTERNS` is withheld, dose or not;
//   4. a withheld sentence is replaced by `WITHHELD_BANNER` (a run of withheld
//      sentences by one banner). Everything else is kept as written;
//   5. if no kept sentence carries a valid citation, nothing citable is left and
//      the whole reply is shown as `LOOKUP_NO_EVIDENCE_TEXT`.
//
// Precedence when a sentence breaks more than one clause: the first of
// citation-malformed, citation-out-of-range, overdose-section, dose-uncited,
// dose-not-in-source is the recorded reason.
import { NUMBER_WORDS } from './detectors.mjs';
import pin from './detectors.pin.js';

export const LOOKUP_RULE = 'dose-cite-v1';

/**
 * The scripted refusal for a lookup with no evidence. Defined ONCE, here, and
 * used for all three no-evidence paths: retrieval found nothing
 * (`rag_lookup` → noEvidence), and a grounded reply with nothing citable left.
 * Product wording, deliberately not the contract's `refusal_with_offer` (the
 * contract text is never re-typed outside its TOML).
 */
export const LOOKUP_NO_EVIDENCE_TEXT =
  'The reference pack has nothing on that, so no answer is given. Ask a pharmacist or clinician.';

/** Stands in for a withheld sentence, or a run of them. */
export const WITHHELD_BANNER =
  '[A sentence was withheld: the reference pack does not confirm it.]';

export const WITHHELD_REASONS = Object.freeze({
  CITATION_MALFORMED: 'citation-malformed',
  CITATION_OUT_OF_RANGE: 'citation-out-of-range',
  OVERDOSE_SECTION: 'overdose-section',
  DOSE_UNCITED: 'dose-uncited',
  DOSE_NOT_IN_SOURCE: 'dose-not-in-source',
});

/**
 * Section paths the lookup never quotes from: overdose and maximum-dose
 * material. A health worker looking up a drug must not be handed the "how much
 * is too much" numbers by a floor-tier model, cited or not. Tested over the
 * section path string as the pack's chunk metadata carries it (for example
 * "Paracetamol > If you take too much").
 */
export const OVERDOSE_SECTION_PATTERNS = Object.freeze([
  /\boverdos/i,
  /\bmax(?:imum|\.)?\s+(?:daily\s+|single\s+|total\s+)?doses?\b/i,
  /\bhow much is too much\b/i,
  /\btoo much\b/i,
  /\bpoison/i,
  /\btoxicity\b/i,
]);

/** Does this section path name overdose or maximum-dose material? */
export function isOverdoseSection(sectionPath) {
  const s = String(sectionPath ?? '');
  return OVERDOSE_SECTION_PATTERNS.some((re) => re.test(s));
}

// ── Normalisation ───────────────────────────────────────────────────────────
//
// Applied identically to the reply and to the source text, so "500 milligrams
// three times a day" in the source and "500 mg three times daily" in the reply
// are the same tokens. It changes SPELLING only, never quantity: no unit is
// converted into another (mg stays mg, g stays g), and a per-kilogram dose
// keeps its "/kg".

const UNIT_SPELLINGS = [
  // longest spellings first inside each alternation
  ['mcg', 'micrograms?|mcg|[µμ]g'],
  ['mg', 'milligrams?|mgs?'],
  ['ml', 'millilit(?:re|er)s?|mls?'],
  ['mmol', 'millimoles?|mmol'],
  ['iu', 'international units?|iu'],
  ['units', 'units?'],
  ['tablets', 'tablets?|tabs?'],
  ['capsules', 'capsules?|caps?'],
  ['sachets', 'sachets?'],
  ['drops', 'drops?'],
  ['puffs', 'puffs?'],
  ['g', 'grams?|gm|g'],
  ['%', 'percent|%'],
];
const UNIT_RES = UNIT_SPELLINGS.map(([canon, alt]) => [
  canon,
  new RegExp(`(\\d)\\s*(?:${alt})(?![a-z0-9])`, 'g'),
]);

/**
 * The spelling normalisation both sides of the verbatim check go through.
 * Exported so the fixtures can pin it and the triage port can match it.
 */
export function normaliseDoseText(text) {
  let s = String(text ?? '').normalize('NFKC').toLowerCase();
  s = s.replace(/[‐‑‒–—−]/g, '-');
  // thousands separators: 1,000 -> 1000 (repeat for 1,000,000)
  for (let prev = null; prev !== s;) { prev = s; s = s.replace(/(\d),(\d{3})(?!\d)/g, '$1$2'); }
  for (const [word, digits] of NUMBER_WORDS) s = s.replace(new RegExp(`\\b${word}\\b`, 'g'), digits);
  s = s.replace(/\bonce\b/g, '1 times').replace(/\btwice\b/g, '2 times').replace(/\bthrice\b/g, '3 times');
  s = s.replace(/\b(\d+) time\b/g, '$1 times');
  // per-period forms
  s = s.replace(/\s*\b(?:a|per|each|every)\s+day\b/g, '/day').replace(/\s+daily\b/g, '/day');
  s = s.replace(/\s*\b(?:a|per|each|every)\s+week\b/g, '/week').replace(/\s+weekly\b/g, '/week');
  // ranges: "4 to 6", "4 - 6", "1 or 2" -> "4-6"
  s = s.replace(/(\d)\s*(?:-|\bto\b|\bor\b)\s*(\d)/g, '$1-$2');
  // hours
  s = s.replace(/(\d)\s*(?:hours?|hrs?|h)(?![a-z0-9])/g, '$1 hours');
  for (const [canon, re] of UNIT_RES) s = s.replace(re, `$1${canon}`);
  // "mg per kg", "mg / kg" -> "mg/kg"
  s = s.replace(/(mcg|mg|ml|mmol|iu|units|g)\s*(?:\/|\bper\b)\s*(kg|dose|day|week|ml|hours?)\b/g, '$1/$2');
  return s.replace(/\s+/g, ' ').trim();
}

// ── Dose tokens ─────────────────────────────────────────────────────────────

const UNIT_ALT = 'mcg|mg|ml|mmol|iu|units|tablets|capsules|sachets|drops|puffs|g|%';
const NUM = '\\d+(?:\\.\\d+)?';
const AMOUNT_RE = new RegExp(
  `${NUM}(?:-${NUM})?(?:${UNIT_ALT})(?:\\/(?:${NUM})?(?:kg|dose|day|week|ml|hours?|mg|g))?(?![a-z0-9])`,
  'g',
);
const FREQUENCY_RE = new RegExp(`${NUM}(?:-${NUM})? times\\/(?:day|week)`, 'g');
const INTERVAL_RE = new RegExp(`\\bevery ${NUM}(?:-${NUM})? hours`, 'g');

function startsCleanly(s, index) {
  // a number token must not be the tail of a longer number ("1500mg" is not "500mg")
  return index === 0 || !/[\d.]/.test(s[index - 1]);
}

/** Every dose token in already-normalised text, in order of appearance. */
export function doseTokens(normalised) {
  const found = [];
  for (const re of [AMOUNT_RE, FREQUENCY_RE, INTERVAL_RE]) {
    for (const m of normalised.matchAll(re)) {
      if (re !== INTERVAL_RE && !startsCleanly(normalised, m.index)) continue;
      found.push({ token: m[0], index: m.index });
    }
  }
  return found.sort((a, b) => a.index - b.index).map((t) => t.token);
}

/** Is `token` present in normalised `source` as a whole token? */
export function sourceHasToken(source, token) {
  for (let i = source.indexOf(token); i !== -1; i = source.indexOf(token, i + 1)) {
    const before = i === 0 ? '' : source[i - 1];
    const after = source.slice(i + token.length, i + token.length + 2);
    const beforeOk = /^\d/.test(token) ? !/[\d.]/.test(before) : !/[a-z0-9]/.test(before);
    // "500mg" must not match inside "500mg/kg"; "2.5ml" not inside "2.55ml"
    const afterOk = !/^[a-z0-9/]/.test(after) && !/^\.\d/.test(after);
    if (beforeOk && afterOk) return true;
  }
  return false;
}

// ── Sentences and citations ─────────────────────────────────────────────────

const ABBREVIATION_END = /(?:^|[\s(])(?:e\.g|i\.e|approx|vs)$/i;
const TRAILING_CITATION = /^[ \t]*\[[^\]\n]*\]/;

/**
 * Split a reply into sentences, each with the whitespace that followed it, so
 * the kept text is reassembled exactly as written. A sentence ends at `.`, `!`
 * or `?` followed by whitespace or the end, or at a newline. A citation written
 * AFTER the full stop ("... doses. [1]") belongs to the sentence before it. A
 * decimal point is not an end ("2.5 ml"), nor are "e.g." and "i.e.".
 *
 * @returns {{text: string, sep: string}[]}
 */
export function splitSentences(reply) {
  const text = String(reply ?? '').trim();
  const out = [];
  let start = 0;
  let i = 0;
  const push = (end, sepEnd) => {
    const body = text.slice(start, end);
    if (body.trim()) out.push({ text: body.trim(), sep: text.slice(end, sepEnd) });
    else if (out.length) out[out.length - 1].sep += text.slice(start, sepEnd);
    start = sepEnd;
    i = sepEnd;
  };
  while (i < text.length) {
    const ch = text[i];
    if (ch === '\n') {
      let j = i;
      while (j < text.length && /\s/.test(text[j])) j++;
      push(i, j);
      continue;
    }
    if ('.!?'.includes(ch) && !(ch === '.' && ABBREVIATION_END.test(text.slice(start, i)))) {
      let k = i + 1;
      while (k < text.length && /[.!?"'”’)]/.test(text[k])) k++;
      for (let m = TRAILING_CITATION.exec(text.slice(k)); m; m = TRAILING_CITATION.exec(text.slice(k))) {
        k += m[0].length;
      }
      if (k >= text.length || /\s/.test(text[k])) {
        let j = k;
        while (j < text.length && /\s/.test(text[j])) j++;
        push(k, j);
        continue;
      }
    }
    i++;
  }
  if (start < text.length) push(text.length, text.length);
  return out;
}

const CITATION_BRACKET = /\[([^\]\n]*)\]/g;
const CITATION_LIST = /^\s*\d+(?:\s*,\s*\d+)*\s*$/;

/**
 * The citations in one sentence. A bracket holding a comma list of numbers is
 * a citation; a bracket holding any other text WITH a digit in it ("[1-3]",
 * "[source 2]") is malformed; a bracket with no digit is not a citation.
 *
 * @returns {{numbers: number[], malformed: boolean}}
 */
export function citationsIn(sentence) {
  const numbers = [];
  let malformed = false;
  for (const m of String(sentence).matchAll(CITATION_BRACKET)) {
    const inner = m[1];
    if (CITATION_LIST.test(inner)) numbers.push(...inner.split(',').map((x) => Number(x.trim())));
    else if (/\d/.test(inner)) malformed = true;
  }
  return { numbers, malformed };
}

/**
 * Judge one sentence. `sources` is the lookup's ordered source list; a
 * source's `n` is its 1-based position.
 *
 * @returns {{keep: true, cites: number[]}|{keep: false, reason: string}}
 */
function judgeSentence(sentence, sources, normalisedSources) {
  const { numbers, malformed } = citationsIn(sentence);
  if (malformed) return { keep: false, reason: WITHHELD_REASONS.CITATION_MALFORMED };
  if (numbers.some((n) => !Number.isInteger(n) || n < 1 || n > sources.length)) {
    return { keep: false, reason: WITHHELD_REASONS.CITATION_OUT_OF_RANGE };
  }
  const cites = [...new Set(numbers)];
  if (cites.some((n) => isOverdoseSection(sources[n - 1].sectionPath))) {
    return { keep: false, reason: WITHHELD_REASONS.OVERDOSE_SECTION };
  }
  const tokens = doseTokens(normaliseDoseText(sentence));
  if (tokens.length) {
    if (!cites.length) return { keep: false, reason: WITHHELD_REASONS.DOSE_UNCITED };
    const everyTokenSourced = tokens.every((tok) => cites.some((n) => sourceHasToken(normalisedSources[n - 1], tok)));
    if (!everyTokenSourced) return { keep: false, reason: WITHHELD_REASONS.DOSE_NOT_IN_SOURCE };
  }
  return { keep: true, cites };
}

/**
 * The lookup verdict for one finished grounded reply.
 *
 * `sources` is `[{ n, docTitle, sectionPath, text, ... }]` in citation order.
 * A source whose text could not be recovered should carry `text: ''`: every
 * dose cited to it then fails the verbatim check, which is the safe direction.
 *
 * The verdict is JSON and its shape is stable (it is persisted through
 * `attach_guard` and replayed by `replayMessage`):
 *
 *   kind          'lookup' — what tells a lookup verdict from a triage one
 *   rule          'dose-cite-v1'
 *   outcome       'grounded', or 'noEvidence' when nothing citable was left
 *   displayText   what is shown and what `attach_guard` writes as the row text
 *   rawReply      the reply as the model wrote it
 *   kept          the sentences shown, verbatim
 *   withheld      [{ sentence, reason }] in reply order
 *   citations     the source numbers the KEPT sentences cite, ascending
 *   detectorsSha  the detectors pin this module's number words come from
 */
export function applyLookupGuard({ replyText = '', sources = [] } = {}) {
  const raw = String(replyText ?? '');
  const list = Array.isArray(sources) ? sources : [];
  const normalisedSources = list.map((s) => normaliseDoseText(s && s.text));
  const kept = [];
  const withheld = [];
  const cited = new Set();
  let display = '';
  let lastWasWithheld = false;
  for (const { text, sep } of splitSentences(raw)) {
    const verdict = judgeSentence(text, list, normalisedSources);
    if (verdict.keep) {
      kept.push(text);
      verdict.cites.forEach((n) => cited.add(n));
      display += text + sep;
      lastWasWithheld = false;
    } else {
      withheld.push({ sentence: text, reason: verdict.reason });
      if (lastWasWithheld) display = display.replace(/\s*$/, '') + sep;
      else display += WITHHELD_BANNER + sep;
      lastWasWithheld = true;
    }
  }
  const citations = [...cited].sort((a, b) => a - b);
  const noEvidence = citations.length === 0;
  return {
    kind: 'lookup',
    rule: LOOKUP_RULE,
    outcome: noEvidence ? 'noEvidence' : 'grounded',
    displayText: noEvidence ? LOOKUP_NO_EVIDENCE_TEXT : display.trim(),
    rawReply: raw,
    kept,
    withheld,
    citations,
    detectorsSha: pin.sha256,
  };
}
