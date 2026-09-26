// src/triage/lookup-guard.js — the LOOKUP guard: rule `dose-cite-v2`.
//
// Phase 1h M6. The reference lookup lets the model state a dose only when the
// dose is CITED from the bundled reference pack. This module is the product's
// check on that, applied to a finished grounded reply before anything reaches
// the screen. It is NOT `applyGuard`: a lookup reply has no route and gets no
// triage banner. Pure: no DOM, no Tauri.
//
// THIS FILE IS THE CANONICAL RULE. The triage repo's `probes/dose-cite.mjs`
// (Task T4) vendors it and asserts identity over every file in
// `src/triage/fixtures/lookup-guard/` (manifest.json pins each file's sha256
// and case count). Change the rule here, update the fixtures, re-vendor.
//
// v2 (M6 fix round 2, controller ruling): FAIL CLOSED AT THE SENTENCE LEVEL.
// An enumerated dose grammar keeps losing to an adversary, so the rule no
// longer asks "is this a dose?". It asks "does this sentence carry a number?",
// and every number it carries must be checked.
//
// The rule, per sentence of the reply:
//   1. every `[n]` must name a source in range (1..sources.length). An
//      out-of-range or unparseable citation withholds the sentence;
//   2. a sentence citing a source whose section path matches
//      `OVERDOSE_SECTION_PATTERNS` is withheld, number or not;
//   3. a sentence is NUMBER-BEARING when, after `normaliseDoseText`, it holds
//      any numeric span (`numericScan`): a number of any kind (any Unicode
//      decimal digit, a number word, a fraction, a mixed number, a roman
//      numeral) with whatever unit, per-kg, concentration, per-period or
//      multiplier is attached to it, or a frequency word ("/day", "double",
//      "nightly"). A number-bearing sentence is KEPT only if it cites at least
//      one in-range source AND every one of its spans EQUALS, as a whole
//      string, a span of one of the sources it cites (`sourceSpans`);
//   4. a sentence with no number is kept (it states no dose);
//   5. a withheld sentence is replaced by `WITHHELD_BANNER` (a run of withheld
//      sentences by one banner);
//   6. if no kept sentence with content of its own carries a valid citation,
//      the reply is `LOOKUP_NO_EVIDENCE_TEXT`.
//
// Precedence: citation-malformed, citation-out-of-range, overdose-section,
// dose-uncited (number-bearing, no citation), dose-not-in-source (a span the
// cited sources do not hold, or a digit no span explains). v1's
// dose-unrecognised is gone: in v2 every number is a span and is checked.
import pin from './detectors.pin.js';

export const LOOKUP_RULE = 'dose-cite-v2';

/**
 * The scripted refusal for a lookup with no evidence. Defined ONCE, here, and
 * used for all no-evidence paths. Product wording, deliberately not the
 * contract's `refusal_with_offer` (the contract text is never re-typed
 * outside its TOML).
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
 * Overdose and maximum-dose material the lookup never quotes from. Matched
 * against a source's SECTION PATH (clause 2) and, since fix round 2 (N7),
 * against each SENTENCE of a source's text: a span that appears only in an
 * overdose sentence of an otherwise ordinary chunk verifies nothing.
 */
export const OVERDOSE_SECTION_PATTERNS = Object.freeze([
  /\boverdos/i,
  /\bmax(?:imum|\.)?\s+(?:daily\s+|single\s+|total\s+)?doses?\b/i,
  /\bhow much is too much\b/i,
  /\btoo much\b/i,
  /\bpoison/i,
  /\btoxicity\b/i,
]);

/** Does this section path (or source sentence) name overdose or maximum-dose material? */
export function isOverdoseSection(sectionPath) {
  const s = String(sectionPath ?? '');
  return OVERDOSE_SECTION_PATTERNS.some((re) => re.test(s));
}

// ── The unit table ──────────────────────────────────────────────────────────
//
// THE ONE TABLE of dose units. Each row is a DISTINCT canonical unit:
// spellings collapse to their canon and no unit is converted into another.
// In v2 the table no longer decides WHETHER a sentence is checked (every number
// is); it decides how a number and its unit are written as one span, so that
// "500 milligrams" and "500mg" are the same span and "500mg" is not "5mg".
// Row order is application order. fixtures/lookup-guard/units.json mirrors it.

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
// A unit ends where no letter, digit, % or / follows: "10mg/kg/day" can never
// be read as the shorter "10mg/kg" (N3).
const AFTER_UNIT = '(?![a-z0-9%/])';

// ── Normalisation ───────────────────────────────────────────────────────────
//
// Applied identically to the reply and to the source. Spelling only, never
// quantity. The steps, in order (normalise.json pins the whole chain):
//    1. invisible characters out; markdown emphasis (* _ `) and table pipes to
//       spaces; parentheses to spaces (so "(500)mg" and "500 (mg)" read as
//       "500 mg"); a space before a vulgar fraction that follows a digit;
//    2. NFKC; every Unicode decimal digit to ASCII; lower case; fraction slash
//       and dashes to ASCII; dotted or spaced units ("m.g.", "m g", "mc g",
//       "m.l.") to their letters;
//    3. thousands separators out; ".5" to "0.5";
//    4. pronoun "one" ("no one", "the one", "this one"...) kept as a word;
//       number words (zero to the thousands) to digits; "a dozen" to 12 and
//       "N dozen" to "Nx12";
//    5. "half"/"quarter" forms to fractions; "N/M of a" to "N/M";
//       once (only before a period)/twice/thrice to "N times";
//    6. ranges ("N to M", "N - M", "N or M") to "N-M";
//    7. hours, minutes, days; "every hour"/"hourly" to "every 1 hours";
//       "every other day" to "every 2 days";
//    8. "a"/"an" before a unit to 1; unit spellings glued to their number as
//       their canon, across a hyphen ("a 500-mg tablet");
//    9. a count word after a strength to its canon; "a 500mg tablet" to
//       "1 500mg tablets";
//   10. multipliers: "2 x 500mg", "4 × 500mg", "tablets x 8" to one product
//       span ("2x500mg", "tabletsx8");
//   11. concentrations ("250mg in 5ml") to "250mg/5ml";
//   12. per-kilogram forms ("per (every|each|1) kilogram (of body weight)",
//       "every kg", "a kilo") to "/kg";
//   13. LAST, per-period forms ("a/per/each/every day", "daily", weekly) to
//       "/day", "/week", so "10 mg/kg per day" becomes "10mg/kg/day" (N3).

const INVISIBLE = /[­͏؜ᅟᅠ឴឵᠎​-‏‪-‮⁠-⁤⁪-⁯﻿]/g;
const VULGAR_FRACTION = /[¼-¾⅐-⅟↉]/;

// Unicode decimal digits -> ASCII. A \p{Nd} run is ten consecutive code points
// 0..9, so a digit's value is how many Nd code points precede it in its run.
const ND = /\p{Nd}/u;
function asciiDigits(s) {
  return s.replace(/\p{Nd}/gu, (ch) => {
    const cp = ch.codePointAt(0);
    if (cp >= 0x30 && cp <= 0x39) return ch;
    let k = 0;
    while (k < 10 && ND.test(String.fromCodePoint(cp - k - 1))) k++;
    return String(k % 10);
  });
}

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

// Gluing allows a "/" after the unit ("15 mg/kg" -> "15mg/kg"); only the span
// scan forbids one, so a truncated span can never match.
const UNIT_GLUE = DOSE_UNITS.map((u) => [u.canon, new RegExp(`(\\d)[ \\t]*-?[ \\t]*(?:${u.spellings})(?![a-z0-9%])`, 'g')]);
const COUNT_AFTER_STRENGTH = DOSE_UNITS.filter((u) => u.kind === 'count')
  .map((u) => [u.canon, new RegExp(`((?:${STRENGTH_ALT})(?:\\/[a-z0-9.]+)*)[ \\t]+(?:${u.spellings})(?![a-z0-9%])`, 'g')]);

/**
 * The spelling normalisation both sides of the check go through.
 * Exported so the fixtures can pin it and the triage port can match it.
 */
export function normaliseDoseText(text) {
  let s = String(text ?? '');
  s = s.replace(INVISIBLE, '');
  s = s.replace(/[*_`|()]/g, ' ');
  s = s.replace(new RegExp(`(\\d)(?=${VULGAR_FRACTION.source})`, 'g'), '$1 ');
  s = asciiDigits(s.normalize('NFKC')).toLowerCase();
  s = s.replace(/⁄/g, '/');
  s = s.replace(/[‐‑‒–—−]/g, '-');
  s = s.replace(/\bm\.?[ \t]?c\.?[ \t]?g\b\.?/g, 'mcg').replace(/\bm\.[ \t]?g\b\.?|\bm[ \t]g\b/g, 'mg').replace(/\bm\.[ \t]?l\b\.?/g, 'ml');
  for (let prev = null; prev !== s;) { prev = s; s = s.replace(/(\d),(\d{3})(?!\d)/g, '$1$2'); }
  s = s.replace(/(^|[^\d])\.(\d)/g, '$10.$2');
  s = s.replace(/\b(no|the|this|that|which|each|any|every|another|other)[ \t]+one\b/g, '$1 pronone');
  s = s.replace(NUMBER_RUN, numberRunToDigits);
  s = s.replace(/\b(\d+)[ \t]+dozen\b/g, '$1x12').replace(/\b(?:an?|1)[ \t]+dozen\b/g, '12').replace(/\bdozen\b/g, '12');
  s = s.replace(/\b(\d+)[ \t]+and[ \t]+(?:a[ \t]+|1[ \t]+)?half\b/g, '$1 1/2');
  s = s.replace(/\b(\d+)[ \t]+and[ \t]+(?:a[ \t]+|1[ \t]+)?quarter\b/g, '$1 1/4');
  s = s.replace(/\b(\d+)[ \t]+quarters\b/g, '$1/4');
  s = s.replace(/\b(?:an?[ \t]+|1[ \t]+)?half(?:[ \t]+of)?(?:[ \t]+an?\b)?/g, '1/2');
  s = s.replace(/\b(?:an?[ \t]+|1[ \t]+)?quarter(?:[ \t]+of)?(?:[ \t]+an?\b)?/g, '1/4');
  s = s.replace(/\b(\d+(?: \d+)?\/\d+)[ \t]+of[ \t]+(?:an?|the)\b/g, '$1');
  s = s.replace(/\bonce(?=[ \t]+(?:(?:a|per|each|every)[ \t]+(?:day|week)\b|daily\b|weekly\b|(?:in|every)[ \t]+\d))/g, '1 times');
  s = s.replace(/\btwice\b/g, '2 times').replace(/\bthrice\b/g, '3 times');
  s = s.replace(/\b(\d+) time\b/g, '$1 times');
  s = s.replace(/(\d)[ \t]*(?:-|\bto\b|\bor\b)[ \t]*(\d)/g, '$1-$2');
  s = s.replace(/(\d)[ \t]*(?:hours?|hrs?|h)(?![a-z0-9])/g, '$1 hours');
  s = s.replace(/(\d)[ \t]*(?:minutes?|mins?)(?![a-z0-9])/g, '$1 minutes');
  s = s.replace(/(\d)[ \t]*days?(?![a-z0-9])/g, '$1 days');
  s = s.replace(/\bevery[ \t]+hour\b|\bhourly\b/g, 'every 1 hours');
  s = s.replace(/\bevery[ \t]+other[ \t]+day\b/g, 'every 2 days');
  s = s.replace(new RegExp(`\\ban?[ \\t]+(?=(?:${ALL_SPELLINGS})(?![a-z0-9%]))`, 'g'), '1 ');
  for (const [canon, re] of UNIT_GLUE) s = s.replace(re, `$1${canon}`);
  for (const [canon, re] of COUNT_AFTER_STRENGTH) s = s.replace(re, `$1 ${canon}`);
  // "a 500mg tablet" states a count of one: "1 500mg tablets"
  s = s.replace(new RegExp(`\\ban?[ \\t]+(?=\\d+(?:\\.\\d+)?(?:${STRENGTH_ALT})(?:\\/[a-z0-9.]+)* (?:${COUNT_ALT})(?![a-z0-9%]))`, 'g'), '1 ');
  s = s.replace(new RegExp(`(\\d|\\b(?:${ALL_SPELLINGS}))[ \\t]*[x×*][ \\t]*(?=\\d)`, 'g'), '$1x');
  s = s.replace(new RegExp(`(${STRENGTH_ALT})[ \\t]*(?:\\/|\\bper\\b|\\bin\\b(?:[ \\t]+(?:each|every))?)[ \\t]*(\\d+(?:\\.\\d+)?)(ml)(?![a-z0-9%])`, 'g'), '$1/$2$3');
  s = s.replace(new RegExp(`(${STRENGTH_ALT})[ \\t]*(?:\\/|\\bper\\b(?:[ \\t]+(?:each|every|1))?|\\ba\\b|\\bfor[ \\t]+(?:each|every)\\b|\\beach\\b|\\bevery\\b)[ \\t]*(?:kg|kilograms?|kilogrammes?|kilos?)\\b(?:[ \\t]+of[ \\t]+body[ \\t]*weight)?`, 'g'), '$1/kg');
  s = s.replace(/[ \t]*\b(?:a|per|each|every)[ \t]+day\b/g, '/day').replace(/[ \t]+daily\b/g, '/day');
  s = s.replace(/[ \t]*\b(?:a|per|each|every)[ \t]+week\b/g, '/week').replace(/[ \t]+weekly\b/g, '/week');
  s = s.replace(new RegExp(`(${STRENGTH_ALT}|kg)[ \\t]*\\/[ \\t]*(kg|dose|day|week)\\b`, 'g'), '$1/$2');
  return s.replace(/\s+/g, ' ').trim();
}

// ── Numeric spans ───────────────────────────────────────────────────────────

const NUM = '(?:\\d+ \\d+\\/\\d+|\\d+\\/\\d+|\\d+(?:\\.\\d+)?)';
const NUMR = `${NUM}(?:-${NUM})?`;
const SUFFIX = `(?:\\/(?:${NUM})?(?:kg|dose|day|week|hours|minutes|ml|mg|g|m2))*`;
const CORE = `${NUMR}(?:(?:${UNIT_ALT})${SUFFIX})?`;
const PERIOD_UNIT = '(?:hours|minutes|days|weeks)';

/**
 * The span forms, tried in this order at each position (the first that matches
 * there wins; spans never overlap). Exported for the port.
 */
export const NUMERIC_SPAN_FORMS = Object.freeze([
  // a multiplier and its factors, one span: "2x500mg", "500mgx2", "tabletsx8", "2x12tablets"
  Object.freeze({ name: 'product', src: `(?:${CORE}|\\b(?:${ALL_SPELLINGS}))(?:x${CORE})+${AFTER_UNIT}` }),
  // a count of a strength: "1-2 500mg tablets"
  Object.freeze({ name: 'count-of-strength', src: `${NUMR} ${NUM}(?:${STRENGTH_ALT})${SUFFIX} (?:${COUNT_ALT})(?![a-z0-9%])` }),
  // a frequency: "3 times/day", "4 times in 24 hours", "1 times every 4 hours", "3 times"
  Object.freeze({ name: 'frequency', src: `${NUMR} times(?:\\/(?:day|week)| (?:in|per) ${NUMR} ${PERIOD_UNIT}| every ${NUMR} ${PERIOD_UNIT})?` }),
  // an interval: "every 4-6 hours", "every 30 minutes", "every 2 days"
  Object.freeze({ name: 'interval', src: `\\bevery ${NUMR} ${PERIOD_UNIT}` }),
  // a gap: "4 hours between doses", "4 hours apart"
  Object.freeze({ name: 'gap', src: `${NUMR} ${PERIOD_UNIT} (?:between doses|apart)` }),
  // a duration: "24 hours", "3 days"
  Object.freeze({ name: 'duration', src: `${NUMR} ${PERIOD_UNIT}` }),
  // an amount: "500mg", "1/2tablets", "10mg/kg/day", "250mg/5ml", "1-2puffs"
  Object.freeze({ name: 'amount', src: `${NUMR}(?:${UNIT_ALT})${SUFFIX}${AFTER_UNIT}` }),
  // frequency and multiplier words with no digit: "/day", "double", "nightly", roman numerals
  Object.freeze({ name: 'word', src: `\\/(?:day|week)\\b|\\b(?:double|triple|quadruple|nightly|fortnightly)\\b|\\b(?:ii|iii|vi|vii|viii|xi|xii)\\b` }),
  // any other number, with any letters glued to it: "16", "6-11", "2k", "1st"
  Object.freeze({ name: 'number', src: `${NUMR}[a-z]*(?:\\/[a-z0-9.]+)*` }),
].map((f) => Object.freeze({ ...f })));

const SPAN_RE = new RegExp(NUMERIC_SPAN_FORMS.map((f) => `(${f.src})`).join('|'), 'g');
const CITATION_BRACKET = /\[([^\]\n]*)\]/g;
const LIST_MARKER = /^\s*(?:[-•]\s*)?\d+[.)](?=\s)/;

/**
 * The numeric spans of already-normalised text, left to right, and whether a
 * digit is left over that no span explains. Citation brackets and a leading
 * list marker ("1.") are not numbers of the sentence.
 *
 * @returns {{spans: string[], unclassifiable: boolean}}
 */
export function numericScan(normalised) {
  const text = String(normalised ?? '').replace(CITATION_BRACKET, ' ').replace(LIST_MARKER, ' ');
  const spans = [];
  let rest = text;
  for (const m of text.matchAll(SPAN_RE)) {
    spans.push(m[0]);
    rest = rest.slice(0, m.index) + ' '.repeat(m[0].length) + rest.slice(m.index + m[0].length);
  }
  return { spans, unclassifiable: /\d/.test(rest) };
}

// Sub-spans a source span also states: the strength inside a count of a
// strength ("500mg" in "1-2 500mg tablets"), a factor of a product ("500mg" in
// "2x500mg"), the interval inside a frequency ("every 4 hours" in "1 times
// every 4 hours"), and a trailing period ("/day"). NEVER a range endpoint
// ("2tablets" is not stated by "1-2tablets") and never a unit-stripped number
// ("10mg" is not stated by "10mg/kg").
const COMPONENT_RES = [
  new RegExp(`(?<=^|[ x])(?:${NUMR}(?:${UNIT_ALT})${SUFFIX}${AFTER_UNIT}|every ${NUMR} ${PERIOD_UNIT})`, 'g'),
  /\/(?:day|week)$/g,
];

/**
 * Every span a source's TEXT states: the spans of each of its sentences, and
 * their components, skipping any sentence that names overdose material (N7).
 * @returns {Set<string>}
 */
export function sourceSpans(sourceText) {
  const out = new Set();
  for (const { text } of splitSentences(sourceText)) {
    if (isOverdoseSection(text)) continue;
    for (const span of numericScan(normaliseDoseText(text)).spans) {
      out.add(span);
      for (const re of COMPONENT_RES) for (const c of span.matchAll(re)) if (c[0] !== span) out.add(c[0]);
    }
  }
  return out;
}

/** Does the source text state this span, as a whole? */
export function sourceHasSpan(sourceText, span) {
  return sourceSpans(sourceText).has(span);
}

// ── Sentences and citations ─────────────────────────────────────────────────

const ABBREVIATION_END = /(?:^|[\s(])(?:e\.g|i\.e|approx|vs)$/i;
const TRAILING_CITATION = /^[ \t]*\[[^\]\n]*\]/;
// a citation cluster alone on the NEXT line (exactly one newline away)
const CITATION_LINE = /^[ \t]*\n[ \t]*(?:\[[^\]\n]*\][ \t]*)+(?=\n|$)/;
// a list marker at the start of a line ("1." or "- 2)") is not a sentence end
const MARKER_SO_FAR = /^\s*(?:[-•]\s*)?\d+$/;

/**
 * Split a reply into sentences, each with the whitespace that followed it, so
 * the kept text is reassembled exactly as written. A sentence ends at `.`, `!`
 * or `?` followed by whitespace or the end, or at a newline. A citation written
 * AFTER the full stop ("... doses. [1]"), or alone on the next line, belongs to
 * the sentence before it. A decimal point is not an end ("2.5 ml"), nor are
 * "e.g.", "i.e." or a list marker ("1. Take ...").
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
    const soFar = text.slice(start, i);
    if ('.!?'.includes(ch) && !(ch === '.' && (ABBREVIATION_END.test(soFar) || MARKER_SO_FAR.test(soFar)))) {
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
// own, so it cannot make a reply "grounded".
const hasOwnContent = (sentence) => /[\p{L}\p{N}]/u.test(String(sentence).replace(CITATION_BRACKET, ''));

/**
 * Judge one sentence. `sources` is the lookup's ordered source list; a
 * source's `n` is its 1-based position.
 *
 * @returns {{keep: true, cites: number[]}|{keep: false, reason: string}}
 */
function judgeSentence(sentence, sources, spansOf) {
  const { numbers, malformed } = citationsIn(sentence);
  if (malformed) return { keep: false, reason: WITHHELD_REASONS.CITATION_MALFORMED };
  if (numbers.some((n) => !Number.isInteger(n) || n < 1 || n > sources.length)) {
    return { keep: false, reason: WITHHELD_REASONS.CITATION_OUT_OF_RANGE };
  }
  const cites = [...new Set(numbers)];
  if (cites.some((n) => isOverdoseSection(sources[n - 1] && sources[n - 1].sectionPath))) {
    return { keep: false, reason: WITHHELD_REASONS.OVERDOSE_SECTION };
  }
  const { spans, unclassifiable } = numericScan(normaliseDoseText(sentence));
  if (spans.length || unclassifiable) {
    if (!cites.length) return { keep: false, reason: WITHHELD_REASONS.DOSE_UNCITED };
    // Defensive: the catch-all `number` form leaves no digit unexplained, so
    // this cannot fire today; if a future form change ever let one through,
    // the digit is unverifiable and the sentence is withheld.
    if (unclassifiable) return { keep: false, reason: WITHHELD_REASONS.DOSE_NOT_IN_SOURCE };
    const everySpanSourced = spans.every((span) => cites.some((n) => spansOf(n).has(span)));
    if (!everySpanSourced) return { keep: false, reason: WITHHELD_REASONS.DOSE_NOT_IN_SOURCE };
  }
  return { keep: true, cites };
}

/**
 * The lookup verdict for one finished grounded reply.
 *
 * `sources` is `[{ n, docTitle, sectionPath, text, ... }]` in citation order.
 * A source whose text could not be recovered should carry `text: ''`: every
 * number cited to it then fails, which is the safe direction.
 *
 * The verdict is JSON and its shape is stable (persisted through
 * `attach_guard`, replayed by `replayMessage`):
 *
 *   kind          'lookup' — what tells a lookup verdict from a triage one
 *   rule          'dose-cite-v2'
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
  const cache = new Map();
  const spansOf = (n) => {
    if (!cache.has(n)) cache.set(n, sourceSpans(list[n - 1] && list[n - 1].text));
    return cache.get(n);
  };
  const kept = [];
  const withheld = [];
  const cited = new Set();
  let display = '';
  let lastWasWithheld = false;
  for (const { text, sep } of splitSentences(raw)) {
    const verdict = judgeSentence(text, list, spansOf);
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
