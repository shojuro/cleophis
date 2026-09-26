// src/triage/lookup-guard.test.mjs — node --test src/
//
// The dose-cite-v1 rule over its fixtures (the files the triage repo's
// probes/dose-cite.mjs will vendor and assert identity against), plus the
// pieces the fixtures lean on: the normalisation, the dose tokens, the
// sentence split, and the registered overdose section list.
import { test } from 'node:test';
import assert from 'node:assert';
import { readFileSync } from 'node:fs';
import {
  LOOKUP_NO_EVIDENCE_TEXT, LOOKUP_RULE, OVERDOSE_SECTION_PATTERNS, WITHHELD_BANNER, WITHHELD_REASONS,
  applyLookupGuard, citationsIn, doseTokens, isOverdoseSection, normaliseDoseText, sourceHasToken,
  splitSentences,
} from './lookup-guard.js';
import pin from './detectors.pin.js';

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/lookup-guard/${name}`, import.meta.url), 'utf8'));
const SOURCES = fixture('sources.json');
const { cases: CASES } = fixture('cases.json');
const placeholders = (s) => s.replaceAll('WITHHELD', WITHHELD_BANNER).replace(/^NO_EVIDENCE$/, LOOKUP_NO_EVIDENCE_TEXT);

test('the fixtures cover every clause of the rule', () => {
  const clauses = new Set(CASES.map((c) => c.clause));
  for (const c of ['1', '2', '3', '4', '5']) assert.ok(clauses.has(c), `no fixture for clause ${c}`);
  const reasons = new Set(CASES.flatMap((c) => c.expected.withheld.map((w) => w.reason)));
  for (const r of Object.values(WITHHELD_REASONS)) assert.ok(reasons.has(r), `no fixture withholds for ${r}`);
});

for (const c of CASES) {
  test(`dose-cite-v1 fixture: ${c.name}`, () => {
    const sources = SOURCES[c.sources];
    assert.ok(sources, `unknown source set ${c.sources}`);
    const v = applyLookupGuard({ replyText: c.replyText, sources });
    assert.strictEqual(v.outcome, c.expected.outcome);
    assert.strictEqual(v.displayText, placeholders(c.expected.displayText));
    assert.deepStrictEqual(v.withheld, c.expected.withheld);
    assert.deepStrictEqual(v.citations, c.expected.citations);
    // kept + withheld partition the reply's sentences, in order
    const all = splitSentences(c.replyText).map((s) => s.text);
    assert.deepStrictEqual([...v.kept, ...v.withheld.map((w) => w.sentence)].sort(), [...all].sort());
  });
}

test('the verdict shape is stable JSON with kind lookup', () => {
  const v = applyLookupGuard({ replyText: 'Paracetamol is a painkiller [2].', sources: SOURCES.paracetamol });
  assert.deepStrictEqual(Object.keys(v), [
    'kind', 'rule', 'outcome', 'displayText', 'rawReply', 'kept', 'withheld', 'citations', 'detectorsSha',
  ]);
  assert.strictEqual(v.kind, 'lookup');
  assert.strictEqual(v.rule, LOOKUP_RULE);
  assert.strictEqual(LOOKUP_RULE, 'dose-cite-v1');
  assert.strictEqual(v.detectorsSha, pin.sha256);
  assert.strictEqual(v.rawReply, 'Paracetamol is a painkiller [2].');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(v)), v);
});

test('a source with no recovered text fails every dose cited to it', () => {
  const sources = [{ n: 1, docTitle: 'X', sectionPath: 'Dose', text: '' }];
  const v = applyLookupGuard({ replyText: 'Take 500mg [1].', sources });
  assert.deepStrictEqual(v.withheld, [{ sentence: 'Take 500mg [1].', reason: 'dose-not-in-source' }]);
  assert.strictEqual(v.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});

test('normaliseDoseText changes spelling, never quantity', () => {
  const cases = [
    ['500 milligrams three times a day', '500mg 3 times/day'],
    ['500 mg three times daily', '500mg 3 times/day'],
    ['Take 1 or 2 puffs every 4–6 hours', 'take 1-2puffs every 4-6 hours'],
    ['every 4 to 6 hrs', 'every 4-6 hours'],
    ['250 micrograms', '250mcg'],
    ['250 µg', '250mcg'],
    ['250 μg', '250mcg'],
    ['5 mL', '5ml'],
    ['5 millilitres', '5ml'],
    ['1,000 units', '1000units'],
    ['400 IU', '400iu'],
    ['twice a day', '2 times/day'],
    ['once daily', '1 times/day'],
    ['10 mg per kg', '10mg/kg'],
    ['2 tabs', '2tablets'],
    ['1% cream', '1% cream'],
    ['1 g', '1g'],
  ];
  for (const [input, want] of cases) assert.strictEqual(normaliseDoseText(input), want, input);
  // no conversion across units
  assert.notStrictEqual(normaliseDoseText('1 g'), normaliseDoseText('1000 mg'));
});

test('doseTokens finds amounts, frequencies and intervals, and nothing else', () => {
  assert.deepStrictEqual(doseTokens(normaliseDoseText('Take 500 mg twice a day, every 4 hours.')),
    ['500mg', '2 times/day', 'every 4 hours']);
  assert.deepStrictEqual(doseTokens(normaliseDoseText('Give 10 mg/kg, or 250 mg/5 ml liquid.')), ['10mg/kg', '250mg/5ml']);
  assert.deepStrictEqual(doseTokens(normaliseDoseText('It is sold in packs of 16 [1].')), []);
  assert.deepStrictEqual(doseTokens(normaliseDoseText('Paracetamol reduces a high temperature.')), []);
  assert.deepStrictEqual(doseTokens(normaliseDoseText('Use 2 drops, 3 puffs, one sachet, 5%.')),
    ['2drops', '3puffs', '1sachets', '5%']);
});

test('sourceHasToken matches whole tokens only', () => {
  assert.ok(sourceHasToken('take 500mg tablets', '500mg'));
  assert.ok(!sourceHasToken('take 1500mg tablets', '500mg'));
  assert.ok(!sourceHasToken('give 500mg/kg', '500mg'));
  assert.ok(!sourceHasToken('give 2.55ml', '2.5ml'));
  assert.ok(sourceHasToken('then every 4 hours.', 'every 4 hours'));
});

test('splitSentences keeps separators and trailing citations', () => {
  assert.deepStrictEqual(splitSentences('A 2.5 ml dose [1]. B, e.g. this. [2]\nC!'), [
    { text: 'A 2.5 ml dose [1].', sep: ' ' },
    { text: 'B, e.g. this. [2]', sep: '\n' },
    { text: 'C!', sep: '' },
  ]);
  assert.deepStrictEqual(splitSentences('   '), []);
});

test('citationsIn reads lists, flags malformed brackets, ignores markers without digits', () => {
  assert.deepStrictEqual(citationsIn('x [1] y [2, 3]'), { numbers: [1, 2, 3], malformed: false });
  assert.deepStrictEqual(citationsIn('x [1-3]'), { numbers: [], malformed: true });
  assert.deepStrictEqual(citationsIn('x [source 2]'), { numbers: [], malformed: true });
  assert.deepStrictEqual(citationsIn('x [[NO_EVIDENCE]]'), { numbers: [], malformed: false });
});

test('the overdose section list is registered, frozen, and matches the sections it names', () => {
  assert.ok(Object.isFrozen(OVERDOSE_SECTION_PATTERNS));
  for (const path of [
    'Paracetamol > If you take too much',
    'Overdose',
    'Ibuprofen > Overdosage',
    'Maximum dose',
    'Dosage > Maximum daily dose',
    'Max dose',
    'How much is too much?',
    'Paracetamol poisoning',
    'Toxicity',
  ]) assert.ok(isOverdoseSection(path), path);
  for (const path of ['Dosage', 'How and when to take it', 'Side effects', 'Dosage > Adults', '', undefined]) {
    assert.ok(!isOverdoseSection(path), String(path));
  }
});
