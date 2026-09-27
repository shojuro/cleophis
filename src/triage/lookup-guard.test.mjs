// src/triage/lookup-guard.test.mjs — node --test src/
//
// The dose-cite-v7 rule, driven ENTIRELY by the JSON fixtures in
// fixtures/lookup-guard/ — the files the triage repo's probes/dose-cite.mjs
// vendors and asserts identity against. Nothing about the rule's behaviour is
// pinned only here: every vector a second implementation must reproduce is in
// a fixture file, and manifest.json pins each file's bytes and case count.
import { test } from 'node:test';
import assert from 'node:assert';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import {
  DOSE_UNITS, LOOKUP_NO_EVIDENCE_TEXT, LOOKUP_RULE, OVERDOSE_SECTION_PATTERNS, OVERDOSE_SENTENCE_PATTERNS,
  WITHHELD_BANNER, WITHHELD_REASONS, applyLookupGuard, citationsIn, eligibleSourceSentences, isDoseBearing,
  isOverdoseSection, isOverdoseSentence, isRouteBearing, isUnreadable, normaliseDoseText, sentencesOf, sourceSentences, splitSentences,
  stripListMarker,
} from './lookup-guard.js';
import pin from './detectors.pin.js';

const DIR = new URL('./fixtures/lookup-guard/', import.meta.url);
const bytes = (name) => readFileSync(new URL(name, DIR));
const fixture = (name) => JSON.parse(bytes(name).toString('utf8'));

const SOURCES = fixture('sources.json');
const { groups: GROUPS } = fixture('verdicts.json');
const sourcesOf = (c) => (Array.isArray(c.sources) ? c.sources : SOURCES[c.sources]);
const placeholders = (s) => s.replaceAll('WITHHELD', WITHHELD_BANNER).replace(/^NO_EVIDENCE$/, LOOKUP_NO_EVIDENCE_TEXT);
const PROBE_GROUPS = ['probes-round-1', 'probes-round-2', 'probes-round-3', 'probes-round-4', 'probes-round-5'];

/* ---------------- the fixture set itself ---------------- */

// Regenerate after a reviewed fixture change by recomputing each file's sha256
// and count (the count rule is `countOf` below).
test('manifest.json pins every fixture file by sha256 and case count', () => {
  const manifest = fixture('manifest.json');
  assert.strictEqual(manifest.rule, LOOKUP_RULE);
  const countOf = (doc) => (doc.groups ? Object.values(doc.groups).reduce((n, g) => n + g.length, 0)
    : (doc.cases || doc.units || []).length || Object.keys(doc).filter((k) => k !== '_comment').length);
  for (const [name, want] of Object.entries(manifest.files)) {
    const b = bytes(name);
    assert.strictEqual(createHash('sha256').update(b).digest('hex'), want.sha256, `${name} bytes changed: update manifest.json`);
    assert.strictEqual(countOf(JSON.parse(b.toString('utf8'))), want.count, `${name} case count`);
  }
  assert.deepStrictEqual(Object.keys(manifest.files).sort(), [
    'citations.json', 'eligible.json', 'normalise.json', 'overdose-sections.json', 'overdose-sentences.json',
    'sentences.json', 'sources.json', 'units.json', 'verdicts.json',
  ]);
});

test('the verdict fixtures have one named group per clause plus the probe sets, and every reason is exercised', () => {
  assert.deepStrictEqual(Object.keys(GROUPS), [
    'citation-range', 'overdose-section', 'overdose-sentence', 'maximum-and-route-quotable', 'script-check', 'dose-bearing', 'dose-uncited', 'route-bearing',
    'extractive-equality', 'extractive-context', 'ordered', 'lead-in-binding', 'source-splitting', 'neighbours',
    'list-markers-and-brackets', 'withholding-and-banner', 'no-evidence-fallback', ...PROBE_GROUPS, 'cost',
  ]);
  for (const [name, cases] of Object.entries(GROUPS)) assert.ok(cases.length >= 3, `${name} has too few cases`);
  const reasons = new Set(Object.values(GROUPS).flat().flatMap((c) => c.expected.withheld.map((w) => w.reason)));
  for (const r of Object.values(WITHHELD_REASONS)) assert.ok(reasons.has(r), `no fixture withholds for ${r}`);
  const names = Object.values(GROUPS).flat().map((c) => c.name);
  assert.strictEqual(new Set(names).size, names.length, 'case names are distinct');
});

/* ---------------- the rule, clause by clause ---------------- */

for (const [group, cases] of Object.entries(GROUPS)) {
  for (const c of cases) {
    test(`dose-cite-v7 [${group}]: ${c.name}`, () => {
      const sources = sourcesOf(c);
      assert.ok(sources, `unknown source set ${c.sources}`);
      const v = applyLookupGuard({ replyText: c.replyText, sources });
      assert.strictEqual(v.outcome, c.expected.outcome);
      assert.strictEqual(v.displayText, placeholders(c.expected.displayText));
      assert.deepStrictEqual(v.kept, c.expected.kept);
      assert.deepStrictEqual(v.withheld, c.expected.withheld);
      assert.deepStrictEqual(v.citations, c.expected.citations);
    });
  }
}

// The five re-reviews' adversarial pairs: every "probe" case is withheld,
// except the ones the report names, which carry a note saying KEPT and why.
// The "quote" cases of rounds 4 and 5 are the verbatim quotes the previous
// rule wrongly withheld: every one is kept.
test('every prior adversarial pair is withheld unless its fixture notes why it is kept; every named quote is kept', () => {
  for (const group of PROBE_GROUPS) {
    for (const c of GROUPS[group]) {
      if (/^quote /.test(c.name)) assert.deepStrictEqual(c.expected.withheld, [], `${group}: ${c.name} is not kept`);
      else if (/^probe /.test(c.name) && !c.expected.withheld.length) {
        assert.ok(c.note && /KEPT/.test(c.note), `${group}: ${c.name} is kept without a note`);
      }
    }
  }
});

/* ---------------- the parts, from their vector files ---------------- */

test('units.json is the DOSE_UNITS table, row for row', () => {
  const { units } = fixture('units.json');
  assert.deepStrictEqual(units, DOSE_UNITS.map((u) => ({ canon: u.canon, kind: u.kind, spellings: u.spellings, examples: [...u.examples] })));
  assert.ok(Object.isFrozen(DOSE_UNITS) && DOSE_UNITS.every(Object.isFrozen));
  assert.strictEqual(new Set(DOSE_UNITS.map((u) => u.canon)).size, DOSE_UNITS.length, 'each canon is distinct');
});

for (const u of DOSE_UNITS) {
  test(`unit table: every listed spelling of ${u.canon} normalises to 3${u.canon}, and the bare word is dose-bearing`, () => {
    for (const spelling of u.examples) {
      for (const written of [`3 ${spelling}`, `3${spelling}`, `3-${spelling}`]) {
        assert.strictEqual(normaliseDoseText(`Take ${written} now.`), `take 3${u.canon} now`, written);
      }
      const bare = normaliseDoseText(`Take some ${spelling} now.`);
      assert.strictEqual(bare, `take some ${u.canon} now`, spelling);
      assert.ok(isDoseBearing(bare), `${spelling} alone is dose-bearing`);
      if (!/\s/.test(spelling)) assert.strictEqual(stripListMarker(`8. ${spelling} at once`), `8. ${spelling} at once`, `a marker before ${spelling} stays`);
    }
  });
}

test('normalise.json: normaliseDoseText', () => {
  for (const c of fixture('normalise.json').cases) assert.strictEqual(normaliseDoseText(c.input), c.normalised, c.input);
  assert.notStrictEqual(normaliseDoseText('1 g'), normaliseDoseText('1000 mg'), 'no conversion across units');
  assert.notStrictEqual(normaliseDoseText('an hour'), normaliseDoseText('1 hour'), 'an hour stays words');
  assert.notStrictEqual(normaliseDoseText('a tablet'), normaliseDoseText('1 tablet'), 'a stays a word');
  assert.notStrictEqual(normaliseDoseText('1 to 2'), normaliseDoseText('1-2'), 'to stays a word');
});

test('sentences.json: splitSentences', () => {
  for (const c of fixture('sentences.json').cases) assert.deepStrictEqual(splitSentences(c.input), c.sentences, c.input);
});

test('eligible.json: sourceSentences, with eligibility and lead-in binding', () => {
  for (const c of fixture('eligible.json').cases) {
    assert.deepStrictEqual(sourceSentences(c.source).map((x) => [x.normalised, x.eligible, x.leadIn]), c.sentences, c.source);
    assert.deepStrictEqual(eligibleSourceSentences(c.source), c.sentences.filter((x) => x[1]).map((x) => x[0]), c.source);
  }
  assert.deepStrictEqual(eligibleSourceSentences(undefined), []);
  assert.deepStrictEqual(eligibleSourceSentences('Take 2 tablets.', 'If you take too much'), [], 'an overdose section path makes nothing eligible');
  assert.deepStrictEqual(sourceSentences('Take 2 tablets. Taking too many can be fatal.').map((s) => s.eligible), [false, false]);
  // v7: a maximum-dose sentence is not overdose wording, so it and its neighbour are eligible
  assert.deepStrictEqual(sourceSentences('Take 2 tablets. Do not take more than 8.').map((s) => s.eligible), [true, true]);
});

test('overdose-sentences.json: the registered sentence list is frozen and matches what it names', () => {
  assert.ok(Object.isFrozen(OVERDOSE_SENTENCE_PATTERNS));
  for (const c of fixture('overdose-sentences.json').cases) {
    assert.strictEqual(isOverdoseSentence(c.sentence), c.overdose, c.sentence);
    assert.strictEqual(isOverdoseSentence(normaliseDoseText(c.sentence)), c.overdose, `normalised: ${c.sentence}`);
  }
});

// v7 (founder decision 2026-09-28): the sentence list names overdose
// NARRATIVES only. No dose-instruction word and no route word may match it,
// and the section list (which names maximum-dose sections) no longer reads
// sentence text.
test('v7: no dose-instruction or route word is on the sentence list; the section list reads section paths only', () => {
  for (const w of ['more than', 'no more than', 'maximum', 'max', 'up to', 'do not exceed', 'at most', 'the maximum dose is',
    'a&e', 'accident and emergency', '999', '111', 'emergency', 'go to hospital', 'call']) {
    assert.strictEqual(isOverdoseSentence(`take 2 tablets, ${w} 8`), false, w);
  }
  for (const w of ['overdose', 'overdosing', 'too many', 'too much', 'fatal', 'lethal', 'could kill', 'can kill', 'poison',
    'poisoning', 'liver damage', 'life-threatening',
    // fix round 1: inflections and near-synonyms of the list's own words
    'overdosed', 'overdoses', 'taken too many', 'took too much', 'fatally', 'fatality', 'kill', 'kills', 'killed', 'will kill',
    'poisonous', 'toxic', 'liver failure', 'damage your liver', 'damage to the liver', 'harm your liver', 'threat to life',
    'stop your heart', 'stop breathing']) {
    assert.strictEqual(isOverdoseSentence(`it says ${w} here`), true, w);
  }
  assert.strictEqual(isOverdoseSentence('paracetamol is a painkiller'), false, 'painkiller is not kill');
  assert.strictEqual(isOverdoseSection('Paracetamol > Maximum dose'), true, 'a maximum-dose SECTION is still withheld whole');
  assert.strictEqual(isOverdoseSentence('the maximum dose is 8tablets in 24 hours'), false);
  // fix round 1: the pack's other overdose headings
  for (const path of ['X > If you use too many', 'X > Accidentally taking an extra dose', 'X > Missed or extra doses > Warfarin',
    'X > If you take more than the prescribed dose', 'X > What if I take too many?', 'X > What if I take extra tablets?']) {
    assert.strictEqual(isOverdoseSection(path), true, path);
  }
});

// v7 fix round 1: a sentence the vendored r3 detectors read as a care
// direction is route-bearing and must be a quote, like a dose.
test('v7: isRouteBearing reads routes, negated and de-escalating directions, and nothing else', () => {
  for (const s of ['Go to A&E straight away.', 'This is not an emergency, there is no need to go to A&E.', 'Do not go to A&E, wait and see.',
    'There is no need to call an ambulance.', 'Taking the whole pack is not an emergency.', 'See a GP.', 'Speak to a pharmacist if you are unsure.']) {
    assert.strictEqual(isRouteBearing(s), true, s);
  }
  for (const s of ['Paracetamol is a painkiller [1].', 'Take it with food.', 'Check the label.']) assert.strictEqual(isRouteBearing(s), false, s);
});

test('overdose-sections.json: the registered list is frozen and matches what it names', () => {
  assert.ok(Object.isFrozen(OVERDOSE_SECTION_PATTERNS));
  for (const c of fixture('overdose-sections.json').cases) assert.strictEqual(isOverdoseSection(c.path), c.overdose, c.path);
  assert.strictEqual(isOverdoseSection(undefined), false);
});

test('citations.json: citationsIn', () => {
  for (const c of fixture('citations.json').cases) {
    assert.deepStrictEqual(citationsIn(c.input), { numbers: c.numbers, malformed: c.malformed }, c.input);
  }
});

// The v6/v7 invariant, checked over every verdict fixture rather than case by
// case: a kept sentence carries no overdose wording and is readable. When a
// reply shows a dose, every kept sentence with content is a whole eligible
// sentence of a source it cites, the kept quotes form one contiguous run of
// the page (same source: the same or the next sentence; across sources: the
// last sentence of a chunk then the first of the chunk right after it in page
// order), a list item bound to a lead-in follows that lead-in or the item
// before it, and every content sentence before a kept dose sentence was
// itself kept.
const CONTENT = /[\p{L}\p{N}]/u;
test('INVARIANT: kept text is readable, quoted whole, contiguous in page order, lead-in bound, and preceded only by kept text when a dose is shown', () => {
  for (const c of Object.values(GROUPS).flat()) {
    const sources = sourcesOf(c);
    const kept = new Set(applyLookupGuard({ replyText: c.replyText, sources }).kept);
    const sentences = sentencesOf(c.replyText).map((s) => ({ ...s, content: CONTENT.test(s.text.replace(/\[[^\]\n]*\]/g, '')), kept: kept.has(s.text) }));
    for (const s of sentences.filter((x) => x.kept)) {
      assert.ok(!isOverdoseSentence(s.normalised), `${c.name}: overdose wording kept: ${s.text}`);
      assert.ok(!isUnreadable(s.normalised), `${c.name}: unreadable kept: ${s.text}`);
      if (isRouteBearing(s.text) && !isDoseBearing(s.normalised)) assert.ok(citationsIn(s.text).numbers.length, `${c.name}: uncited route kept: ${s.text}`);
    }
    const mustQuote = (s) => isDoseBearing(s.normalised) || isRouteBearing(s.text);
    const doseShown = sentences.some((s) => s.kept && mustQuote(s));
    if (!doseShown) continue;
    const byChunk = sources.every((src) => Number.isFinite(src.chunkId));
    const page = (n) => ({ key: byChunk ? sources[n - 1].chunkId : n, sentences: sourceSentences(sources[n - 1].text, sources[n - 1].sectionPath) });
    const contiguous = (l, p) => (l[0] === p[0] ? p[1] === l[1] || p[1] === l[1] + 1
      : page(p[0]).key === page(l[0]).key + 1 && l[1] === page(l[0]).sentences.length - 1 && p[1] === 0);
    let lasts = null;
    let allBeforeKept = true;
    for (const s of sentences) {
      if (!s.content) continue;
      if (s.kept) {
        const positions = citationsIn(s.text).numbers.flatMap((n) => page(n).sentences
          .map((src, i) => (src.eligible && src.normalised === s.normalised ? [n, i] : null)).filter(Boolean))
          .filter((p) => lasts === null || lasts.some((l) => contiguous(l, p)))
          .filter((p) => page(p[0]).sentences[p[1]].leadIn === null || (lasts !== null && lasts.some((l) => l[0] === p[0] && p[1] - l[1] <= 1)));
        assert.ok(positions.length, `${c.name}: kept content is not a contiguous, lead-in bound whole quote: ${s.text}`);
        if (mustQuote(s)) assert.ok(allBeforeKept, `${c.name}: a dose or route quote follows withheld content: ${s.text}`);
        lasts = positions;
      } else allBeforeKept = false;
    }
  }
});

/* ---------------- shape ---------------- */

test('the verdict shape is stable JSON with kind lookup', () => {
  const v = applyLookupGuard({ replyText: 'Paracetamol is a painkiller [2].', sources: SOURCES.paracetamol });
  assert.deepStrictEqual(Object.keys(v), [
    'kind', 'rule', 'outcome', 'displayText', 'rawReply', 'kept', 'withheld', 'citations', 'detectorsSha',
  ]);
  assert.strictEqual(v.kind, 'lookup');
  assert.strictEqual(LOOKUP_RULE, 'dose-cite-v7');
  assert.strictEqual(v.detectorsSha, pin.sha256);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(v)), v);
});

test('a source with no recovered text fails every dose cited to it', () => {
  const v = applyLookupGuard({ replyText: 'Take 500mg [1].', sources: [{ n: 1, docTitle: 'X', sectionPath: 'Dose', text: '' }] });
  assert.deepStrictEqual(v.withheld, [{ sentence: 'Take 500mg [1].', reason: 'dose-not-in-source' }]);
  assert.strictEqual(v.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});
