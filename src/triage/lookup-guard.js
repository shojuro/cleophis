// src/triage/lookup-guard.js — the LOOKUP guard: rule `dose-cite-v1`.
//
// Phase 1h M6. The reference lookup lets the model state a dose only when the
// dose is CITED from the bundled reference pack. This module is the product's
// check on that, applied to a finished grounded reply before anything reaches
// the screen. It is NOT `applyGuard`: a lookup reply has no route and gets no
// triage banner. Pure: no DOM, no Tauri.
//
// THIS FILE IS THE CANONICAL RULE. The triage repo's `probes/dose-cite.mjs`
// (Task T4) vendors it and asserts identity over every file in
// `src/triage/fixtures/lookup-guard/` (see that directory's manifest.json: one
// sha256 and one case count per file). Change the rule here, update the
// fixtures, re-vendor. Never fork it.
//
// The founder's rule, read strictly: a dose the source does not state, in any
// spelling, is never shown uncited. So every doubt resolves toward WITHHOLDING.
//
// The rule, per sentence of the reply:
//   1. every `[n]` must name a source in range (1..sources.length). An
//      out-of-range or unparseable citation withholds the sentence;
//   2. every DOSE TOKEN must sit in a sentence that cites, and must EQUAL, as a
//      whole canonical token, a dose token of one of the sources that sentence
//      cites. Tokens come from `doseScan` over `normaliseDoseText`: an amount
//      with a unit from `DOSE_UNITS` (ranges kept as ranges, fractions kept as
//      fractions, per-kg and concentration suffixes kept), a count of a
//      strength ("1-2 500mg tablets"), or a frequency form. A number beside a
//      dose-like word that is NOT in the table cannot be verified and withholds
//      the sentence (the fail-closed backstop);
//   3. a sentence citing a source whose section path matches
//      `OVERDOSE_SECTION_PATTERNS` is withheld, dose or not;
//   4. a withheld sentence is replaced by `WITHHELD_BANNER` (a run of withheld
//      sentences by one banner). Everything else is kept as written;
//   5. if no kept sentence with content of its own carries a valid citation,
//      nothing citable is left and the reply is `LOOKUP_NO_EVIDENCE_TEXT`.
//
// Precedence when a sentence breaks more than one clause: the first of
// citation-malformed, citation-out-of-range, overdose-section, dose-uncited,
// dose-unrecognised, dose-not-in-source is the recorded reason.
import pin from './detectors.pin.js';

export const LOOKUP_RULE = 'dose-cite-v1';

/**
 * The scripted refusal for a lookup with no evidence. Defined ONCE, here, and
 * used for all no-evidence paths: retrieval found nothing (`rag_lookup` →
 * noEvidence), and a grounded reply with nothing citable left. Product
 * wording, deliberately not the contract's `refusal_with_offer` (the contract
 * text is never re-typed outside its TOML).
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
  DOSE_UNRECOGNISED: 'dose-unrecognised',
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

// ── The unit table ──────────────────────────────────────────────────────────
//
// THE ONE TABLE of dose units the rule recognises. Each row is a DISTINCT
// canonical unit: spellings collapse to their canon, and no unit is ever
// converted into another (mg stays mg, g stays g, a teaspoon is not 5 ml).
// `kind` says whether the unit is a STRENGTH (a quantity of drug) or a COUNT
// (a number of things taken). Row order is application order: a longer or more
// specific unit comes before one it contains (mcg before g, ml before l).
// `examples` are spellings the test enumerates; fixtures/lookup-guard/units.json
// mirrors this table and a test asserts they are equal.

export const DOSE_UNITS = Object.freeze([
  { canon: 'mcg', kind: 'strength', spellings: 'micrograms?|microgrammes?|mcgs?|µg|μg|ug', examples: ['micrograms', 'microgramme', 'mcg', 'µg', 'μg', 'ug'] },
  { canon: 'ng', kind: 'strength', spellings: 'nanograms?|nanogrammes?|ng', examples: ['nanograms', 'nanogramme', 'ng'] },
  { canon: 'mg', kind: 'strength', spellings: 'milligrams?|milligrammes?|mgs?', examples: ['milligram', 'milligrammes', 'mg', 'mgs'] },
  { canon: 'mcl', kind: 'strength', spellings: 'microlit(?:re|er)s?|µl|μl|ul', examples: ['microlitres', 'microliter', 'µl', 'ul'] },
  { canon: 'ml', kind: 'strength', spellings: 'millilit(?:re|er)s?|mls?', examples: ['millilitres', 'milliliter', 'ml', 'mls'] },
  { canon: 'litres', kind: 'strength', spellings: 'lit(?:re|er)s?', examples: ['litre', 'liters'] },
  { canon: 'cc', kind: 'strength', spellings: 'cc', examples: ['cc'] },
  { canon: 'mmol', kind: 'strength', spellings: 'millimoles?|mmols?', examples: ['millimoles', 'mmol'] },
  { canon: 'iu', kind: 'strength', spellings: 'international units?|iu', examples: ['international units', 'IU'] },
  { canon: 'units', kind: 'strength', spellings: 'units?', examples: ['unit', 'units'] },
  { canon: 'g', kind: 'strength', spellings: 'grams?|grammes?|gms?|g', examples: ['gram', 'grammes', 'gm', 'g'] },
  { canon: '%', kind: 'strength', spellings: 'per ?cent|%', examples: ['percent', 'per cent', '%'] },
  { canon: 'tablets', kind: 'count', spellings: 'tablets?|tabs?', examples: ['tablet', 'tablets', 'tab', 'tabs'] },
  { canon: 'caplets', kind: 'count', spellings: 'caplets?', examples: ['caplet', 'caplets'] },
  { canon: 'capsules', kind: 'count', spellings: 'capsules?|caps?', examples: ['capsule', 'caps'] },
  { canon: 'pills', kind: 'count', spellings: 'pills?', examples: ['pill', 'pills'] },
  { canon: 'sachets', kind: 'count', spellings: 'sachets?', examples: ['sachet', 'sachets'] },
  { canon: 'drops', kind: 'count', spellings: 'drops?', examples: ['drop', 'drops'] },
  { canon: 'puffs', kind: 'count', spellings: 'puffs?', examples: ['puff', 'puffs'] },
  { canon: 'sprays', kind: 'count', spellings: 'sprays?', examples: ['spray', 'sprays'] },
  { canon: 'patches', kind: 'count', spellings: 'patch(?:es)?', examples: ['patch', 'patches'] },
  { canon: 'suppositories', kind: 'count', spellings: 'suppositor(?:y|ies)', examples: ['suppository', 'suppositories'] },
  { canon: 'pessaries', kind: 'count', spellings: 'pessar(?:y|ies)', examples: ['pessary', 'pessaries'] },
  { canon: 'lozenges', kind: 'count', spellings: 'lozenges?', examples: ['lozenge', 'lozenges'] },
  { canon: 'pastilles', kind: 'count', spellings: 'pastilles?', examples: ['pastille', 'pastilles'] },
  { canon: 'ampoules', kind: 'count', spellings: 'ampoules?|ampules?', examples: ['ampoule', 'ampules'] },
  { canon: 'vials', kind: 'count', spellings: 'vials?', examples: ['vial', 'vials'] },
  { canon: 'injections', kind: 'count', spellings: 'injections?', examples: ['injection', 'injections'] },
  { canon: 'doses', kind: 'count', spellings: 'doses?', examples: ['dose', 'doses'] },
  { canon: 'teaspoons', kind: 'count', spellings: 'teaspoon(?:ful)?s?|tsps?', examples: ['teaspoon', 'teaspoonfuls', 'tsp'] },
  { canon: 'tablespoons', kind: 'count', spellings: 'tablespoon(?:ful)?s?|tbsps?', examples: ['tablespoons', 'tablespoonful', 'tbsp'] },
  { canon: 'spoonfuls', kind: 'count', spellings: 'spoon(?:ful)?s?', examples: ['spoon', 'spoonfuls'] },
].map((u) => Object.freeze({ ...u, examples: Object.freeze([...u.examples]) })));

const ALL_SPELLINGS = DOSE_UNITS.map((u) => u.spellings).join('|');
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\%]/g, '\\$&');
const CANON_ALT = (kind) => DOSE_UNITS.filter((u) => !kind || u.kind === kind).map((u) => esc(u.canon)).join('|');
const STRENGTH_ALT = CANON_ALT('strength');
const COUNT_ALT = CANON_ALT('count');
const UNIT_ALT = CANON_ALT();
const AFTER_UNIT = '(?![a-z0-9%])';

// ── Normalisation ───────────────────────────────────────────────────────────
//
// Applied identically to the reply and to the source. It changes SPELLING only,
// never quantity. Steps, in order (fixtures/lookup-guard/normalise.json pins
// the result of the whole chain):
//   invisible characters out; vulgar fractions to "1/2" etc. (before NFKC, which
//   would glue "1½" into "11⁄2"); NFKC; lower case; fraction slash and dashes to
//   ASCII; thousands separators out; ".5" to "0.5"; number words (zero to the
//   thousands) to digits; "half"/"quarter" to 1/2 and 1/4; once/twice/thrice;
//   per-day and per-week; ranges "N to M"/"N - M"/"N or M" to "N-M"; hours;
//   "a"/"an" before a unit to 1; unit spellings glued to their number as their
//   canon; a count word after a strength to its canon; concentrations
//   ("250 mg in 5 ml") to "250mg/5ml"; per-kilogram forms to "/kg".

const VULGAR = { '½': '1/2', '¼': '1/4', '¾': '3/4', '⅓': '1/3', '⅔': '2/3', '⅛': '1/8' };

const SMALL = {
  zero: 0, one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, seven: 7, eight: 8, nine: 9, ten: 10,
  eleven: 11, twelve: 12, thirteen: 13, fourteen: 14, fifteen: 15, sixteen: 16, seventeen: 17,
  eighteen: 18, nineteen: 19,
};
const TENS = { twenty: 20, thirty: 30, forty: 40, fifty: 50, sixty: 60, seventy: 70, eighty: 80, ninety: 90 };
const NUMBER_WORD = `(?:${[...Object.keys(SMALL), ...Object.keys(TENS), 'hundred', 'thousand'].join('|')})`;
const NUMBER_RUN = new RegExp(`\\b${NUMBER_WORD}(?:(?:[ \\t]*-[ \\t]*|[ \\t]+and[ \\t]+|[ \\t]+)${NUMBER_WORD})*\\b`, 'g');

// "five hundred and twenty-five" -> "525"; "one two" -> "1 2"; "one-two" -> "1-2";
// "one and two" -> "1 and 2" ("and" joins only after hundred/thousand).
function numberRunToDigits(run) {
  const parts = run.split(/([ \t]*-[ \t]*|[ \t]+)/);
  const out = [];
  let total = 0; let cur = 0; let last = null;
  const flush = (sep) => {
    if (last !== null) out.push(String(total + cur), sep);
    total = 0; cur = 0; last = null;
  };
  for (let i = 0; i < parts.length; i += 2) {
    const w = parts[i];
    const sep = i > 0 ? parts[i - 1] : '';
    const joinSep = sep.includes('-') ? '-' : ' ';
    if (w === 'and') {
      if (last === 'hundred' || last === 'thousand') continue;
      flush(' '); out.push('and', ' ');
      continue;
    }
    if (w in SMALL) {
      const v = SMALL[w];
      if (last === 'small' || last === 'teen' || (last === 'tens' && v >= 10)) flush(joinSep);
      cur += v; last = v < 10 ? 'small' : 'teen';
    } else if (w in TENS) {
      if (last === 'small' || last === 'teen' || last === 'tens') flush(joinSep);
      cur += TENS[w]; last = 'tens';
    } else if (w === 'hundred') {
      cur = (cur || 1) * 100; last = 'hundred';
    } else if (w === 'thousand') {
      total += (cur || 1) * 1000; cur = 0; last = 'thousand';
    }
  }
  flush('');
  return out.join('').replace(/\s+$/, '');
}

const UNIT_GLUE = DOSE_UNITS.map((u) => [u.canon, new RegExp(`(\\d)[ \\t]*(?:${u.spellings})${AFTER_UNIT}`, 'g')]);
const COUNT_AFTER_STRENGTH = DOSE_UNITS.filter((u) => u.kind === 'count')
  .map((u) => [u.canon, new RegExp(`((?:${STRENGTH_ALT})(?:\\/[a-z0-9.]+)?)[ \\t]+(?:${u.spellings})${AFTER_UNIT}`, 'g')]);

/**
 * The spelling normalisation both sides of the check go through.
 * Exported so the fixtures can pin it and the triage port can match it.
 */
export function normaliseDoseText(text) {
  let s = String(text ?? '');
  s = s.replace(/[​-‍⁠﻿­]/g, '');
  s = s.replace(/(\d?)[ \t]*([½¼¾⅓⅔⅛])/g, (m, d, f) => `${d ? `${d} ` : ' '}${VULGAR[f]}`);
  s = s.normalize('NFKC').toLowerCase();
  s = s.replace(/⁄/g, '/');
  s = s.replace(/[‐‑‒–—−]/g, '-');
  for (let prev = null; prev !== s;) { prev = s; s = s.replace(/(\d),(\d{3})(?!\d)/g, '$1$2'); }
  s = s.replace(/(^|[^\d])\.(\d)/g, '$10.$2');
  s = s.replace(NUMBER_RUN, numberRunToDigits);
  // fractions in words
  s = s.replace(/\b(\d+)[ \t]+and[ \t]+(?:a[ \t]+|1[ \t]+)?half\b/g, '$1 1/2');
  s = s.replace(/\b(\d+)[ \t]+and[ \t]+(?:a[ \t]+|1[ \t]+)?quarter\b/g, '$1 1/4');
  s = s.replace(/\b(\d+)[ \t]+quarters\b/g, '$1/4');
  s = s.replace(/\b(?:an?[ \t]+|1[ \t]+)?half(?:[ \t]+of)?(?:[ \t]+an?\b)?/g, '1/2');
  s = s.replace(/\b(?:an?[ \t]+|1[ \t]+)?quarter(?:[ \t]+of)?(?:[ \t]+an?\b)?/g, '1/4');
  s = s.replace(/\b(\d+(?: \d+)?\/\d+)[ \t]+of[ \t]+(?:an?|the)\b/g, '$1');
  // "once" is a frequency only when a period follows ("once a day", "once
  // every 4 hours"); "once you feel better" is not a dose
  s = s.replace(/\bonce(?=[ \t]+(?:(?:a|per|each|every)[ \t]+(?:day|week)\b|daily\b|weekly\b|(?:in|every)[ \t]+\d))/g, '1 times');
  s = s.replace(/\btwice\b/g, '2 times').replace(/\bthrice\b/g, '3 times');
  s = s.replace(/\b(\d+) time\b/g, '$1 times');
  // per-period forms
  s = s.replace(/[ \t]*\b(?:a|per|each|every)[ \t]+day\b/g, '/day').replace(/[ \t]+daily\b/g, '/day');
  s = s.replace(/[ \t]*\b(?:a|per|each|every)[ \t]+week\b/g, '/week').replace(/[ \t]+weekly\b/g, '/week');
  // ranges: "4 to 6", "4 - 6", "1 or 2" -> "4-6"
  s = s.replace(/(\d)[ \t]*(?:-|\bto\b|\bor\b)[ \t]*(\d)/g, '$1-$2');
  // hours
  s = s.replace(/(\d)[ \t]*(?:hours?|hrs?|h)(?![a-z0-9])/g, '$1 hours');
  // "a gram", "an injection" -> "1g", "1injections"
  s = s.replace(new RegExp(`\\ban?[ \\t]+(?=(?:${ALL_SPELLINGS})${AFTER_UNIT})`, 'g'), '1 ');
  for (const [canon, re] of UNIT_GLUE) s = s.replace(re, `$1${canon}`);
  for (const [canon, re] of COUNT_AFTER_STRENGTH) s = s.replace(re, `$1 ${canon}`);
  // concentrations: "250mg in 5ml", "250mg per 5ml" -> "250mg/5ml"
  s = s.replace(new RegExp(`(${STRENGTH_ALT})[ \\t]*(?:\\/|\\bper\\b|\\bin\\b(?:[ \\t]+(?:each|every))?)[ \\t]*(\\d+(?:\\.\\d+)?)(ml)${AFTER_UNIT}`, 'g'), '$1/$2$3');
  // per kilogram: "10mg per kilogram of body weight" -> "10mg/kg"
  s = s.replace(new RegExp(`(${STRENGTH_ALT})[ \\t]*(?:\\/|\\bper\\b|\\ba\\b|\\bfor (?:each|every)\\b|\\beach\\b)[ \\t]*(?:kg|kilograms?|kilogrammes?|kilos?)\\b(?:[ \\t]+of[ \\t]+body[ \\t]*weight)?`, 'g'), '$1/kg');
  s = s.replace(new RegExp(`(${STRENGTH_ALT})[ \\t]*\\/[ \\t]*(kg|dose|day|week)\\b`, 'g'), '$1/$2');
  return s.replace(/\s+/g, ' ').trim();
}

// ── Dose tokens ─────────────────────────────────────────────────────────────

const NUM = '(?:\\d+ \\d+\\/\\d+|\\d+\\/\\d+|\\d+(?:\\.\\d+)?)';
const NUMR = `${NUM}(?:-${NUM})?`;
const SUFFIX = `(?:\\/(?:${NUM})?(?:kg|dose|day|week|hours|ml|mg|g|m2))?`;

/** The token forms, in the order `doseScan` reports them. Exported for the port. */
export const DOSE_TOKEN_FORMS = Object.freeze([
  // a count of a strength: "1-2 500mg tablets"
  Object.freeze({ name: 'count-of-strength', re: new RegExp(`(?<![\\d./])${NUMR} ${NUM}(?:${STRENGTH_ALT})${SUFFIX} (?:${COUNT_ALT})${AFTER_UNIT}`, 'g') }),
  // an amount with a unit: "500mg", "1/2tablets", "10mg/kg", "250mg/5ml", "1-2puffs"
  Object.freeze({ name: 'amount', re: new RegExp(`(?<![\\d./])${NUMR}(?:${UNIT_ALT})${SUFFIX}${AFTER_UNIT}`, 'g') }),
  // a frequency: "3 times/day", "4 times in 24 hours", "3 times"
  Object.freeze({ name: 'frequency', re: new RegExp(`(?<![\\d./])${NUMR} times(?:\\/(?:day|week)| (?:in|per|every) ${NUMR} hours)?`, 'g') }),
  // an interval: "every 4-6 hours"; a gap: "4 hours between doses", "at least 4 hours apart"
  Object.freeze({ name: 'interval', re: new RegExp(`\\bevery ${NUMR} hours`, 'g') }),
  Object.freeze({ name: 'gap', re: new RegExp(`(?<![\\d./])${NUMR} hours (?:between doses|apart)`, 'g') }),
]);

// The fail-closed backstop: a number directly before a word that LOOKS like a
// dose unit but is not in `DOSE_UNITS` (ounces, grains, pumps, "mgm", a
// misspelling). Such a quantity cannot be verified, so it withholds.
const DOSE_LIKE_WORD = /^(?:pill|tab|cap|spoon|tsp|tbsp|drop|puff|spray|squirt|pump|patch|dose|dosage|gram|gramme|gm|milli|micro|nano|kilo|lit|unit|mol|mmol|sachet|lozenge|pastille|ampoule|ampule|amp|vial|inject|jab|shot|suppos|pessar|oz|ounce|dram|grain|minim|dab|mg|mcg|ug|ml|cc|iu|ng)/;
const NUMBER_THEN_WORD = /(?<![\d./])(\d+(?:[./]\d+)?)[ \t]*([a-z]+)/g;

/**
 * Every dose token in already-normalised text, and every quantity the backstop
 * found that no token covers.
 *
 * @returns {{tokens: string[], unrecognised: string[]}}
 */
export function doseScan(normalised) {
  const spans = [];
  for (const form of DOSE_TOKEN_FORMS) {
    for (const m of normalised.matchAll(form.re)) spans.push({ token: m[0], start: m.index, end: m.index + m[0].length });
  }
  spans.sort((a, b) => a.start - b.start || b.end - a.end);
  const unrecognised = [];
  for (const m of normalised.matchAll(NUMBER_THEN_WORD)) {
    if (!DOSE_LIKE_WORD.test(m[2])) continue;
    const covered = spans.some((sp) => m.index >= sp.start && m.index < sp.end);
    if (!covered) unrecognised.push(m[0]);
  }
  return { tokens: spans.map((sp) => sp.token), unrecognised };
}

/** Every dose token in already-normalised text, in order of appearance. */
export function doseTokens(normalised) {
  return doseScan(normalised).tokens;
}

/**
 * Is `token` one of normalised `source`'s own dose tokens? WHOLE-TOKEN
 * EQUALITY, never a substring: "50mg" is not in "500mg", "2tablets" is not in
 * "1/2tablets", "2tablets" is not in "1-2tablets" (a range endpoint is not
 * the range), and "10mg" is not in "10mg/kg".
 */
export function sourceHasToken(source, token) {
  return doseTokens(source).includes(token);
}

// ── Sentences and citations ─────────────────────────────────────────────────

const ABBREVIATION_END = /(?:^|[\s(])(?:e\.g|i\.e|approx|vs)$/i;
const TRAILING_CITATION = /^[ \t]*\[[^\]\n]*\]/;
// a citation cluster alone on the NEXT line (exactly one newline away)
const CITATION_LINE = /^[ \t]*\n[ \t]*(?:\[[^\]\n]*\][ \t]*)+(?=\n|$)/;

/**
 * Split a reply into sentences, each with the whitespace that followed it, so
 * the kept text is reassembled exactly as written. A sentence ends at `.`, `!`
 * or `?` followed by whitespace or the end, or at a newline. A citation written
 * AFTER the full stop ("... doses. [1]"), or alone on the next line, belongs to
 * the sentence before it. A decimal point is not an end ("2.5 ml"), nor are
 * "e.g." and "i.e.".
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
  const endAt = (k) => {
    let j = k;
    while (j < text.length && /\s/.test(text[j])) j++;
    push(k, j);
  };
  while (i < text.length) {
    const ch = text[i];
    if (ch === '\n') {
      const cite = CITATION_LINE.exec(text.slice(i));
      endAt(cite ? i + cite[0].replace(/[ \t]+$/, '').length : i);
      continue;
    }
    if ('.!?'.includes(ch) && !(ch === '.' && ABBREVIATION_END.test(text.slice(start, i)))) {
      let k = i + 1;
      while (k < text.length && /[.!?"'”’)]/.test(text[k])) k++;
      for (let m = TRAILING_CITATION.exec(text.slice(k)); m; m = TRAILING_CITATION.exec(text.slice(k))) {
        k += m[0].length;
      }
      if (k >= text.length || /\s/.test(text[k])) {
        const cite = CITATION_LINE.exec(text.slice(k));
        endAt(cite ? k + cite[0].replace(/[ \t]+$/, '').length : k);
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

// A sentence that is nothing but citations and punctuation says nothing of its
// own, so it cannot make a reply "grounded" (clause 5).
const hasOwnContent = (sentence) => /[\p{L}\p{N}]/u.test(String(sentence).replace(CITATION_BRACKET, ''));

/**
 * Judge one sentence. `sources` is the lookup's ordered source list; a
 * source's `n` is its 1-based position.
 *
 * @returns {{keep: true, cites: number[]}|{keep: false, reason: string}}
 */
function judgeSentence(sentence, sources, sourceTokens) {
  const { numbers, malformed } = citationsIn(sentence);
  if (malformed) return { keep: false, reason: WITHHELD_REASONS.CITATION_MALFORMED };
  if (numbers.some((n) => !Number.isInteger(n) || n < 1 || n > sources.length)) {
    return { keep: false, reason: WITHHELD_REASONS.CITATION_OUT_OF_RANGE };
  }
  const cites = [...new Set(numbers)];
  if (cites.some((n) => isOverdoseSection(sources[n - 1] && sources[n - 1].sectionPath))) {
    return { keep: false, reason: WITHHELD_REASONS.OVERDOSE_SECTION };
  }
  const { tokens, unrecognised } = doseScan(normaliseDoseText(sentence));
  if (tokens.length || unrecognised.length) {
    if (!cites.length) return { keep: false, reason: WITHHELD_REASONS.DOSE_UNCITED };
    if (unrecognised.length) return { keep: false, reason: WITHHELD_REASONS.DOSE_UNRECOGNISED };
    const everyTokenSourced = tokens.every((tok) => cites.some((n) => sourceTokens[n - 1].has(tok)));
    if (!everyTokenSourced) return { keep: false, reason: WITHHELD_REASONS.DOSE_NOT_IN_SOURCE };
  }
  return { keep: true, cites };
}

/**
 * The lookup verdict for one finished grounded reply.
 *
 * `sources` is `[{ n, docTitle, sectionPath, text, ... }]` in citation order.
 * A source whose text could not be recovered should carry `text: ''`: every
 * dose cited to it then fails, which is the safe direction.
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
 *   detectorsSha  the detectors pin in force when the verdict was made
 */
export function applyLookupGuard({ replyText = '', sources = [] } = {}) {
  const raw = String(replyText ?? '');
  const list = Array.isArray(sources) ? sources : [];
  const sourceTokens = list.map((s) => new Set(doseTokens(normaliseDoseText(s && s.text))));
  const kept = [];
  const withheld = [];
  const cited = new Set();
  let display = '';
  let lastWasWithheld = false;
  for (const { text, sep } of splitSentences(raw)) {
    const verdict = judgeSentence(text, list, sourceTokens);
    if (verdict.keep) {
      kept.push(text);
      if (hasOwnContent(text)) verdict.cites.forEach((n) => cited.add(n));
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
