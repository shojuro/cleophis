// src/triage/lookup-guard.js — the LOOKUP guard: rule `dose-cite-v4`.
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
// v4 (M6 fix round 4, controller ruling): EXTRACTIVE EQUALITY. Three rounds of
// enumerating dose grammars (v1..v3) each lost to recombination, form changes
// and cross-source numbers, so the rule no longer parses doses at all. A reply
// sentence that carries a number (or names a dose unit) is shown only when it
// IS, whole, one sentence of a source it cites — after a normalisation that is
// faithful (spelling, typography, digits) and never semantic.
//
// The rule, per sentence of the reply (`splitSentences`):
//   1. every `[n]` must name a source in range (1..sources.length). An
//      out-of-range or unparseable citation withholds the sentence;
//   2. a sentence citing a source whose section path matches
//      `OVERDOSE_SECTION_PATTERNS` is withheld, number or not;
//   3. a sentence matching `OVERDOSE_SENTENCE_PATTERNS` is withheld outright;
//   4. the sentence is normalised (`normaliseDoseText`). It is DOSE-BEARING
//      when the normalised text holds an ASCII digit (number words, fractions
//      and every Unicode digit are ASCII digits by then) or a `DOSE_UNITS`
//      canon as a word. A sentence that is not dose-bearing is kept: it
//      states no dose;
//   5. a dose-bearing sentence with any non-ASCII character left after
//      normalisation is withheld as `unreadable` (homoglyphs fail closed);
//   6. a dose-bearing sentence with no citation is withheld (`dose-uncited`);
//   7. a dose-bearing sentence is kept only if its normalised text is EXACTLY
//      EQUAL to the normalised text of one ELIGIBLE sentence of one source it
//      cites (`eligibleSourceSentences`): the same splitter over the source's
//      text, minus sentences that match `OVERDOSE_SENTENCE_PATTERNS` and minus
//      sentences holding a square bracket. Not a prefix, not a substring, not
//      a span: the model quotes a source sentence whole, or nothing;
//   8. a run of withheld sentences becomes one `WITHHELD_BANNER`;
//   9. if no kept sentence with content of its own carries a valid citation,
//      the reply is `LOOKUP_NO_EVIDENCE_TEXT`.
//
// Precedence: citation-malformed, citation-out-of-range, overdose-section,
// overdose-sentence, unreadable, dose-uncited, dose-not-in-source.
import pin from './detectors.pin.js';

export const LOOKUP_RULE = 'dose-cite-v4';

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
  OVERDOSE_SENTENCE: 'overdose-sentence',
  UNREADABLE: 'unreadable',
  DOSE_UNCITED: 'dose-uncited',
  DOSE_NOT_IN_SOURCE: 'dose-not-in-source',
});

// ── The founder's overdose lists (kept as registered; their cost is listed) ──

/**
 * Overdose and maximum-dose material the lookup never quotes from. Matched
 * against a source's SECTION PATH (clause 2) and, through
 * `isOverdoseSentence`, against sentence text.
 */
export const OVERDOSE_SECTION_PATTERNS = Object.freeze([
  /\boverdos/i,
  /\bmax(?:imum|\.)?\s+(?:daily\s+|single\s+|total\s+)?doses?\b/i,
  /\bhow much is too much\b/i,
  /\btoo much\b/i,
  /\bpoison/i,
  /\btoxicity\b/i,
]);

/** Does this section path (or sentence) name overdose or maximum-dose material? */
export function isOverdoseSection(sectionPath) {
  const s = String(sectionPath ?? '');
  return OVERDOSE_SECTION_PATTERNS.some((re) => re.test(s));
}

/**
 * Overdose, maximum-dose and harm wording at SENTENCE level (registered in fix
 * round 3). A REPLY sentence that matches is withheld outright; a SOURCE
 * sentence that matches is never the sentence a reply may quote. Deliberately
 * broad: it also withholds benign "do not take more than" advice, which is the
 * founder's registered cost.
 */
export const OVERDOSE_SENTENCE_PATTERNS = Object.freeze([
  /\bmore than\b/i,
  /\bmaximum\b|\bmax\b/i,
  /\bliver damage\b/i,
  /\ba ?& ?e\b|\baccident and emergency\b/i,
  /\bemergency\b/i,
  /\bfatal\b/i,
  /\bharm(?:s|ed|ful)?\b/i,
  /\bdangerous\b/i,
  /\boverdos/i,
  /\btoo much\b/i,
  /\btoo many\b/i,
]);

/** Does this sentence carry overdose, maximum-dose or harm wording? */
export function isOverdoseSentence(sentence) {
  const s = String(sentence ?? '');
  return OVERDOSE_SENTENCE_PATTERNS.some((re) => re.test(s)) || isOverdoseSection(s);
}

// ── The unit table ──────────────────────────────────────────────────────────
//
// THE ONE TABLE of dose units, frozen. Each row is a DISTINCT canonical unit:
// spellings collapse to their canon and no unit is converted into another.
// In v4 the table does two things: it makes "500 milligrams" and "500mg" the
// same text, and a sentence that names a unit with no number at all ("take a
// tablet", "take another dose") is dose-bearing and must be a quote.
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

const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const CANON_ALT = DOSE_UNITS.map((u) => esc(u.canon)).join('|');
// a spelling is a whole word among letters: "mg" in "500mg" and "500 mg/kg",
// never the "g" of "kg" or the "ug" of "drug"
const UNIT_SPELLINGS = DOSE_UNITS.map((u) => [u.canon, new RegExp(`(?<![a-z])(?:${u.spellings})(?![a-z])`, 'g')]);
// a canon right after a number, across spaces or a hyphen, is glued to it:
// "500 mg", "500-mg" and "500mg" are one text
const UNIT_AFTER_NUMBER = new RegExp(`(\\d)[ \\t]*-?[ \\t]*(${CANON_ALT})(?![a-z])`, 'g');
// a unit named anywhere, with or without a number: "take a tablets". Not a
// canon touching a dot: the "g" of "e.g." is no unit
const UNIT_WORD = new RegExp(`(?<![a-z0-9.])(?:${CANON_ALT})(?![a-z0-9.])`);

// ── Normalisation ───────────────────────────────────────────────────────────
//
// Applied identically to the reply and to the source. FAITHFUL, never
// semantic: it changes how a thing is written, never what is said. "an hour"
// stays "an hour"; "each", "per" and "a" stay words; "to" and "or" stay words.
// The steps, in order (normalise.json pins the whole chain):
//    1. citation brackets ("[1]", "[1, 2]") out, with NOTHING in their place,
//       so "1[1]6 tablets" is 16 tablets here as it is on screen;
//    2. invisible characters out;
//    3. a space between a digit and a vulgar fraction ("2½" -> "2 ½");
//       NFKC; every Unicode decimal digit to ASCII; lower case;
//    4. typography to ASCII: the fraction slash, dashes, quotes, "×" to "x",
//       "°" to " degrees ";
//    5. thousands separators out ("1,000" -> "1000"); ".5" to "0.5";
//    6. number words to digits ("five hundred and twenty" -> "520", "one" ->
//       "1", "dozen" -> "12", "twice" -> "2 times", "thrice" -> "3 times");
//    7. "half" and "quarter" forms to one fraction form ("half a", "a half",
//       "½", "1/2 of a" -> "1/2"; "2 and a half" -> "2 1/2");
//    8. unit spellings to their canon ("milligrams" -> "mg"), then a canon
//       right after a number glued to it ("500 mg", "500-mg" -> "500mg");
//    9. whitespace collapsed; leading and trailing punctuation stripped.

const CITATION_MARK = /\[\s*\d+(?:\s*,\s*\d+)*\s*\]/g;
const INVISIBLE = /[­͏؜ᅟᅠ឴឵᠎​-‏‪-‮⁠-⁤⁦-⁯︀-️﻿]/g;
const VULGAR_FRACTION = /[¼-¾⅐-⅟↉]/;
const EDGE_PUNCTUATION = /^[\s.,;:!?'"()\-•]+|[\s.,;:!?'"()\-•]+$/g;

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

/**
 * The faithful normalisation both sides of the check go through. Exported so
 * the fixtures can pin it and the triage port can match it.
 */
export function normaliseDoseText(text) {
  let s = String(text ?? '');
  s = s.replace(CITATION_MARK, '');
  s = s.replace(INVISIBLE, '');
  s = s.replace(new RegExp(`(\\d)(?=${VULGAR_FRACTION.source})`, 'g'), '$1 ');
  s = asciiDigits(s.normalize('NFKC')).toLowerCase();
  s = s.replace(/⁄/g, '/').replace(/[‐‑‒–—−]/g, '-');
  s = s.replace(/[‘’‚‛]/g, "'").replace(/[“”„‟]/g, '"');
  s = s.replace(/×/g, 'x').replace(/°/g, ' degrees ');
  for (let prev = null; prev !== s;) { prev = s; s = s.replace(/(\d),(\d{3})(?!\d)/g, '$1$2'); }
  s = s.replace(/(^|[^\d])\.(\d)/g, '$10.$2');
  s = s.replace(NUMBER_RUN, numberRunToDigits);
  s = s.replace(/\bdozen\b/g, '12').replace(/\btwice\b/g, '2 times').replace(/\bthrice\b/g, '3 times');
  s = s.replace(/\b(\d+)[ \t]+and[ \t]+(?:an?[ \t]+|1[ \t]+)?half\b/g, '$1 1/2');
  s = s.replace(/\b(\d+)[ \t]+and[ \t]+(?:an?[ \t]+|1[ \t]+)?quarter\b/g, '$1 1/4');
  s = s.replace(/\b(\d+)[ \t]+quarters\b/g, '$1/4');
  s = s.replace(/\b(?:an?[ \t]+|1[ \t]+)?half\b(?:[ \t]+of\b)?(?:[ \t]+an?\b)?/g, '1/2');
  s = s.replace(/\b(?:an?[ \t]+|1[ \t]+)?quarter\b(?:[ \t]+of\b)?(?:[ \t]+an?\b)?/g, '1/4');
  s = s.replace(/\b(\d+(?: \d+)?\/\d+)[ \t]+of[ \t]+(?:an?|the)\b/g, '$1');
  for (const [canon, re] of UNIT_SPELLINGS) s = s.replace(re, canon);
  s = s.replace(UNIT_AFTER_NUMBER, '$1$2');
  return s.replace(/\s+/g, ' ').replace(EDGE_PUNCTUATION, '');
}

/** Does normalised text carry a number, or name a dose unit? Only such a sentence is checked. */
export function isDoseBearing(normalised) {
  const s = String(normalised ?? '');
  return /\d/.test(s) || UNIT_WORD.test(s);
}

/** Is anything left outside ASCII after normalisation (a homoglyph, a stray symbol)? */
export function isUnreadable(normalised) {
  return /[^\x00-\x7F]/.test(String(normalised ?? ''));
}

// ── Sentences and citations ─────────────────────────────────────────────────

const ABBREVIATION_END = /(?:^|[\s(])(?:e\.g|i\.e|approx|vs)$/i;
const TRAILING_CITATION = /^[ \t]*\[[^\]\n]*\]/;
// a citation cluster alone on the NEXT line (exactly one newline away)
const CITATION_LINE = /^[ \t]*\n[ \t]*(?:\[[^\]\n]*\][ \t]*)+(?=\n|$)/;
// a list marker at the start of a line ("1." or "- 2)") is not a sentence end
const MARKER_SO_FAR = /^\s*(?:[-•]\s*)?\d+$/;
const LIST_MARKER = /^\s*(?:[-•]\s*)?(\d+)[.)](?=\s)/;

/**
 * Split a text into sentences, each with the whitespace that followed it, so
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

/**
 * Are a text's leading list markers a real list — 1, 2, 3, ... in order?
 * Only then is a marker not part of its sentence. "16. Take this many
 * tablets." alone is a count, not a list item.
 */
export function listMarkersAreASequence(sentences) {
  const markers = sentences.map((t) => LIST_MARKER.exec(t)).filter(Boolean).map((m) => Number(m[1]));
  return markers.length > 0 && markers.every((n, i) => n === i + 1);
}

/**
 * A text's sentences with their normalised form: the splitter, the list-marker
 * rule over the whole text, then `normaliseDoseText` per sentence. The same
 * function reads the reply and each source.
 *
 * @returns {{text: string, sep: string, normalised: string}[]}
 */
export function sentencesOf(text) {
  const split = splitSentences(text);
  const list = listMarkersAreASequence(split.map((x) => x.text));
  return split.map((x) => ({ ...x, normalised: normaliseDoseText(list ? x.text.replace(LIST_MARKER, '') : x.text) }));
}

/**
 * The normalised sentences of a source a reply may quote whole. Not eligible:
 * an empty sentence, a sentence with overdose, maximum-dose or harm wording,
 * and a sentence holding a square bracket (the pack writes no citations, and a
 * bracketed aside is not part of what the reader would see quoted).
 *
 * @returns {string[]}
 */
export function eligibleSourceSentences(sourceText) {
  return sentencesOf(sourceText)
    .filter((x) => x.normalised && !isOverdoseSentence(x.normalised) && !/[[\]]/.test(x.text))
    .map((x) => x.normalised);
}

const CITATION_LIST = /^\s*\d+(?:\s*,\s*\d+)*\s*$/;
const CITATION_BRACKET = /\[([^\]\n]*)\]/g;

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
 * Judge one reply sentence (clauses 1 to 7).
 *
 * @returns {{keep: true, cites: number[]}|{keep: false, reason: string}}
 */
function judgeSentence({ text, normalised }, { sources, eligibleOf }) {
  const { numbers, malformed } = citationsIn(text);
  if (malformed) return { keep: false, reason: WITHHELD_REASONS.CITATION_MALFORMED };
  if (numbers.some((n) => !Number.isInteger(n) || n < 1 || n > sources.length)) {
    return { keep: false, reason: WITHHELD_REASONS.CITATION_OUT_OF_RANGE };
  }
  const cites = [...new Set(numbers)];
  if (cites.some((n) => isOverdoseSection(sources[n - 1] && sources[n - 1].sectionPath))) {
    return { keep: false, reason: WITHHELD_REASONS.OVERDOSE_SECTION };
  }
  if (isOverdoseSentence(normalised)) return { keep: false, reason: WITHHELD_REASONS.OVERDOSE_SENTENCE };
  if (!isDoseBearing(normalised)) return { keep: true, cites };
  if (isUnreadable(normalised)) return { keep: false, reason: WITHHELD_REASONS.UNREADABLE };
  if (!cites.length) return { keep: false, reason: WITHHELD_REASONS.DOSE_UNCITED };
  const quoted = cites.some((n) => eligibleOf(n).includes(normalised));
  return quoted ? { keep: true, cites } : { keep: false, reason: WITHHELD_REASONS.DOSE_NOT_IN_SOURCE };
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
 *   rule          'dose-cite-v4'
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
  const eligibleOf = (n) => {
    if (!cache.has(n)) cache.set(n, eligibleSourceSentences(list[n - 1] && list[n - 1].text));
    return cache.get(n);
  };
  const ctx = { sources: list, eligibleOf };
  const kept = [];
  const withheld = [];
  const cited = new Set();
  let display = '';
  let lastWasWithheld = false;
  for (const sentence of sentencesOf(raw)) {
    const { text, sep } = sentence;
    const verdict = judgeSentence(sentence, ctx);
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
