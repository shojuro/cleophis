// src/triage/lookup-guard.test.mjs — node --test src/
//
// The dose-cite-v1 rule, driven ENTIRELY by the JSON fixtures in
// fixtures/lookup-guard/ — the files the triage repo's probes/dose-cite.mjs
// vendors and asserts identity against. Nothing about the rule's behaviour is
// pinned only here: every vector a second implementation must reproduce is in
// a fixture file, and manifest.json pins each file's bytes and case count.
import { test } from 'node:test';
import assert from 'node:assert';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import {
  DOSE_UNITS, LOOKUP_NO_EVIDENCE_TEXT, LOOKUP_RULE, OVERDOSE_SECTION_PATTERNS, WITHHELD_BANNER, WITHHELD_REASONS,
  applyLookupGuard, citationsIn, doseScan, isOverdoseSection, normaliseDoseText, sourceHasToken, splitSentences,
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
    'boundaries.json', 'citations.json', 'normalise.json', 'overdose-sections.json', 'sentences.json',
    'sources.json', 'tokens.json', 'units.json', 'verdicts.json',
  ]);
});

test('the verdict fixtures have one named group per clause, and every reason is exercised', () => {
  assert.deepStrictEqual(Object.keys(GROUPS), [
    'citation-range', 'dose-recognition', 'verbatim-presence', 'range-handling',
    'overdose-section', 'withholding-and-banner', 'no-evidence-fallback',
  ]);
  for (const [name, cases] of Object.entries(GROUPS)) assert.ok(cases.length >= 3, `${name} has too few cases`);
  const reasons = new Set(Object.values(GROUPS).flat().flatMap((c) => c.expected.withheld.map((w) => w.reason)));
  for (const r of Object.values(WITHHELD_REASONS)) assert.ok(reasons.has(r), `no fixture withholds for ${r}`);
});

/* ---------------- the rule, clause by clause ---------------- */

for (const [group, cases] of Object.entries(GROUPS)) {
  for (const c of cases) {
    test(`dose-cite-v1 [${group}]: ${c.name}`, () => {
      const sources = SOURCES[c.sources];
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
  test(`unit table: every listed spelling of ${u.canon} is a ${u.canon} dose`, () => {
    for (const spelling of u.examples) {
      for (const written of [`3 ${spelling}`, `3${spelling}`]) {
        const { tokens, unrecognised } = doseScan(normaliseDoseText(`Take ${written} now.`));
        assert.deepStrictEqual(tokens, [`3${u.canon}`], written);
        assert.deepStrictEqual(unrecognised, [], written);
      }
    }
  });
}

test('normalise.json: normaliseDoseText', () => {
  for (const c of fixture('normalise.json').cases) assert.strictEqual(normaliseDoseText(c.input), c.normalised, c.input);
  assert.notStrictEqual(normaliseDoseText('1 g'), normaliseDoseText('1000 mg'), 'no conversion across units');
});

test('tokens.json: doseScan over the normalised text', () => {
  for (const c of fixture('tokens.json').cases) {
    assert.deepStrictEqual(doseScan(normaliseDoseText(c.input)), { tokens: c.tokens, unrecognised: c.unrecognised }, c.input);
  }
});

test('boundaries.json: sourceHasToken is whole-token equality', () => {
  for (const c of fixture('boundaries.json').cases) {
    assert.strictEqual(sourceHasToken(normaliseDoseText(c.source), c.token), c.present, `${c.source} / ${c.token}`);
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
  assert.strictEqual(LOOKUP_RULE, 'dose-cite-v1');
  assert.strictEqual(v.detectorsSha, pin.sha256);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(v)), v);
});

test('a source with no recovered text fails every dose cited to it', () => {
  const v = applyLookupGuard({ replyText: 'Take 500mg [1].', sources: [{ n: 1, docTitle: 'X', sectionPath: 'Dose', text: '' }] });
  assert.deepStrictEqual(v.withheld, [{ sentence: 'Take 500mg [1].', reason: 'dose-not-in-source' }]);
  assert.strictEqual(v.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});
