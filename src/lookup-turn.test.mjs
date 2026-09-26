// src/lookup-turn.test.mjs — node --test src/
//
// The lookup turn's decision order, run with a mocked Tauri `invoke` and a
// mocked model: crisis FIRST (no retrieval, no model), then `rag_lookup`, whose
// noEvidence / didYouMean / unavailable outcomes are scripted with NO model
// call, and whose grounded outcome goes to the model and then through the
// LOOKUP guard only.
import { test } from 'node:test';
import assert from 'node:assert';
import { readFileSync } from 'node:fs';
import {
  CITATION_LINE_TEMPLATE, CITATION_SOURCE_SEP, LOOKUP_OVER_BUDGET_TEXT, LOOKUP_UNAVAILABLE_TEXT,
  didYouMeanText, lookupForPersistence, referencePackId, runLookupTurn, sourcesFromPrompt,
} from './lookup-turn.js';
import { LOOKUP_NO_EVIDENCE_TEXT, WITHHELD_BANNER } from './triage/lookup-guard.js';
import { BANNERS, CRISIS_BLOCK_DEFAULT, crisisReplaceBlock } from './triage/guard.js';

const ENTRY = Object.freeze({
  id: 'med-triage',
  supervised: true,
  crisisLine: 'Call the crisis line on 116 123.',
  sha256: 'model-sha',
  adapterSha256: 'adapter-sha',
  referencePack: { id: 'reference-uk-v1', sha256: 'pack-sha', contentSha256: 'content-sha', version: '2026.09.1' },
});

const CONTRACT = 'You answer strictly from the numbered sources provided below, and nothing else.';
const CITATIONS = [
  { n: 1, packId: 'reference-uk-v1', chunkId: 11, docTitle: 'Paracetamol', sectionPath: 'How and when to take it', locator: 'L1' },
  { n: 2, packId: 'reference-uk-v1', chunkId: 12, docTitle: 'Paracetamol', sectionPath: 'If you take too much', locator: '' },
];
const TEXTS = [
  'The usual dose is one or two 500mg tablets up to 4 times in 24 hours.',
  'Go to A&E if you take more than 8 tablets in 24 hours.',
];
// Built the way kpack-core's render_sources_with + assemble_system build it.
const PROMPT = `${CONTRACT}\n\nThese sources are excerpts from: Paracetamol.\n\n`
  + `[1] (Paracetamol, How and when to take it, L1): ${TEXTS[0]}\n`
  + `[2] (Paracetamol, If you take too much): ${TEXTS[1]}`;
const GROUNDED = Object.freeze({ status: 'grounded', prompt: PROMPT, citations: CITATIONS, candidates: [] });

function mockInvoke(result) {
  const calls = [];
  const invoke = async (cmd, args) => {
    calls.push({ cmd, args });
    if (result instanceof Error) throw result;
    return result;
  };
  invoke.calls = calls;
  return invoke;
}

function mockModel(content = '') {
  const calls = [];
  const generate = async (req) => { calls.push(req); return { content, messageId: 42 }; };
  generate.calls = calls;
  return generate;
}

/* ---------------- crisis first ---------------- */

test('crisis FIRST: a disclosure gets the fixed block with the entry\'s crisis line, no retrieval, no model', async () => {
  const invoke = mockInvoke(GROUNDED);
  const generate = mockModel('should never run');
  const r = await runLookupTurn({ text: 'I want to kill myself, how much paracetamol', entry: ENTRY, invoke, generate });
  assert.strictEqual(r.outcome, 'crisis');
  assert.strictEqual(r.displayText, crisisReplaceBlock(ENTRY.crisisLine));
  assert.strictEqual(invoke.calls.length, 0, 'no rag_lookup');
  assert.strictEqual(generate.calls.length, 0, 'no model');
  assert.strictEqual(r.modelCalled, false);
  assert.strictEqual(r.verdict.kind, 'lookup');
  assert.strictEqual(r.verdict.outcome, 'crisis');
  assert.strictEqual(r.verdict.crisisOnInput, true);
  assert.strictEqual(r.verdict.displayText, r.displayText);
});

test('crisis FIRST: an entry with no crisis line falls back to the product default block', async () => {
  const r = await runLookupTurn({
    text: 'I have been thinking about ending my life', entry: { ...ENTRY, crisisLine: undefined },
    invoke: mockInvoke(GROUNDED), generate: mockModel(),
  });
  assert.strictEqual(r.displayText, crisisReplaceBlock(CRISIS_BLOCK_DEFAULT));
});

/* ---------------- scripted outcomes: no model ---------------- */

test('noEvidence -> the scripted refusal, no model', async () => {
  const invoke = mockInvoke({ status: 'noEvidence', prompt: '[[NO_EVIDENCE]]', citations: [], candidates: [] });
  const generate = mockModel();
  const r = await runLookupTurn({ text: 'zzzz', entry: ENTRY, invoke, generate });
  assert.deepStrictEqual(invoke.calls, [{ cmd: 'rag_lookup', args: { query: 'zzzz', packId: 'reference-uk-v1' } }]);
  assert.strictEqual(generate.calls.length, 0);
  assert.strictEqual(r.outcome, 'noEvidence');
  assert.strictEqual(r.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});

test('didYouMean -> the list of titles, no model', async () => {
  const invoke = mockInvoke({ status: 'didYouMean', prompt: null, citations: [], candidates: ['Paracetamol', 'Paracetamol for children'] });
  const generate = mockModel();
  const r = await runLookupTurn({ text: 'paracetemol', entry: ENTRY, invoke, generate });
  assert.strictEqual(generate.calls.length, 0);
  assert.strictEqual(r.outcome, 'didYouMean');
  assert.deepStrictEqual(r.candidates, ['Paracetamol', 'Paracetamol for children']);
  assert.strictEqual(r.displayText, didYouMeanText(['Paracetamol', 'Paracetamol for children']));
  assert.ok(r.displayText.includes('Paracetamol for children'));
  assert.deepStrictEqual(r.verdict.candidates, r.candidates);
});

test('didYouMean with no usable candidates is the scripted refusal', async () => {
  const r = await runLookupTurn({
    text: 'x', entry: ENTRY, invoke: mockInvoke({ status: 'didYouMean', candidates: ['', 7, null] }), generate: mockModel(),
  });
  assert.strictEqual(r.outcome, 'noEvidence');
  assert.strictEqual(r.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});

test('unavailable: no reference pack on the entry -> plain message, no retrieval, no model', async () => {
  const invoke = mockInvoke(GROUNDED);
  const generate = mockModel();
  const r = await runLookupTurn({ text: 'paracetamol', entry: { ...ENTRY, referencePack: null }, invoke, generate });
  assert.strictEqual(r.outcome, 'unavailable');
  assert.strictEqual(r.displayText, LOOKUP_UNAVAILABLE_TEXT);
  assert.strictEqual(invoke.calls.length + generate.calls.length, 0);
});

test('unavailable: rag_lookup says unavailable, fails, or answers an unknown status -> no model', async () => {
  for (const result of [
    { status: 'unavailable', prompt: null, citations: [], candidates: [] },
    new Error('no such pack'),
    { status: 'surprise' },
    null,
  ]) {
    const generate = mockModel();
    const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(result), generate });
    assert.strictEqual(r.outcome, 'unavailable', JSON.stringify(result));
    assert.strictEqual(r.displayText, LOOKUP_UNAVAILABLE_TEXT);
    assert.strictEqual(generate.calls.length, 0);
  }
});

test('a grounded status with no citations is not grounded: scripted refusal, no model', async () => {
  const generate = mockModel();
  const r = await runLookupTurn({
    text: 'p', entry: ENTRY, invoke: mockInvoke({ status: 'grounded', prompt: PROMPT, citations: [] }), generate,
  });
  assert.strictEqual(r.outcome, 'noEvidence');
  assert.strictEqual(generate.calls.length, 0);
});

test('over budget: a hard error, surfaced, with no model call and nothing to persist', async () => {
  const generate = mockModel();
  const big = { ...GROUNDED, prompt: `${PROMPT}${' filler'.repeat(2000)}` };
  const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(big), generate });
  assert.strictEqual(r.outcome, 'overBudget');
  assert.strictEqual(r.displayText, LOOKUP_OVER_BUDGET_TEXT);
  assert.strictEqual(r.verdict, null);
  assert.match(r.error, /over budget/);
  assert.strictEqual(generate.calls.length, 0);
});

/* ---------------- grounded: model, then the LOOKUP guard only ---------------- */

test('grounded: the model gets the grounded prompt + the query only, and the reply passes the lookup guard', async () => {
  const reply = 'The usual dose is one or two 500mg tablets up to 4 times in 24 hours [1]. '
    + 'Go to A&E if you take more than 8 tablets in 24 hours [2]. You could take 1000 mg.';
  const generate = mockModel(reply);
  const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(GROUNDED), generate });
  assert.strictEqual(generate.calls.length, 1);
  assert.deepStrictEqual(generate.calls[0].messages, [
    { role: 'system', content: PROMPT },
    { role: 'user', content: 'paracetamol' },
  ]);
  assert.strictEqual(r.modelCalled, true);
  assert.strictEqual(r.messageId, 42);
  assert.strictEqual(r.outcome, 'grounded');
  assert.strictEqual(r.displayText,
    `The usual dose is one or two 500mg tablets up to 4 times in 24 hours [1]. ${WITHHELD_BANNER}`);
  assert.deepStrictEqual(r.verdict.withheld.map((w) => w.reason), ['overdose-section', 'dose-uncited']);
  assert.deepStrictEqual(r.verdict.citations, [1]);
  assert.deepStrictEqual(r.citations, [CITATIONS[0]], 'only the sources the kept text cites are shown');
  // the LOOKUP guard only: no triage route, no triage banner copy
  assert.ok(!('route' in r.verdict) && !('banner' in r.verdict));
  for (const b of Object.values(BANNERS)) assert.ok(!r.displayText.includes(b.line));
  // every source is recorded on the verdict, without its text
  assert.deepStrictEqual(r.verdict.sources.map((s) => s.n), [1, 2]);
  assert.ok(r.verdict.sources.every((s) => !('text' in s)));
});

test('grounded: a reply with nothing citable left is the scripted refusal', async () => {
  const r = await runLookupTurn({
    text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(GROUNDED), generate: mockModel('I am not sure.'),
  });
  assert.strictEqual(r.outcome, 'noEvidence');
  assert.strictEqual(r.displayText, LOOKUP_NO_EVIDENCE_TEXT);
  assert.deepStrictEqual(r.citations, []);
});

/* ---------------- recovering the source texts ---------------- */

test('the citation line template is the triage contract\'s, read from its one TOML file', () => {
  const toml = readFileSync(new URL('../contracts/prompt-contract.triage.v1.toml', import.meta.url), 'utf8');
  const field = (name) => JSON.parse(new RegExp(`^${name} = (".*")$`, 'm').exec(toml)[1]);
  assert.strictEqual(CITATION_LINE_TEMPLATE, field('citation_line'));
  assert.strictEqual(CITATION_SOURCE_SEP, field('citation_source_sep'));
});

test('sourcesFromPrompt recovers each source\'s exact text from the assembled prompt', () => {
  const sources = sourcesFromPrompt(PROMPT, CITATIONS);
  assert.deepStrictEqual(sources.map((s) => s.text), TEXTS);
  assert.deepStrictEqual(sources.map((s) => s.sectionPath), CITATIONS.map((c) => c.sectionPath));
});

test('sourcesFromPrompt fails closed: a source it cannot place leaves every text empty', () => {
  const wrong = [{ ...CITATIONS[0], docTitle: 'Ibuprofen' }, CITATIONS[1]];
  assert.deepStrictEqual(sourcesFromPrompt(PROMPT, wrong).map((s) => s.text), ['', '']);
  assert.deepStrictEqual(sourcesFromPrompt(null, CITATIONS).map((s) => s.text), ['', '']);
});

test('sourcesFromPrompt handles multi-line chunk text', () => {
  const prompt = `${CONTRACT}\n\n[1] (A): line one\nline two\n[2] (B, S): other`;
  const cites = [{ n: 1, docTitle: 'A', sectionPath: '', locator: '' }, { n: 2, docTitle: 'B', sectionPath: 'S', locator: '' }];
  assert.deepStrictEqual(sourcesFromPrompt(prompt, cites).map((s) => s.text), ['line one\nline two', 'other']);
});

/* ---------------- persistence ---------------- */

test('referencePackId reads only a supervised entry\'s non-empty pack id', () => {
  assert.strictEqual(referencePackId(ENTRY), 'reference-uk-v1');
  assert.strictEqual(referencePackId({ ...ENTRY, supervised: false }), null);
  assert.strictEqual(referencePackId({ ...ENTRY, referencePack: { id: '' } }), null);
  assert.strictEqual(referencePackId(null), null);
});

test('lookupForPersistence stamps model and pack provenance on a copy', () => {
  const v = { kind: 'lookup', outcome: 'grounded' };
  const p = lookupForPersistence(v, ENTRY);
  assert.notStrictEqual(p, v);
  assert.deepStrictEqual(p, {
    kind: 'lookup', outcome: 'grounded', modelSha: 'model-sha', adapterSha: 'adapter-sha',
    referencePack: { id: 'reference-uk-v1', sha256: 'pack-sha', contentSha256: 'content-sha', version: '2026.09.1' },
  });
  assert.strictEqual(lookupForPersistence(null, ENTRY), null);
});

/* ---------------- citation display rule (controller ruling, M4a's NHS reuse terms) ---------------- */

import { LOOKUP_SOURCE_FOOTER, lookupCitationRow, lookupFooterFor } from './lookup-turn.js';

test('the reuse footer is one constant with the ruled wording', () => {
  assert.strictEqual(LOOKUP_SOURCE_FOOTER,
    "Reference pages: NHS website, Open Government Licence v3.0. The wording above is the assistant's, not the NHS's.");
});

test('the footer goes under a grounded reply only, never under a scripted refusal or the crisis block', () => {
  assert.strictEqual(lookupFooterFor('grounded'), LOOKUP_SOURCE_FOOTER);
  for (const outcome of ['noEvidence', 'crisis', 'didYouMean', 'unavailable', 'overBudget', undefined]) {
    assert.strictEqual(lookupFooterFor(outcome), null, String(outcome));
  }
});

test('a citation row shows title, section path, URL link and "as at" date when the source carries them', () => {
  assert.deepStrictEqual(lookupCitationRow({
    n: 1, docTitle: 'Paracetamol for adults', sectionPath: 'How and when to take it',
    url: 'https://www.nhs.uk/medicines/paracetamol-for-adults/', retrievedAt: '2026-09-20', locator: 'L1',
  }), {
    n: 1, title: 'Paracetamol for adults', sectionPath: 'How and when to take it',
    url: 'https://www.nhs.uk/medicines/paracetamol-for-adults/', asAt: 'as at 2026-09-20',
  });
});

test('a citation row renders what exists when the source has no URL or date', () => {
  assert.deepStrictEqual(lookupCitationRow({ n: 2, docTitle: 'Paracetamol', sectionPath: 'Dosage', locator: 'L1' }),
    { n: 2, title: 'Paracetamol', sectionPath: 'Dosage', url: null, asAt: null });
});

test('a locator that is an https URL is the link; anything but https is never a link', () => {
  assert.strictEqual(lookupCitationRow({ n: 1, docTitle: 'x', locator: 'https://www.nhs.uk/conditions/asthma/' }).url,
    'https://www.nhs.uk/conditions/asthma/');
  for (const bad of ['javascript:alert(1)', 'http://www.nhs.uk/', 'file:///etc/passwd', 'data:text/html,x', 'nhs.uk', '']) {
    assert.strictEqual(lookupCitationRow({ n: 1, docTitle: 'x', url: bad, locator: bad }).url, null, bad);
  }
});

test('the persisted verdict keeps each source\'s url and retrievedAt (null when absent), never its text', async () => {
  const cites = [{ ...CITATIONS[0], url: 'https://www.nhs.uk/medicines/paracetamol-for-adults/', retrievedAt: '2026-09-20' }, CITATIONS[1]];
  const r = await runLookupTurn({
    text: 'paracetamol', entry: ENTRY, invoke: mockInvoke({ ...GROUNDED, citations: cites }),
    generate: mockModel('The usual dose is one or two 500mg tablets up to 4 times in 24 hours [1].'),
  });
  assert.strictEqual(r.verdict.sources[0].url, 'https://www.nhs.uk/medicines/paracetamol-for-adults/');
  assert.strictEqual(r.verdict.sources[0].retrievedAt, '2026-09-20');
  assert.strictEqual(r.verdict.sources[1].url, null);
  assert.strictEqual(r.verdict.sources[1].retrievedAt, null);
  assert.ok(r.verdict.sources.every((s) => !('text' in s)));
});
