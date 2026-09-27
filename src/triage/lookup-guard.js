// src/triage/lookup-guard.js — the LOOKUP guard: rule `dose-cite-v7`.
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
// v4 (M6 fix round 4) made the rule EXTRACTIVE: a sentence that carries a
// number, or names a dose unit, is shown only when it IS, whole, one sentence
// of a source it cites, after a normalisation that is faithful (spelling,
// typography, digits) and never semantic. v5 (fix round 5, controller ruling)
// makes it EXTRACTIVE AND ORDERED: once a reply shows a dose, every sentence
// with content must be such a quote, the quotes must follow source order, and
// a dose quote is shown only when nothing unverified was said before it.
// v6 (fix round 6, adjudicated) makes the order CONTIGUOUS and PAGE-ORDERED,
// and binds a list item to the lead-in line above it.
// v7 (Phase 1i MA2, founder decision) narrows the SENTENCE list to overdose
// narratives: verbatim maximum-dose sentences ("Do not take more than 8
// tablets in 24 hours.") and route sentences ("Call 111 or go to A&E.") are
// quotable; a section titled Overdose is still withheld whole, and the
// section list no longer applies to sentence text. Nothing else changed.
//
// The rule (`applyLookupGuard`):
//   Per sentence of the reply (`sentencesOf`: the splitter, the list-marker
//   rule, `normaliseDoseText`):
//   1. every `[n]` must name a source in range (1..sources.length); an
//      out-of-range or unparseable citation withholds the sentence;
//   2. a sentence citing a source whose section path matches
//      `OVERDOSE_SECTION_PATTERNS` is withheld, number or not;
//   3. a sentence matching `OVERDOSE_SENTENCE_PATTERNS` is withheld outright;
//   4. a sentence with any character outside the Latin script and the small
//      symbol whitelist left after normalisation is withheld as `unreadable`;
//   5. the sentence is DOSE-BEARING when its normalised text holds an ASCII
//      digit (number words, fractions, uppercase roman numerals and every
//      Unicode digit are ASCII digits by then) or a `DOSE_UNITS` canon as a
//      word. A dose-bearing sentence with no citation is withheld
//      (`dose-uncited`); one whose normalised text EQUALS no ELIGIBLE sentence
//      of a source it cites is withheld (`dose-not-in-source`). Equality, not
//      prefix, not substring: the model quotes a source sentence whole.
//   Over the reply, when any sentence passed 5 as a dose-bearing quote (a dose
//   is shown):
//   6. every other sentence with content must also be such a quote, else it is
//      withheld (`not-a-quote`);
//   7. the kept quotes must be ONE CONTIGUOUS RUN of the page's sentences: the
//      next quote is the same sentence or the sentence right after the last
//      one in the same source, or the first sentence of the chunk that
//      directly follows the last one's chunk on the page (page order is the
//      sources' `chunkId`; when any source lacks one, their `n`). A quote that
//      skips or breaks the order is withheld (`order`);
//   8. a source list item under a lead-in line ending in ":" is quotable only
//      right after that lead-in, or after the item before it in the same run
//      (`lead-in` otherwise): the lead-in and its items are one unit;
//   9. a dose-bearing quote is kept only if every content sentence before it
//      was kept (`context-unverified` otherwise): no unverified lead-in can
//      re-target it.
//   A reply that shows no dose keeps today's behaviour: sentences with no
//   number and no unit are kept as they are.
//  10. a run of withheld sentences becomes one `WITHHELD_BANNER`; if no kept
//      sentence with content of its own carries a valid citation, the reply is
//      `LOOKUP_NO_EVIDENCE_TEXT`.
//
//   Eligible source sentences (`sourceSentences`): the same splitter,
//   list-marker rule and normaliser over the source's text; a sentence is not
//   eligible when it is empty, when its source's section path matches the
//   overdose list, when it or its immediate neighbour (before or after)
//   matches `OVERDOSE_SENTENCE_PATTERNS`, when it is a list item whose lead-in
//   matches that list, or when it holds a square bracket with a digit in it.
//
// Precedence per sentence: citation-malformed, citation-out-of-range,
// overdose-section, overdose-sentence, unreadable, dose-uncited,
// dose-not-in-source; then over the reply: not-a-quote, lead-in, order,
// context-unverified.
import pin from './detectors.pin.js';

export const LOOKUP_RULE = 'dose-cite-v7';

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
  NOT_A_QUOTE: 'not-a-quote',
  LEAD_IN: 'lead-in',
  ORDER: 'order',
  CONTEXT_UNVERIFIED: 'context-unverified',
});

// ── The founder's overdose lists (v7: the sentence list is narratives only) ──

/**
 * Overdose and maximum-dose material the lookup never quotes from. Matched
 * against a source's SECTION PATH (clause 2) only (v7: no longer against
 * sentence text). The list itself is unchanged in v7.
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

/**
 * Overdose NARRATIVE wording at SENTENCE level (v7, founder decision
 * 2026-09-28: verbatim maximum-dose sentences are quotable; only overdose
 * narratives are withheld). A REPLY sentence that matches is withheld
 * outright; a SOURCE sentence that matches, and its immediate neighbours, are
 * never sentences a reply may quote. v7 removed the dose-instruction words
 * ("more than", "maximum", "max") and the route words ("A&E", "accident and
 * emergency", "emergency"): "Do not take more than 8 tablets in 24 hours." and
 * "Call 111 or go to A&E." are quotable when they are verbatim. Matched on
 * normalised (lower-case, ASCII-dash) text.
 */
export const OVERDOSE_SENTENCE_PATTERNS = Object.freeze([
  /\boverdos/i,
  /\btoo much\b/i,
  /\btoo many\b/i,
  /\bfatal\b/i,
  /\blethal\b/i,
  /\b(?:could|can) kill\b/i,
  /\bpoison/i,
  /\btoxicity\b/i,
  /\bliver damage\b/i,
  /\blife[- ]?threatening\b/i,
  /\bharm(?:s|ed|ful)?\b/i,
  /\bdangerous\b/i,
]);

/**
 * Does this sentence carry overdose-narrative wording? v7: the sentence list
 * only. The section list is no longer applied to sentence text, so a
 * sentence naming a "maximum dose" is judged like any other; every narrative
 * word of the section list ("overdos", "too much", "poison", "toxicity") is in
 * the sentence list too.
 */
export function isOverdoseSentence(sentence) {
  const s = String(sentence ?? '');
  return OVERDOSE_SENTENCE_PATTERNS.some((re) => re.test(s));
}

// ── The unit table ──────────────────────────────────────────────────────────
//
// THE ONE TABLE of dose units, frozen. Each row is a DISTINCT canonical unit:
// spellings collapse to their canon and no unit is converted into another.
// The table does two things: it makes "500 milligrams" and "500mg" the same
// text, and a sentence that names a unit with no number at all ("take a
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
const ALL_SPELLINGS = DOSE_UNITS.map((u) => u.spellings).join('|');
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
// one word that is a unit spelling, as written ("tablets", "mg", "Tabs")
const UNIT_SPELLING_WORD = new RegExp(`^(?:${ALL_SPELLINGS})$`, 'i');

// ── Normalisation ───────────────────────────────────────────────────────────
//
// Applied identically to the reply and to the source. FAITHFUL, never
// semantic: it changes how a thing is written, never what is said. "an hour"
// stays "an hour"; "each", "per" and "a" stay words; "to" and "or" stay words.
// The steps, in order (normalise.json pins the whole chain):
//    1. citation brackets ("[1]", "[1, 2]") out, with NOTHING in their place,
//       so "1[1]6 tablets" is 16 tablets here as it is on screen; a footnote
//       mark ("[a]", "[*]") out; paired markdown emphasis (** __ * _ `) out,
//       the text between kept;
//    2. invisible characters out;
//    3. an UPPERCASE roman numeral of two or more letters to digits ("XVI" ->
//       "16"; never a unit spelling such as "ML", never "CM" or "MM");
//    4. a space between a digit and a vulgar fraction ("2½" -> "2 ½");
//       NFKC; every Unicode decimal digit to ASCII; lower case;
//    5. typography to ASCII: the fraction slash, dashes, quotes, "×" to "x",
//       "°" to " degrees ";
//    6. thousands separators out ("1,000" -> "1000"); ".5" to "0.5";
//    7. number words to digits ("five hundred and twenty" -> "520", "one" ->
//       "1", "dozen" -> "12", "twice" -> "2 times", "thrice" -> "3 times");
//    8. "half" and "quarter" forms to one fraction form ("half a", "a half",
//       "½", "1/2 of a" -> "1/2"; "2 and a half" -> "2 1/2");
//    9. unit spellings to their canon ("milligrams" -> "mg"), then a canon
//       right after a number glued to it ("500 mg", "500-mg" -> "500mg");
//   10. whitespace collapsed; leading and trailing punctuation stripped.

const CITATION_MARK = /\[\s*\d+(?:\s*,\s*\d+)*\s*\]/g;
const FOOTNOTE_MARK = /\[\s*(?:[a-z]|\*|†|‡)\s*\]/gi;
const EMPHASIS = [
  [/\*\*(\S(?:[^*]*?\S)?)\*\*/g, '$1'],
  [/__(\S(?:[^_]*?\S)?)__/g, '$1'],
  [/(?<![A-Za-z0-9])\*(\S(?:[^*]*?\S)?)\*(?![A-Za-z0-9])/g, '$1'],
  [/(?<![A-Za-z0-9])_(\S(?:[^_]*?\S)?)_(?![A-Za-z0-9])/g, '$1'],
  [/`([^`]+)`/g, '$1'],
];
const INVISIBLE = /[­͏؜ᅟᅠ឴឵᠎​-‏‪-‮⁠-⁤⁦-⁯︀-️﻿]/g;
const VULGAR_FRACTION = /[¼-¾⅐-⅟↉]/;
const EDGE_PUNCTUATION = /^[\s.,;:!?'"()\-•*#`]+|[\s.,;:!?'"()\-•*#`]+$/g;

// An uppercase roman numeral in strict form, two letters or more, as a word.
const ROMAN = /\b[IVXLCDM]{2,}\b/g;
const STRICT_ROMAN = /^M{0,3}(?:CM|CD|D?C{0,3})(?:XC|XL|L?X{0,3})(?:IX|IV|V?I{0,3})$/i;
const ROMAN_VALUES = { I: 1, V: 5, X: 10, L: 50, C: 100, D: 500, M: 1000 };
const NOT_ROMAN = new Set(['CM', 'MM']);
function romanToDigits(word) {
  if (!STRICT_ROMAN.test(word) || NOT_ROMAN.has(word) || UNIT_SPELLING_WORD.test(word)) return word;
  let total = 0;
  for (let i = 0; i < word.length; i++) {
    const v = ROMAN_VALUES[word[i]];
    const next = ROMAN_VALUES[word[i + 1]] || 0;
    total += v < next ? -v : v;
  }
  return String(total);
}

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
  s = s.replace(CITATION_MARK, '').replace(FOOTNOTE_MARK, '');
  for (const [re, to] of EMPHASIS) s = s.replace(re, to);
  s = s.replace(INVISIBLE, '');
  s = s.replace(ROMAN, romanToDigits);
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

// A lowercase roman numeral of two or more letters in strict form, as its
// own token, makes a sentence dose-bearing (never mapped to digits: "iv" is
// also intravenous). Not "cm", "mm" (lengths) or the word "mix".
const LOWER_ROMAN = /\b[ivxlcdm]{2,}\b/g;
const NOT_LOWER_ROMAN = new Set(['cm', 'mm', 'mix']);
const hasLowerRoman = (s) => [...s.matchAll(LOWER_ROMAN)].some((m) => STRICT_ROMAN.test(m[0]) && !NOT_LOWER_ROMAN.has(m[0]));

/** Does normalised text carry a number, name a dose unit, or hold a lowercase roman numeral? Only such a sentence must be a quote. */
export function isDoseBearing(normalised) {
  const s = String(normalised ?? '');
  return /\d/.test(s) || UNIT_WORD.test(s) || hasLowerRoman(s);
}

// What may remain after normalisation: printable ASCII, letters of the Latin
// script (Latin-1 and Latin Extended: é, ñ, ø ...), a few symbols the
// reference text uses (£ € ≥ ≤ ® ©), and the Greek letters of drug names
// (α β γ δ κ μ) when they stand as a word of their own ("β-blockers"), never
// inside a word with Latin letters. Anything else — a Cyrillic letter, a
// homoglyph inside a word, a combining mark, a stray symbol — makes the
// sentence unreadable.
const READABLE = /^[\x20-\x7E\p{Script=Latin}£€≥≤®©]*$/u;
const GREEK_WORD = /(?<!\p{L})[αβγδκμ]+(?!\p{L})/gu;

/** Is anything left outside the Latin script and the symbol whitelist after normalisation? */
export function isUnreadable(normalised) {
  return !READABLE.test(String(normalised ?? '').replace(GREEK_WORD, ''));
}

// ── Sentences and citations ─────────────────────────────────────────────────

const ABBREVIATION_END = /(?:^|[\s(])(?:e\.g|i\.e|etc|approx|vs|dr|mr|mrs|ms)$/i;
// "No." is an abbreviation only when a number follows it ("pack No. 3")
const NUMBER_ABBREVIATION_END = /(?:^|[\s(])no$/i;
const TRAILING_CITATION = /^[ \t]*\[[^\]\n]*\]/;
// a citation cluster alone on the NEXT line (exactly one newline away)
const CITATION_LINE = /^[ \t]*\n[ \t]*(?:\[[^\]\n]*\][ \t]*)+(?=\n|$)/;
// a list marker at the start of a line ("1." or "- 2)") is not a sentence end
const MARKER_SO_FAR = /^\s*(?:[-•*]\s*)?\d+$/;
// a line ends with terminal punctuation (closing quotes, brackets and
// citations may follow it)
const TERMINAL_LINE_END = /[.!?]["'”’)\]]*(?:[ \t]*\[[^\]\n]*\])*[ \t]*$/;
// a line that starts a new block: blank, a heading, a list item
const BLOCK_START = /^[ \t]*(?:$|#|[-•*][ \t]|\d+[.)][ \t])/;
// a list marker with its bullet: "1. ", "- 2) "
const LIST_MARKER = /^\s*(?:[-•*]\s*)?\d+[.)]\s+/;

/**
 * Split a text into sentences, each with the whitespace that followed it, so
 * the kept text is reassembled exactly as written. A sentence ends at `.`, `!`
 * or `?` followed by whitespace or the end. A line break ends a sentence only
 * when the line ends with terminal punctuation, when the line is a heading,
 * or when the next line is blank or starts a heading or a list item;
 * otherwise the lines are one wrapped sentence. A citation written AFTER the
 * full stop ("... doses. [1]"), or alone on the next line, belongs to the
 * sentence before it. A decimal point is not an end ("2.5 ml"), nor are
 * "e.g.", "i.e.", "No." before a number, or a list marker ("1. Take ...").
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
    const soFar = text.slice(start, i);
    if (ch === '\n') {
      const cite = CITATION_LINE.exec(text.slice(i));
      if (cite) { endAt(i + cite[0].replace(/[ \t]+$/, '').length); continue; }
      const line = soFar.slice(soFar.lastIndexOf('\n') + 1);
      const nextLine = text.slice(i + 1, (text.indexOf('\n', i + 1) + 1 || text.length + 1) - 1);
      if (!soFar.trim() || TERMINAL_LINE_END.test(soFar) || /^[ \t]*#/.test(line) || BLOCK_START.test(nextLine)) {
        endAt(i);
        continue;
      }
      i++;
      continue;
    }
    const abbreviation = ch === '.' && (ABBREVIATION_END.test(soFar) || MARKER_SO_FAR.test(soFar)
      || (NUMBER_ABBREVIATION_END.test(soFar) && /^\s*\d/.test(text.slice(i + 1))));
    if ('.!?'.includes(ch) && !abbreviation) {
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
 * A leading list marker ("1. ", "- 2) ") is not part of its sentence when a
 * word follows it that is not a unit: "2. Take 2 tablets" is "Take 2 tablets".
 * It stays when a unit word follows ("8. tablets at once" is a count) or when
 * no word follows.
 */
export function stripListMarker(text) {
  const s = String(text ?? '');
  const m = LIST_MARKER.exec(s);
  if (!m) return s;
  const rest = s.slice(m[0].length);
  const word = /^\p{L}+/u.exec(rest);
  if (!word || UNIT_SPELLING_WORD.test(word[0])) return s;
  return rest;
}

/**
 * A text's sentences with their normalised form: the splitter, the list-marker
 * rule, then `normaliseDoseText` per sentence. The same function reads the
 * reply and each source.
 *
 * @returns {{text: string, sep: string, normalised: string}[]}
 */
export function sentencesOf(text) {
  return splitSentences(text).map((x) => ({ ...x, normalised: normaliseDoseText(stripListMarker(x.text)) }));
}

const DIGIT_BRACKET = /\[[^\]]*\d[^\]]*\]/;
const LIST_ITEM = /^\s*(?:[-•*]\s|\d+[.)]\s)/;

/**
 * A source's sentences, in order, each marked eligible or not, and each list
 * item bound to its lead-in: a list item whose preceding sentence is a line
 * ending in ":" (or another item bound to that lead-in) carries `leadIn`, the
 * index of that line. Not eligible: an empty sentence; every sentence of a
 * source whose section path matches the overdose list; a sentence with
 * overdose-narrative wording (`OVERDOSE_SENTENCE_PATTERNS`), and the sentence
 * immediately before or after it; a list item whose lead-in has that wording;
 * a sentence holding a square bracket with a digit in it (the pack writes no
 * citations, and a bracketed number is not what the reader would see quoted).
 *
 * @returns {{text: string, normalised: string, eligible: boolean, leadIn: number|null}[]}
 */
export function sourceSentences(sourceText, sectionPath) {
  const all = sentencesOf(sourceText);
  const overdose = all.map((x) => isOverdoseSentence(x.normalised));
  const section = isOverdoseSection(sectionPath);
  const leadIns = [];
  all.forEach((x, i) => {
    const prev = i - 1;
    if (!LIST_ITEM.test(x.text) || prev < 0) leadIns.push(null);
    else if (leadIns[prev] !== null) leadIns.push(leadIns[prev]);
    else leadIns.push(/:$/.test(all[prev].text.replace(/\s*\[[^\]\n]*\]\s*$/, '')) ? prev : null);
  });
  return all.map((x, i) => ({
    text: x.text,
    normalised: x.normalised,
    eligible: Boolean(x.normalised) && !section && !overdose[i] && !overdose[i - 1] && !overdose[i + 1]
      && !(leadIns[i] !== null && overdose[leadIns[i]]) && !DIGIT_BRACKET.test(x.text),
    leadIn: leadIns[i],
  }));
}

/** The normalised sentences of a source a reply may quote whole. */
export function eligibleSourceSentences(sourceText, sectionPath) {
  return sourceSentences(sourceText, sectionPath).filter((x) => x.eligible).map((x) => x.normalised);
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
// own, so it cannot make a reply "grounded" and takes no part in the reply's
// order.
const hasOwnContent = (sentence) => /[\p{L}\p{N}]/u.test(String(sentence).replace(CITATION_BRACKET, ''));

const withhold = (reason) => ({ keep: false, reason });

/**
 * Judge one reply sentence on its own (clauses 1 to 5). A kept sentence
 * carries its citations, whether it is dose-bearing, and every (source n,
 * sentence index) position at which it quotes an eligible sentence of a
 * source it cites.
 */
function judgeSentence({ text, normalised }, { sources, sentencesOfSource }) {
  const { numbers, malformed } = citationsIn(text);
  if (malformed) return withhold(WITHHELD_REASONS.CITATION_MALFORMED);
  if (numbers.some((n) => !Number.isInteger(n) || n < 1 || n > sources.length)) {
    return withhold(WITHHELD_REASONS.CITATION_OUT_OF_RANGE);
  }
  const cites = [...new Set(numbers)];
  if (cites.some((n) => isOverdoseSection(sources[n - 1] && sources[n - 1].sectionPath))) {
    return withhold(WITHHELD_REASONS.OVERDOSE_SECTION);
  }
  if (isOverdoseSentence(normalised)) return withhold(WITHHELD_REASONS.OVERDOSE_SENTENCE);
  if (isUnreadable(normalised)) return withhold(WITHHELD_REASONS.UNREADABLE);
  const dose = isDoseBearing(normalised);
  if (dose && !cites.length) return withhold(WITHHELD_REASONS.DOSE_UNCITED);
  const positions = cites.flatMap((n) => sentencesOfSource(n)
    .map((s, i) => (s.eligible && s.normalised === normalised ? [n, i] : null)).filter(Boolean));
  if (dose && !positions.length) return withhold(WITHHELD_REASONS.DOSE_NOT_IN_SOURCE);
  return { keep: true, cites, dose, positions };
}

/**
 * Is the quote at `p` the next sentence of the page after the quote at `l`?
 * Positions are [source n, sentence index]; `page` gives each source its
 * page-order key and sentence count.
 */
function contiguous(l, p, page) {
  if (l[0] === p[0]) return p[1] === l[1] || p[1] === l[1] + 1;
  return page(p[0]).key === page(l[0]).key + 1 && l[1] === page(l[0]).length - 1 && p[1] === 0;
}

/**
 * The lookup verdict for one finished grounded reply.
 *
 * `sources` is `[{ n, chunkId, docTitle, sectionPath, text, ... }]` in
 * citation order. Page order is read from `chunkId` (the chunk's ordinal in
 * the pack); when any source lacks one, `n` stands in. A source whose text
 * could not be recovered should carry `text: ''`: every number cited to it
 * then fails, which is the safe direction.
 *
 * The verdict is JSON and its shape is stable (persisted through
 * `attach_guard`, replayed by `replayMessage`):
 *
 *   kind          'lookup' — what tells a lookup verdict from a triage one
 *   rule          'dose-cite-v7'
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
  const sentencesOfSource = (n) => {
    if (!cache.has(n)) cache.set(n, sourceSentences(list[n - 1] && list[n - 1].text, list[n - 1] && list[n - 1].sectionPath));
    return cache.get(n);
  };
  const ctx = { sources: list, sentencesOfSource };
  const byChunk = list.every((src) => src && Number.isFinite(src.chunkId));
  const page = (n) => ({ key: byChunk ? list[n - 1].chunkId : n, length: sentencesOfSource(n).length });
  const bound = (p) => sentencesOfSource(p[0])[p[1]].leadIn !== null;
  const judged = sentencesOf(raw).map((s) => ({ ...s, content: hasOwnContent(s.text), verdict: judgeSentence(s, ctx) }));
  // A dose is shown: the reply must be extractive and ordered (clauses 6 to 9).
  const extractive = judged.some((j) => j.verdict.keep && j.verdict.dose);
  const kept = [];
  const withheld = [];
  const cited = new Set();
  let display = '';
  let lastWasWithheld = false;
  // every position the run of kept quotes so far may be at (a sentence can
  // occur more than once); null before the first quote
  let lasts = null;
  let contextOk = true;
  for (const { text, sep, content, verdict } of judged) {
    let v = verdict;
    if (v.keep && content && extractive) {
      const next = v.positions.filter((p) => (lasts === null || lasts.some((l) => contiguous(l, p, page)))
        && (!bound(p) || (lasts !== null && lasts.some((l) => l[0] === p[0] && (l[1] === p[1] - 1 || l[1] === p[1])))));
      if (!v.positions.length) v = withhold(WITHHELD_REASONS.NOT_A_QUOTE);
      else if (!next.length) v = withhold(v.positions.every(bound) ? WITHHELD_REASONS.LEAD_IN : WITHHELD_REASONS.ORDER);
      else if (v.dose && !contextOk) v = withhold(WITHHELD_REASONS.CONTEXT_UNVERIFIED);
      else lasts = next;
    }
    if (v.keep) {
      kept.push(text);
      if (content) v.cites.forEach((n) => cited.add(n));
      display += text + sep;
      lastWasWithheld = false;
    } else {
      if (content) contextOk = false;
      withheld.push({ sentence: text, reason: v.reason });
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
