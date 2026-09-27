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
  CITATION_LINE_TEMPLATE, CITATION_SOURCE_SEP, LOOKUP_OVER_BUDGET_TEXT, LOOKUP_PACK_MISMATCH_ERROR,
  LOOKUP_UNAVAILABLE_TEXT, didYouMeanText, lookupForPersistence, packMatchesPin, referencePackId, reportedPack,
  runLookupTurn, sourcesFromPrompt,
} from './lookup-turn.js';
import { LOOKUP_NO_EVIDENCE_TEXT, WITHHELD_BANNER } from './triage/lookup-guard.js';
import { BANNERS, CRISIS_LINE_REPLACE, crisisReplaceBlock } from './triage/guard.js';

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
// What the pinned pack reports about itself (its manifest), equal to ENTRY's pins.
const PACK = Object.freeze({ contentSha256: 'content-sha', packVersion: '2026.09.1' });
const GROUNDED = Object.freeze({ status: 'grounded', prompt: PROMPT, citations: CITATIONS, candidates: [], ...PACK });

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
  assert.strictEqual(r.displayText, crisisReplaceBlock(CRISIS_LINE_REPLACE));
  assert.strictEqual(r.displayText.includes('advice above'), false);
});

/* ---------------- scripted outcomes: no model ---------------- */

test('noEvidence -> the scripted refusal, no model', async () => {
  const invoke = mockInvoke({ status: 'noEvidence', prompt: '[[NO_EVIDENCE]]', citations: [], candidates: [], ...PACK });
  const generate = mockModel();
  const r = await runLookupTurn({ text: 'zzzz', entry: ENTRY, invoke, generate });
  assert.deepStrictEqual(invoke.calls, [{ cmd: 'rag_lookup', args: { query: 'zzzz', packId: 'reference-uk-v1' } }]);
  assert.strictEqual(generate.calls.length, 0);
  assert.strictEqual(r.outcome, 'noEvidence');
  assert.strictEqual(r.displayText, LOOKUP_NO_EVIDENCE_TEXT);
});

test('didYouMean -> the list of titles, no model', async () => {
  const invoke = mockInvoke({ status: 'didYouMean', prompt: null, citations: [], candidates: ['Paracetamol', 'Paracetamol for children'], ...PACK });
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
    text: 'x', entry: ENTRY, invoke: mockInvoke({ status: 'didYouMean', candidates: ['', 7, null], ...PACK }), generate: mockModel(),
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
    text: 'p', entry: ENTRY, invoke: mockInvoke({ status: 'grounded', prompt: PROMPT, citations: [], ...PACK }), generate,
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

/* ---------------- the pack identity check (whole-branch review I2) ---------------- */

test('a pack whose reported content sha or version differs from the catalog pin answers nothing, no model', async () => {
  const mismatches = [
    { ...GROUNDED, contentSha256: 'other-content-sha' },
    { ...GROUNDED, packVersion: '2026.10.1' },
    { ...GROUNDED, contentSha256: null },
    { ...GROUNDED, packVersion: undefined },
    { status: 'noEvidence', prompt: '[[NO_EVIDENCE]]', citations: [], candidates: [], contentSha256: 'other' },
    { status: 'didYouMean', candidates: ['Paracetamol'], packVersion: 'x', contentSha256: 'content-sha' },
  ];
  for (const rag of mismatches) {
    const generate = mockModel('The usual dose is one or two 500mg tablets up to 4 times in 24 hours [1].');
    const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(rag), generate });
    assert.strictEqual(r.outcome, 'unavailable', JSON.stringify(rag));
    assert.strictEqual(r.displayText, LOOKUP_UNAVAILABLE_TEXT);
    assert.strictEqual(r.error, undefined, 'the error lives on the verdict');
    assert.strictEqual(r.verdict.error, LOOKUP_PACK_MISMATCH_ERROR);
    assert.strictEqual(generate.calls.length, 0);
    assert.strictEqual(lookupForPersistence(r.verdict, ENTRY).referencePack, null, 'no pack answered');
  }
});

test('the pin check fails closed when the catalog entry carries no content or version pin', async () => {
  for (const referencePack of [{ id: 'reference-uk-v1' }, { id: 'reference-uk-v1', contentSha256: 'content-sha' }]) {
    const generate = mockModel();
    const r = await runLookupTurn({
      text: 'paracetamol', entry: { ...ENTRY, referencePack }, invoke: mockInvoke(GROUNDED), generate,
    });
    assert.strictEqual(r.outcome, 'unavailable');
    assert.strictEqual(generate.calls.length, 0);
  }
});

test('packMatchesPin and reportedPack read exactly the two manifest fields', () => {
  assert.deepStrictEqual(reportedPack(GROUNDED), { contentSha256: 'content-sha', version: '2026.09.1' });
  assert.strictEqual(reportedPack({ status: 'unavailable', contentSha256: null, packVersion: null }), null);
  assert.strictEqual(reportedPack(null), null);
  assert.strictEqual(packMatchesPin(GROUNDED, ENTRY), true);
  assert.strictEqual(packMatchesPin(GROUNDED, { ...ENTRY, referencePack: { ...ENTRY.referencePack, version: 'v2' } }), false);
  assert.strictEqual(packMatchesPin(GROUNDED, null), false);
});

test('every pack-answered outcome persists the identity the pack reported', async () => {
  const cases = [
    [{ status: 'noEvidence', prompt: '[[NO_EVIDENCE]]', citations: [], candidates: [], ...PACK }, mockModel(), 'noEvidence'],
    [{ status: 'didYouMean', candidates: ['Paracetamol'], ...PACK }, mockModel(), 'didYouMean'],
    [GROUNDED, mockModel('The usual dose is one or two 500mg tablets up to 4 times in 24 hours [1].'), 'grounded'],
  ];
  for (const [rag, generate, outcome] of cases) {
    const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(rag), generate });
    assert.strictEqual(r.outcome, outcome);
    assert.deepStrictEqual(r.verdict.referencePack, { id: 'reference-uk-v1', contentSha256: 'content-sha', version: '2026.09.1' });
    assert.deepStrictEqual(lookupForPersistence(r.verdict, ENTRY).referencePack,
      { id: 'reference-uk-v1', sha256: 'pack-sha', contentSha256: 'content-sha', version: '2026.09.1' });
  }
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

test('grounded: a reply with nothing citable left, over sources with nothing showable, is the scripted refusal', async () => {
  // Phase 1i MA3: the refusal survives only when the excerpt fallback has
  // nothing to show either (here every source sits in a withheld section).
  const cites = CITATIONS.map((c) => ({ ...c, sectionPath: 'If you take too much' }));
  const prompt = PROMPT.replace('How and when to take it', 'If you take too much');
  const r = await runLookupTurn({
    text: 'paracetamol', entry: ENTRY, invoke: mockInvoke({ ...GROUNDED, prompt, citations: cites }),
    generate: mockModel('I am not sure.'),
  });
  assert.strictEqual(r.outcome, 'noEvidence');
  assert.strictEqual(r.displayText, LOOKUP_NO_EVIDENCE_TEXT);
  assert.deepStrictEqual(r.citations, []);
  assert.strictEqual(r.modelCalled, true);
  assert.ok(!('excerptsShown' in r.verdict));
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

test('lookupForPersistence stamps model provenance and the PACK-reported identity on a copy', () => {
  // The pack reported values the catalog does not carry: those, not the
  // catalog's, are what is persisted (sha256 stays the catalog's file pin).
  const v = {
    kind: 'lookup', outcome: 'grounded',
    referencePack: { id: 'reference-uk-v1', contentSha256: 'reported-content', version: 'reported-version' },
  };
  const p = lookupForPersistence(v, ENTRY);
  assert.notStrictEqual(p, v);
  assert.deepStrictEqual(p, {
    kind: 'lookup', outcome: 'grounded', modelSha: 'model-sha', adapterSha: 'adapter-sha',
    referencePack: { id: 'reference-uk-v1', sha256: 'pack-sha', contentSha256: 'reported-content', version: 'reported-version' },
  });
  assert.strictEqual(lookupForPersistence(null, ENTRY), null);
});

test('lookupForPersistence records no pack when none answered (crisis first, unavailable)', async () => {
  const crisis = await runLookupTurn({
    text: 'I want to kill myself', entry: ENTRY, invoke: mockInvoke(GROUNDED), generate: mockModel(),
  });
  assert.strictEqual(lookupForPersistence(crisis.verdict, ENTRY).referencePack, null);
  const missing = await runLookupTurn({
    text: 'paracetamol', entry: ENTRY, invoke: mockInvoke({ status: 'unavailable', contentSha256: null, packVersion: null }),
    generate: mockModel(),
  });
  assert.strictEqual(missing.outcome, 'unavailable');
  assert.strictEqual(lookupForPersistence(missing.verdict, ENTRY).referencePack, null);
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

/* ---------------- fix round 1, M3: one reply length for budget and model ---------------- */

import { lookupReplyTokens } from './lookup-turn.js';

test('the reply length is the entry\'s pinned maxTokens, else 320, and the model is asked for the budgeted value', async () => {
  assert.strictEqual(lookupReplyTokens(ENTRY), 320);
  assert.strictEqual(lookupReplyTokens({ sampling: { maxTokens: 400 } }), 400);
  assert.strictEqual(lookupReplyTokens({ sampling: { maxTokens: 'x' } }), 320);
  const generate = mockModel('Paracetamol treats aches [1].');
  await runLookupTurn({ text: 'paracetamol', entry: { ...ENTRY, sampling: { temperature: 0, maxTokens: 400 } }, invoke: mockInvoke(GROUNDED), generate });
  assert.strictEqual(generate.calls[0].maxTokens, 400);
});

test('a larger pinned reply shrinks the room for the prompt: over budget with 1000, not with 320', async () => {
  const prompt = `${PROMPT}${'x'.repeat(4000)}`;
  const big = { ...GROUNDED, prompt };
  const at320 = await runLookupTurn({ text: 'p', entry: ENTRY, invoke: mockInvoke(big), generate: mockModel('') });
  assert.notStrictEqual(at320.outcome, 'overBudget');
  const generate = mockModel('');
  const at1000 = await runLookupTurn({ text: 'p', entry: { ...ENTRY, sampling: { maxTokens: 1000 } }, invoke: mockInvoke(big), generate });
  assert.strictEqual(at1000.outcome, 'overBudget');
  assert.match(at1000.error, /\+ 1000\)/);
  assert.strictEqual(generate.calls.length, 0);
});

/* ---------------- Phase 1i MA3: the excerpt fallback ---------------- */

import { LOOKUP_EXCERPT_FOOTER } from './lookup-turn.js';
import { EXCERPT_OMISSION } from './triage/lookup-guard.js';
import { replayMessage } from './triage-turn.js';

// Three sources of one page, CITED out of page order: [1] is the page's
// third chunk, [2] its first, [3] an overdose section.
const EX_CITATIONS = [
  { n: 1, packId: 'reference-uk-v1', chunkId: 23, docTitle: 'Paracetamol for adults', sectionPath: 'Side effects', locator: '' },
  {
    n: 2, packId: 'reference-uk-v1', chunkId: 21, docTitle: 'Paracetamol for adults', sectionPath: 'How and when to take it',
    locator: 'L2', url: 'https://www.nhs.uk/medicines/paracetamol-for-adults/', retrievedAt: '2026-09-20',
  },
  { n: 3, packId: 'reference-uk-v1', chunkId: 22, docTitle: 'Paracetamol for adults', sectionPath: 'If you take too much', locator: '' },
];
const EX_TEXTS = [
  'Side effects are rare. Most people have no problems. Taking too much can cause liver damage. Keep it out of reach. It is sold in pharmacies.',
  'The usual dose is one or two 500mg tablets up to 4 times in 24 hours. Swallow the tablets with water.',
  'Paracetamol is dangerous in overdose. Go to A&E straight away.',
];
const EX_PROMPT = `${CONTRACT}\n\nThese sources are excerpts from: Paracetamol for adults.\n\n`
  + `[1] (Paracetamol for adults, Side effects): ${EX_TEXTS[0]}\n`
  + `[2] (Paracetamol for adults, How and when to take it, L2): ${EX_TEXTS[1]}\n`
  + `[3] (Paracetamol for adults, If you take too much): ${EX_TEXTS[2]}`;
const EX_GROUNDED = Object.freeze({ status: 'grounded', prompt: EX_PROMPT, citations: EX_CITATIONS, candidates: [], ...PACK });
// A paraphrase: every content sentence withheld, nothing kept.
const PARAPHRASE = 'You can take 1g up to four times a day [2]. Take two tablets with food [1].';

const EXPECTED_EXCERPTS = [
  '[2] Paracetamol for adults · How and when to take it',
  'https://www.nhs.uk/medicines/paracetamol-for-adults/ · as at 2026-09-20',
  EX_TEXTS[1],
  '',
  '[1] Paracetamol for adults · Side effects',
  `Side effects are rare. ${EXCERPT_OMISSION} It is sold in pharmacies.`,
].join('\n');

async function excerptTurn(reply = PARAPHRASE) {
  const generate = mockModel(reply);
  const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(EX_GROUNDED), generate });
  return { r, generate };
}

test('grounded -> model -> nothing kept -> the excerpts, verbatim, in page order, each under its citation line', async () => {
  const { r, generate } = await excerptTurn();
  assert.strictEqual(generate.calls.length, 1);
  assert.strictEqual(r.modelCalled, true);
  assert.strictEqual(r.messageId, 42);
  assert.strictEqual(r.outcome, 'excerpts');
  assert.strictEqual(r.displayText, EXPECTED_EXCERPTS);
});

test('the excerpts never show the model\'s words', async () => {
  const { r } = await excerptTurn();
  assert.ok(!r.displayText.includes('1g'));
  assert.ok(!r.displayText.includes('with food'));
  assert.ok(!r.displayText.includes(WITHHELD_BANNER));
  assert.deepStrictEqual(r.verdict.kept, [], 'no model sentence was shown');
});

test('the excerpts obey the guard\'s eligibility: a withheld section shows nothing, an overdose sentence and its neighbours are dropped', async () => {
  const { r } = await excerptTurn();
  for (const gone of ['dangerous', 'A&E', 'liver damage', 'Keep it out of reach', 'Most people', 'Taking too much']) {
    assert.ok(!r.displayText.includes(gone), gone);
  }
  assert.ok(!r.displayText.includes('[3]'), 'the overdose-section source has no block');
  assert.strictEqual(r.verdict.excerptsShown, 4);
});

test('the excerpts carry the OGL footer for excerpts, not the assistant-wording one', async () => {
  const { r } = await excerptTurn();
  assert.strictEqual(lookupFooterFor(r.outcome), LOOKUP_EXCERPT_FOOTER);
  assert.notStrictEqual(LOOKUP_EXCERPT_FOOTER, LOOKUP_SOURCE_FOOTER);
  assert.match(LOOKUP_EXCERPT_FOOTER, /^Reference pages: NHS website, Open Government Licence v3\.0\./);
  assert.ok(!/assistant/.test(LOOKUP_EXCERPT_FOOTER));
  assert.strictEqual(lookupFooterFor('grounded'), LOOKUP_SOURCE_FOOTER);
});

test('the excerpt verdict: kind lookup, outcome excerpts, the rule, the shown sources as citations, the withheld tally, excerptsShown', async () => {
  const { r } = await excerptTurn();
  const v = lookupForPersistence(r.verdict, ENTRY);
  assert.strictEqual(v.kind, 'lookup');
  assert.strictEqual(v.outcome, 'excerpts');
  assert.strictEqual(v.rule, 'dose-cite-v6');
  assert.strictEqual(v.displayText, EXPECTED_EXCERPTS);
  assert.deepStrictEqual(v.citations, [1, 2]);
  assert.deepStrictEqual(v.withheld.map((w) => w.reason), ['dose-not-in-source', 'dose-not-in-source']);
  assert.strictEqual(v.excerptsShown, 4);
  assert.strictEqual(v.rawReply, PARAPHRASE, 'the raw reply stays where the grounded path keeps it');
  assert.deepStrictEqual(v.sources.map((s) => s.n), [1, 2, 3]);
  assert.ok(v.sources.every((s) => !('text' in s)));
  assert.deepStrictEqual(v.referencePack,
    { id: 'reference-uk-v1', sha256: 'pack-sha', contentSha256: 'content-sha', version: '2026.09.1' });
  assert.deepStrictEqual(r.citations, [EX_CITATIONS[0], EX_CITATIONS[1]], 'the sources shown, in citation order');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(v)), v);
});

test('an excerpts row replays from its persisted record alone, like a grounded one', async () => {
  const { r } = await excerptTurn();
  const guard = JSON.parse(JSON.stringify(lookupForPersistence(r.verdict, ENTRY)));
  const view = replayMessage({ supervised: true, role: 'assistant', content: 'RAW paraphrase', guard });
  assert.strictEqual(view.banner, null);
  assert.strictEqual(view.withheld, false);
  assert.strictEqual(view.text, EXPECTED_EXCERPTS);
  assert.strictEqual(view.lookup.outcome, 'excerpts');
  assert.strictEqual(view.lookup.footer, LOOKUP_EXCERPT_FOOTER);
  assert.deepStrictEqual(view.lookup.citations.map((c) => c.n), [1, 2]);
  assert.strictEqual(view.lookup.withheldCount, 2);
});

test('a grounded reply that keeps one sentence is unchanged by the fallback', async () => {
  const { r } = await excerptTurn(
    'The usual dose is one or two 500mg tablets up to 4 times in 24 hours [2]. You could take 1g.');
  assert.strictEqual(r.outcome, 'grounded');
  assert.strictEqual(r.displayText,
    `The usual dose is one or two 500mg tablets up to 4 times in 24 hours [2]. ${WITHHELD_BANNER}`);
  assert.ok(!('excerptsShown' in r.verdict));
  assert.strictEqual(lookupFooterFor(r.outcome), LOOKUP_SOURCE_FOOTER);
});

test('the scripted branches never fall back to excerpts and never call the model', async () => {
  const cases = [
    [{ status: 'noEvidence', ...PACK }, 'noEvidence'],
    [{ status: 'didYouMean', candidates: ['Paracetamol for adults'], ...PACK }, 'didYouMean'],
    [{ status: 'unavailable' }, 'unavailable'],
    [{ ...EX_GROUNDED, citations: [] }, 'noEvidence'],
  ];
  for (const [rag, outcome] of cases) {
    const generate = mockModel(PARAPHRASE);
    const r = await runLookupTurn({ text: 'paracetamol', entry: ENTRY, invoke: mockInvoke(rag), generate });
    assert.strictEqual(r.outcome, outcome, outcome);
    assert.strictEqual(generate.calls.length, 0, outcome);
    assert.ok(!('excerptsShown' in r.verdict), outcome);
  }
  const crisis = await runLookupTurn({
    text: 'I want to kill myself', entry: ENTRY, invoke: mockInvoke(EX_GROUNDED), generate: mockModel(PARAPHRASE),
  });
  assert.strictEqual(crisis.outcome, 'crisis');
  assert.strictEqual(crisis.modelCalled, false);
});
