import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  READING_TEXT, TAKE_PHOTO_LABEL, CHOOSE_IMAGE_LABEL, USE_LABEL, USE_ANYWAY_LABEL, RETAKE_LABEL, TYPE_IT_LABEL,
  HANDOFF_PREFIX, REFUSE_NO_MATHS, REFUSE_LOW_CONFIDENCE, REVIEW_NOTE, WARNING_COPY, CONFIDENCE_FLAG,
  confirmState, applyEdit, composeHandoff, attachmentFor,
} from './vision-confirm.js';

const MODEL = { id: 'granite-docling-258M', family: 'docling' };
const TIMINGS = { prepMs: 1, prefillMs: 2, decodeMs: 3 };
const result = (over = {}) => ({
  blocks: [{ kind: 'latex', content: '3x-7=2x+5', confidence: 0.99, minOperatorProb: 0.995 }],
  warnings: [],
  verdict: 'accept',
  model: MODEL,
  timings: TIMINGS,
  imageTokens: 64,
  peakRssKb: 1,
  coresident: false,
  ...over,
});

test('copy constants match the brief verbatim', () => {
  assert.equal(READING_TEXT, 'Reading the image…');
  assert.equal(TAKE_PHOTO_LABEL, 'Take a photo');
  assert.equal(CHOOSE_IMAGE_LABEL, 'Choose an image');
  assert.equal(USE_LABEL, 'Use this');
  assert.equal(USE_ANYWAY_LABEL, 'Use anyway');
  assert.equal(RETAKE_LABEL, 'Retake');
  assert.equal(TYPE_IT_LABEL, 'Type it instead');
  assert.equal(HANDOFF_PREFIX, 'Problem from photo:');
  assert.equal(REFUSE_NO_MATHS, 'I could not find any maths in this image. Try a closer photo of the problem, or type it.');
  assert.equal(REFUSE_LOW_CONFIDENCE, 'I could not read this clearly enough to be sure. Try a closer, straighter photo, or type it.');
  assert.equal(REVIEW_NOTE, 'Check the parts marked in amber before you continue.');
  assert.deepEqual(WARNING_COPY, {
    lowConfidence: 'Some symbols were hard to read.',
    operatorUncertain: 'A sign or operator may be wrong.',
    noMathFound: 'No maths was found.',
    invalidLatex: 'A bracket is unbalanced.',
    truncated: 'The reading was cut short.',
  });
  assert.equal(CONFIDENCE_FLAG, 0.9);
});

test('accept: one latex block can be used and hands off verbatim', () => {
  const s = confirmState(result());
  assert.equal(s.mode, 'accept');
  assert.equal(s.canUse, true);
  assert.equal(s.canUseAnyway, false);
  assert.equal(s.refuseText, null);
  assert.deepEqual(s.notes, []);
  assert.equal(s.blocks[0].id, 0);
  assert.equal(s.blocks[0].flagged, false);
  assert.equal(s.blocks[0].edited, false);
  assert.equal(composeHandoff(s), 'Problem from photo:\n3x-7=2x+5');
});

test('a block is flagged below the confidence threshold or with a weak operator', () => {
  const s = confirmState(result({
    blocks: [
      { kind: 'latex', content: 'a', confidence: 0.89 },
      { kind: 'latex', content: 'b', confidence: 0.9 },
      { kind: 'latex', content: 'c', confidence: 0.99, minOperatorProb: 0.96 },
      { kind: 'latex', content: 'd', confidence: 0.99, minOperatorProb: 0.97 },
      { kind: 'latex', content: 'e', confidence: 0.99 },
    ],
  }));
  assert.deepEqual(s.blocks.map((b) => b.flagged), [true, false, true, false, false]);
  assert.deepEqual(s.blocks.map((b) => b.id), [0, 1, 2, 3, 4]);
});

test('review: operatorUncertain blocks Use, allows Use anyway, flags the block, and explains', () => {
  const s = confirmState(result({
    verdict: 'review',
    warnings: ['operatorUncertain'],
    blocks: [{ kind: 'latex', content: '3x-7=2x+5', confidence: 0.95, minOperatorProb: 0.6 }],
  }));
  assert.equal(s.mode, 'review');
  assert.equal(s.canUse, false);
  assert.equal(s.canUseAnyway, true);
  assert.equal(s.blocks[0].flagged, true);
  assert.ok(s.notes.includes(WARNING_COPY.operatorUncertain));
  assert.equal(s.notes[s.notes.length - 1], REVIEW_NOTE);
});

test('notes follow wire order, dedupe, ignore unknown warnings, and add REVIEW_NOTE only in review', () => {
  const review = confirmState(result({ verdict: 'review', warnings: ['truncated', 'lowConfidence', 'truncated', 'mystery'] }));
  assert.deepEqual(review.notes, [WARNING_COPY.truncated, WARNING_COPY.lowConfidence, REVIEW_NOTE]);
  const accept = confirmState(result({ warnings: ['lowConfidence'] }));
  assert.deepEqual(accept.notes, [WARNING_COPY.lowConfidence]);
});

test('editing a block in review mode unflags it and enables Use', () => {
  const s = confirmState(result({
    verdict: 'review',
    warnings: ['operatorUncertain'],
    blocks: [{ kind: 'latex', content: '3x+7=2x+5', confidence: 0.95, minOperatorProb: 0.6 }],
  }));
  const e = applyEdit(s, 0, '3x-7=2x+5');
  assert.equal(e.blocks[0].content, '3x-7=2x+5');
  assert.equal(e.blocks[0].edited, true);
  assert.equal(e.blocks[0].flagged, false);
  assert.equal(e.canUse, true);
  assert.equal(e.canUseAnyway, false);
  assert.equal(e.mode, 'review');
  assert.equal(composeHandoff(e), 'Problem from photo:\n3x-7=2x+5');
  // pure: the input state is untouched
  assert.equal(s.blocks[0].content, '3x+7=2x+5');
  assert.equal(s.blocks[0].edited, false);
  assert.equal(s.canUse, false);
});

test('applyEdit is a no-op for an unknown id', () => {
  const s = confirmState(result({ verdict: 'review' }));
  assert.deepEqual(applyEdit(s, 7, 'x'), s);
  assert.deepEqual(applyEdit(s, -1, 'x'), s);
});

test('applyEdit in accept mode marks the edit and keeps Use', () => {
  const s = confirmState(result());
  const e = applyEdit(s, 0, '3x=12');
  assert.equal(e.blocks[0].edited, true);
  assert.equal(e.canUse, true);
  assert.equal(e.canUseAnyway, false);
});

test('refuse with noMathFound uses the no-maths copy and offers neither Use nor Use anyway', () => {
  const s = confirmState(result({ verdict: 'refuse', warnings: ['noMathFound'], blocks: [] }));
  assert.equal(s.mode, 'refuse');
  assert.equal(s.refuseText, REFUSE_NO_MATHS);
  assert.equal(s.canUse, false);
  assert.equal(s.canUseAnyway, false);
});

test('refuse for any other reason uses the low-confidence copy', () => {
  const s = confirmState(result({ verdict: 'refuse', warnings: ['lowConfidence'] }));
  assert.equal(s.refuseText, REFUSE_LOW_CONFIDENCE);
});

test('applyEdit is a no-op in refuse mode, so an edit cannot launder a refusal', () => {
  const s = confirmState(result({ verdict: 'refuse', warnings: ['lowConfidence'] }));
  const e = applyEdit(s, 0, 'anything');
  assert.deepEqual(e, s);
  assert.equal(e.canUse, false);
});

test('composeHandoff uses only trimmed latex blocks, joined by newlines', () => {
  const s = confirmState(result({
    blocks: [
      { kind: 'text', content: 'Solve for x', confidence: 0.99 },
      { kind: 'latex', content: '  x+1=2  ', confidence: 0.99 },
      { kind: 'latex', content: '   ', confidence: 0.99 },
      { kind: 'latex', content: 'y=3', confidence: 0.99 },
    ],
  }));
  assert.equal(composeHandoff(s), 'Problem from photo:\nx+1=2\ny=3');
});

test('composeHandoff throws when no latex remains', () => {
  const onlyText = confirmState(result({ blocks: [{ kind: 'text', content: 'hello', confidence: 0.99 }] }));
  assert.throws(() => composeHandoff(onlyText), /No maths to hand off/);
  const blank = confirmState(result({ blocks: [{ kind: 'latex', content: ' ', confidence: 0.99 }] }));
  assert.throws(() => composeHandoff(blank), /No maths to hand off/);
  assert.throws(() => composeHandoff(confirmState(result({ blocks: [] }))), /No maths to hand off/);
});

test('attachmentFor carries counts and provenance, never block text', () => {
  const r = result({
    blocks: [
      { kind: 'latex', content: '3x-7=2x+5', confidence: 0.99 },
      { kind: 'latex', content: 'y=\\frac{1}{2}', confidence: 0.99 },
      { kind: 'text', content: 'Solve the equation below', confidence: 0.99 },
    ],
  });
  const s = confirmState(r);
  const a = attachmentFor(r, s);
  assert.deepEqual(a, { kind: 'vision', model: MODEL, verdict: 'accept', edited: false, blocks: { latex: 2, text: 1 } });
  const wire = JSON.stringify(a);
  for (const b of r.blocks) assert.ok(!wire.includes(b.content), `leaked ${b.content}`);
  assert.equal(attachmentFor(r, applyEdit(s, 0, 'z=9')).edited, true);
  assert.ok(!JSON.stringify(attachmentFor(r, applyEdit(s, 0, 'z=9'))).includes('z=9'));
});

test('attachmentFor records the verdict the model gave, even after a human edit', () => {
  const r = result({ verdict: 'review', warnings: ['lowConfidence'] });
  const a = attachmentFor(r, applyEdit(confirmState(r), 0, 'x=1'));
  assert.equal(a.verdict, 'review');
  assert.equal(a.edited, true);
});

// ---------------------------------------------------------- fail-closed round
const rv = (blocks, over = {}) => result({ verdict: 'review', warnings: ['operatorUncertain'], blocks, ...over });

test('accept with only text blocks is forced to refuse', () => {
  const s = confirmState(result({ blocks: [{ kind: 'text', content: 'hello', confidence: 0.99 }] }));
  assert.equal(s.mode, 'refuse');
  assert.equal(s.refuseText, REFUSE_NO_MATHS);
  assert.equal(s.canUse, false);
  assert.equal(s.canUseAnyway, false);
});

test('review with an empty latex block is forced to refuse', () => {
  const s = confirmState(rv([{ kind: 'latex', content: '  ', confidence: 0.5 }]));
  assert.equal(s.mode, 'refuse');
  assert.equal(s.refuseText, REFUSE_NO_MATHS);
  assert.equal(s.canUseAnyway, false);
});

test('an explicit refuse is unchanged by the no-maths rule', () => {
  const s = confirmState(result({ verdict: 'refuse', warnings: ['lowConfidence'] }));
  assert.equal(s.mode, 'refuse');
  assert.equal(s.refuseText, REFUSE_LOW_CONFIDENCE);
});

test('unknown confidence is flagged, never trusted', () => {
  const s = confirmState(result({
    blocks: [
      { kind: 'latex', content: 'a' },
      { kind: 'latex', content: 'b', confidence: NaN },
      { kind: 'latex', content: 'c', confidence: '0.95' },
      { kind: 'latex', content: 'd', confidence: 0.95, minOperatorProb: NaN },
      { kind: 'latex', content: 'e', confidence: 0.95 },
    ],
  }));
  assert.deepEqual(s.blocks.map((b) => b.flagged), [true, true, true, false, false]);
});

const flaggedBlock = (content = 'x') => ({ kind: 'latex', content, confidence: 0.5 });

test('editing a text block never unlocks review', () => {
  const s = confirmState(rv([flaggedBlock(), { kind: 'text', content: 'prose', confidence: 0.99 }]));
  const e = applyEdit(s, 1, 'other prose');
  assert.equal(e.blocks[1].edited, true);
  assert.equal(e.blocks[0].flagged, true);
  assert.equal(e.canUse, false);
  assert.equal(e.canUseAnyway, true);
});

test('a blank latex edit leaves the block amber and review locked', () => {
  const s = confirmState(rv([flaggedBlock(), flaggedBlock('y')]));
  const e = applyEdit(s, 0, '   ');
  assert.equal(e.blocks[0].edited, true);
  assert.equal(e.blocks[0].flagged, true);
  assert.equal(e.canUse, false);
  assert.equal(e.canUseAnyway, true);
});

test('a non-blank edit of the only flagged block unlocks Use and retires Use anyway', () => {
  const e = applyEdit(confirmState(rv([flaggedBlock()])), 0, 'x=1');
  assert.equal(e.canUse, true);
  assert.equal(e.canUseAnyway, false);
});

test('with two flagged latex blocks both must be edited', () => {
  const s = confirmState(rv([flaggedBlock('a'), flaggedBlock('b')]));
  const one = applyEdit(s, 0, 'a=1');
  assert.equal(one.canUse, false);
  assert.equal(one.canUseAnyway, true);
  const both = applyEdit(one, 1, 'b=2');
  assert.equal(both.canUse, true);
  assert.equal(both.canUseAnyway, false);
});

test('review with nothing flagged still needs an edit', () => {
  const s = confirmState(result({ verdict: 'review' }));
  assert.equal(s.canUse, false);
  assert.equal(s.canUseAnyway, true);
  assert.equal(applyEdit(s, 0, '3x=12').canUse, true);
});

test('blanking the only maths in accept mode withdraws Use', () => {
  const e = applyEdit(confirmState(result()), 0, '');
  assert.equal(e.canUse, false);
});

test('an unknown verdict is treated as review and kept raw', () => {
  const r = result({ verdict: 'maybe' });
  const s = confirmState(r);
  assert.equal(s.mode, 'review');
  assert.equal(s.canUse, false);
  assert.equal(s.verdict, 'maybe');
  assert.equal(attachmentFor(r, s).verdict, 'maybe');
});

test('attachmentFor tolerates a missing model', () => {
  const r = result({ model: undefined });
  assert.deepEqual(attachmentFor(r, confirmState(r)).model, { id: null, family: null });
});

test('blanking the only flagged latex block withdraws both buttons, and retyping restores Use', () => {
  const s = confirmState(rv([flaggedBlock()]));
  assert.equal(s.canUseAnyway, true);
  const blank = applyEdit(s, 0, ' ');
  assert.equal(blank.mode, 'review');
  assert.equal(blank.canUse, false);
  assert.equal(blank.canUseAnyway, false);
  const back = applyEdit(blank, 0, 'x=1');
  assert.equal(back.canUse, true);
  assert.equal(back.canUseAnyway, false);
});

test('Use anyway stays when another latex block still holds maths', () => {
  const s = confirmState(rv([flaggedBlock('a'), flaggedBlock('b')]));
  const e = applyEdit(s, 0, '');
  assert.equal(e.canUse, false);
  assert.equal(e.canUseAnyway, true);
});
