// src/triage/lookup-guard.test.mjs — node --test src/
//
// The dose-cite-v3 rule, driven ENTIRELY by the JSON fixtures in
// fixtures/lookup-guard/ — the files the triage repo's probes/dose-cite.mjs
// vendors and asserts identity against. Nothing about the rule's behaviour is
// pinned only here: every vector a second implementation must reproduce is in
// a fixture file, and manifest.json pins each file's bytes and case count.
import { test } from 'node:test';
import assert from 'node:assert';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import {
  DOSE_UNITS, LOOKUP_NO_EVIDENCE_TEXT, LOOKUP_RULE, NUMERIC_SPAN_FORMS, OVERDOSE_SECTION_PATTERNS,
  OVERDOSE_SENTENCE_PATTERNS, WITHHELD_BANNER, WITHHELD_REASONS, applyLookupGuard, citationsIn, isOverdoseSection,
  isOverdoseSentence, listMarkersAreASequence, normaliseDoseText, numericScan, sourceHasSpan, sourceSentenceSpans, splitSentences, titleKey,
} from './lookup-guard.js';
import pin from './detectors.pin.js';

const DIR = new URL('./fixtures/lookup-guard/', import.meta.url);
const bytes = (name) => readFileSync(new URL(name, DIR));
const fixture = (name) => JSON.parse(bytes(name).toString('utf8'));

const SOURCES = fixture('sources.json');
const { groups: GROUPS } = fixture('verdicts.json');
const placeholders = (s) => s.replaceAll('WITHHELD', WITHHELD_BANNER).replace(/^NO_EVIDENCE$/, LOOKUP_NO_EVIDENCE_TEXT);

/* ---------------- the fixture set itself ---------------- */

// Regenerate after a reviewed fixture change:
//   node -e "<see manifest.json _comment>"
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
    'boundaries.json', 'citations.json', 'normalise.json', 'overdose-sections.json', 'overdose-sentences.json',
    'sentences.json', 'sources.json', 'spans.json', 'titles.json', 'units.json', 'verdicts.json',
  ]);
});

test('the verdict fixtures have one named group per clause, and every reason is exercised', () => {
  assert.deepStrictEqual(Object.keys(GROUPS), [
    'citation-range', 'number-bearing', 'verbatim-presence', 'one-source-sentence', 'source-title', 'range-handling',
    'multipliers', 'per-kg-and-period', 'intervals', 'overdose-section', 'withholding-and-banner', 'no-evidence-fallback',
    'over-withholding-residual',
  ]);
  for (const [name, cases] of Object.entries(GROUPS)) assert.ok(cases.length >= 3, `${name} has too few cases`);
  const reasons = new Set(Object.values(GROUPS).flat().flatMap((c) => c.expected.withheld.map((w) => w.reason)));
  for (const r of Object.values(WITHHELD_REASONS)) assert.ok(reasons.has(r), `no fixture withholds for ${r}`);
});

/* ---------------- the rule, clause by clause ---------------- */

for (const [group, cases] of Object.entries(GROUPS)) {
  for (const c of cases) {
    test(`dose-cite-v3 [${group}]: ${c.name}`, () => {
      const sources = Array.isArray(c.sources) ? c.sources : SOURCES[c.sources];
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

/* ---------------- the parts, from their vector files ---------------- */

test('units.json is the DOSE_UNITS table, row for row', () => {
  const { units } = fixture('units.json');
  assert.deepStrictEqual(units, DOSE_UNITS.map((u) => ({ canon: u.canon, kind: u.kind, spellings: u.spellings, examples: [...u.examples] })));
  assert.ok(Object.isFrozen(DOSE_UNITS) && DOSE_UNITS.every(Object.isFrozen));
  assert.strictEqual(new Set(DOSE_UNITS.map((u) => u.canon)).size, DOSE_UNITS.length, 'each canon is distinct');
});

for (const u of DOSE_UNITS) {
  test(`unit table: every listed spelling of ${u.canon} is one span, 3${u.canon}`, () => {
    for (const spelling of u.examples) {
      for (const written of [`3 ${spelling}`, `3${spelling}`, `3-${spelling}`]) {
        assert.deepStrictEqual(numericScan(normaliseDoseText(`Take ${written} now.`)), { spans: [`3${u.canon}`], unclassifiable: false }, written);
      }
    }
  });
}

test('normalise.json: normaliseDoseText', () => {
  for (const c of fixture('normalise.json').cases) assert.strictEqual(normaliseDoseText(c.input), c.normalised, c.input);
  assert.notStrictEqual(normaliseDoseText('1 g'), normaliseDoseText('1000 mg'), 'no conversion across units');
});

test('spans.json: numericScan over the normalised text', () => {
  for (const c of fixture('spans.json').cases) {
    assert.deepStrictEqual(numericScan(normaliseDoseText(c.input)), { spans: c.spans, unclassifiable: c.unclassifiable }, c.input);
  }
  assert.deepStrictEqual(NUMERIC_SPAN_FORMS.map((f) => f.name), [
    'product', 'count-of-strength', 'frequency', 'interval', 'gap', 'duration', 'amount', 'word', 'number',
  ]);
});

test('overdose-sentences.json: the registered sentence list is frozen and matches what it names', () => {
  assert.ok(Object.isFrozen(OVERDOSE_SENTENCE_PATTERNS));
  for (const c of fixture('overdose-sentences.json').cases) assert.strictEqual(isOverdoseSentence(c.sentence), c.overdose, c.sentence);
});

test('titles.json: titleKey', () => {
  for (const c of fixture('titles.json').cases) assert.strictEqual(titleKey(c.docTitle), c.key, c.docTitle);
});

test('boundaries.json: sourceHasSpan is whole-span equality against ONE eligible source sentence', () => {
  for (const c of fixture('boundaries.json').cases) {
    assert.strictEqual(sourceHasSpan(c.source, c.span), c.present, `${c.source} / ${c.span}`);
  }
});

// The v3 invariant, checked over every verdict fixture rather than case by
// case: a kept sentence carries no overdose wording, and either carries no
// number at all, or cites, and ONE eligible sentence of one cited source states
// every one of its spans.
test('INVARIANT: every number in a kept sentence is stated by one cited source sentence', () => {
  for (const c of Object.values(GROUPS).flat()) {
    const sources = Array.isArray(c.sources) ? c.sources : SOURCES[c.sources];
    const reply = splitSentences(c.replyText).map((x) => x.text);
    const listMarker = listMarkersAreASequence(reply);
    for (const sentence of applyLookupGuard({ replyText: c.replyText, sources }).kept) {
      assert.ok(!isOverdoseSentence(sentence), `${c.name}: overdose wording kept`);
      const { spans } = numericScan(normaliseDoseText(sentence.replace(/\[[^\]\n]*\]/g, '')), { listMarker });
      if (!spans.length) continue;
      const cites = citationsIn(sentence).numbers;
      assert.ok(cites.length, `${c.name}: uncited number kept: ${sentence}`);
      const one = cites.some((n) => sourceSentenceSpans(sources[n - 1].text).some((src) => spans.every((sp) => src.spans.has(sp))));
      assert.ok(one, `${c.name}: ${spans.join(', ')} not stated by one cited source sentence`);
    }
  }
});

test('sentences.json: splitSentences', () => {
  for (const c of fixture('sentences.json').cases) assert.deepStrictEqual(splitSentences(c.input), c.sentences, c.input);
});

test('citations.json: citationsIn', () => {
  for (const c of fixture('citations.json').cases) {
    assert.deepStrictEqual(citationsIn(c.input), { numbers: c.numbers, malformed: c.malformed }, c.input);
  }
});

test('overdose-sections.json: the registered list is frozen and matches what it names', () => {
  assert.ok(Object.isFrozen(OVERDOSE_SECTION_PATTERNS));
  for (const c of fixture('overdose-sections.json').cases) assert.strictEqual(isOverdoseSection(c.path), c.overdose, c.path);
  assert.strictEqual(isOverdoseSection(undefined), false);
});

/* ---------------- shape ---------------- */

test('the verdict shape is stable JSON with kind lookup', () => {
  const v = applyLookupGuard({ replyText: 'Paracetamol is a painkiller [2].', sources: SOURCES.paracetamol });
  assert.deepStrictEqual(Object.keys(v), [
    'kind', 'rule', 'outcome', 'displayText', 'rawReply', 'kept', 'withheld', 'citations', 'detectorsSha',
  ]);
  assert.strictEqual(v.kind, 'lookup');
  assert.strictEqual(LOOKUP_RULE, 'dose-cite-v3');
  assert.strictEqual(v.detectorsSha, pin.sha256);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(v)), v);
});

test('a source with no recovered text fails every dose cited to it', () => {
  const v = applyLookupGuard({ replyText: 'Take 500mg [1].', sources: [{ n: 1, docTitle: 'X', sectionPath: 'Dose', text: '' }] });
  assert.deepStrictEqual(v.withheld, [{ sentence: 'Take 500mg [1].', reason: 'dose-not-in-source' }]);
  assert.strictEqual(v.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});
