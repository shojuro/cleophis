// src/triage/excerpt-display.test.mjs — node --test src/
//
// Phase 1i MA3: `excerptDisplay`, the source text a grounded lookup shows
// verbatim when the model's reply kept nothing. Its eligibility IS the
// guard's (`sourceSentences`), so every case here is phrased against that
// function: a sentence the guard would never let a reply quote is never shown
// as an excerpt either. Kept apart from lookup-guard.test.mjs, which is
// driven entirely by the vendored fixture set and its manifest.
import { test } from 'node:test';
import assert from 'node:assert';
import {
  EXCERPT_OMISSION, excerptDisplay, sourceSentences,
} from './lookup-guard.js';

const src = (n, chunkId, sectionPath, text) => ({ n, chunkId, docTitle: 'Paracetamol', sectionPath, text });

test('a source shows its eligible sentences verbatim, as one block', () => {
  const text = 'Paracetamol is a common painkiller. It is used to treat aches and pains.';
  const r = excerptDisplay([src(1, 10, 'About paracetamol', text)]);
  assert.deepStrictEqual(r, {
    blocks: [{ n: 1, sentences: ['Paracetamol is a common painkiller.', 'It is used to treat aches and pains.'], text }],
    shown: 2,
  });
});

test('blocks follow page order (chunkId), not citation order', () => {
  const r = excerptDisplay([
    src(1, 12, 'Side effects', 'Side effects are rare.'),
    src(2, 11, 'About', 'Paracetamol is a painkiller.'),
  ]);
  assert.deepStrictEqual(r.blocks.map((b) => b.n), [2, 1]);
});

test('page order falls back to n when any source lacks a chunkId', () => {
  // By chunkId alone this would be [2, 1]; one source without a chunkId
  // makes page order the citation order, exactly as the guard's order clause.
  const r = excerptDisplay([
    src(1, 9, 'About', 'First.'),
    src(2, 3, 'About', 'Second.'),
    src(3, undefined, 'About', 'Third.'),
  ]);
  assert.deepStrictEqual(r.blocks.map((b) => b.n), [1, 2, 3]);
});

test('a source in a withheld (overdose) section shows nothing, not even a block', () => {
  const r = excerptDisplay([
    src(1, 11, 'How and when to take it', 'Take it with water.'),
    src(2, 12, 'If you take too much', 'Paracetamol is a painkiller. Speak to a pharmacist.'),
  ]);
  assert.deepStrictEqual(r.blocks.map((b) => b.n), [1]);
  assert.strictEqual(r.shown, 1);
  assert.ok(!r.blocks.some((b) => b.text.includes('pharmacist')));
});

test('an overdose-narrative sentence is dropped, with the neighbours the guard also excludes, and the gap is marked', () => {
  const text = 'Paracetamol is a painkiller. It comes as tablets. Taking too much can cause liver damage. '
    + 'Keep it somewhere safe. It is sold in shops. Children can take it too.';
  const eligible = sourceSentences(text, 'About').map((s) => s.eligible);
  // the guard's own verdict on this source: the overdose sentence and both neighbours out
  assert.deepStrictEqual(eligible, [true, false, false, false, true, true]);
  const r = excerptDisplay([src(1, 1, 'About', text)]);
  assert.deepStrictEqual(r.blocks[0].sentences, ['Paracetamol is a painkiller.', 'It is sold in shops.', 'Children can take it too.']);
  assert.strictEqual(r.blocks[0].text,
    `Paracetamol is a painkiller. ${EXCERPT_OMISSION} It is sold in shops. Children can take it too.`);
  assert.ok(!r.blocks[0].text.includes('liver'));
  assert.strictEqual(r.shown, 3);
});

test('every shown sentence is one the guard marks eligible: never a second rule', () => {
  const text = 'Adults: take 1 or 2 tablets. Do not take more than 8 tablets in 24 hours. '
    + 'See [1] for details. Swallow it whole.';
  const eligible = new Set(sourceSentences(text, 'Dosage').filter((s) => s.eligible).map((s) => s.text));
  const r = excerptDisplay([src(1, 1, 'Dosage', text)]);
  for (const s of r.blocks.flatMap((b) => b.sentences)) assert.ok(eligible.has(s), s);
  assert.ok(!r.blocks.some((b) => /more than|\[1\]/.test(b.text)));
});

test('a list item bound to an overdose lead-in is dropped with its lead-in', () => {
  const text = 'Paracetamol is a painkiller.\n\nGo to A&E if you have taken too much:\n- you feel sick\n- you are sleepy\n\nIt is sold in shops.';
  const r = excerptDisplay([src(1, 1, 'About', text)]);
  const shown = r.blocks.flatMap((b) => b.sentences);
  assert.ok(!shown.some((s) => /feel sick|sleepy|A&E/.test(s)), JSON.stringify(shown));
  assert.ok(shown.includes('It is sold in shops.'));
});

test('a list item whose lead-in is not shown is not shown either (the lead-in and its items are one unit)', () => {
  // The lead-in holds a bracketed digit, so the guard makes it ineligible;
  // its items stay eligible to the guard but must not stand without it.
  const text = 'Paracetamol is a painkiller.\n\nFor children [see 2] you can give:\n- syrup\n- melts\n\nIt is sold in shops.';
  const sents = sourceSentences(text, 'About');
  assert.deepStrictEqual(sents.map((s) => s.eligible), [true, false, true, true, true]);
  const r = excerptDisplay([src(1, 1, 'About', text)]);
  assert.deepStrictEqual(r.blocks[0].sentences, ['Paracetamol is a painkiller.', 'It is sold in shops.']);
});

test('a list with its lead-in shown keeps its line structure', () => {
  const text = 'You can take it for:\n- headache\n- toothache';
  const r = excerptDisplay([src(1, 1, 'About', text)]);
  assert.strictEqual(r.blocks[0].text, text);
  assert.strictEqual(r.shown, 3);
});

test('a source whose text was not recovered shows nothing; no source at all shows nothing', () => {
  assert.deepStrictEqual(excerptDisplay([src(1, 1, 'About', '')]), { blocks: [], shown: 0 });
  assert.deepStrictEqual(excerptDisplay([]), { blocks: [], shown: 0 });
  assert.deepStrictEqual(excerptDisplay(undefined), { blocks: [], shown: 0 });
});
