// The confirmation gate between "the image was read" and "the tutor sees it".
//
// A misread sign silently changes the problem, so nothing reaches the tutor
// until the user has either accepted a confident reading or looked at, and
// edited or overridden, an uncertain one. Pure state + copy; app.js renders it.

export const READING_TEXT = 'Reading the image…';
export const TAKE_PHOTO_LABEL = 'Take a photo';
export const CHOOSE_IMAGE_LABEL = 'Choose an image';
export const USE_LABEL = 'Use this';
export const USE_ANYWAY_LABEL = 'Use anyway';
export const RETAKE_LABEL = 'Retake';
export const TYPE_IT_LABEL = 'Type it instead';
export const HANDOFF_PREFIX = 'Problem from photo:';
export const REFUSE_NO_MATHS = 'I could not find any maths in this image. Try a closer photo of the problem, or type it.';
export const REFUSE_LOW_CONFIDENCE = 'I could not read this clearly enough to be sure. Try a closer, straighter photo, or type it.';
export const REVIEW_NOTE = 'Check the parts marked in amber before you continue.';
export const WARNING_COPY = {
  lowConfidence: 'Some symbols were hard to read.',
  operatorUncertain: 'A sign or operator may be wrong.',
  noMathFound: 'No maths was found.',
  invalidLatex: 'A bracket is unbalanced.',
  truncated: 'The reading was cut short.',
};
// Plan threshold; Stage 3 calibrates it against real photos.
export const CONFIDENCE_FLAG = 0.9;
// Stricter than the block threshold because an operator flip (+/-, =/≠) changes
// the problem while the block as a whole still reads as confident.
const OPERATOR_FLAG = 0.97;

const isFlagged = (b) =>
  b.confidence < CONFIDENCE_FLAG || (typeof b.minOperatorProb === 'number' && b.minOperatorProb < OPERATOR_FLAG);

export function confirmState(result) {
  const mode = result.verdict === 'refuse' || result.verdict === 'review' ? result.verdict : 'accept';
  const warnings = Array.isArray(result.warnings) ? result.warnings : [];
  const blocks = (result.blocks || []).map((b, id) => ({
    id,
    kind: b.kind,
    content: b.content,
    confidence: b.confidence,
    flagged: isFlagged(b),
    edited: false,
  }));
  const notes = [];
  for (const w of warnings) {
    const copy = Object.hasOwn(WARNING_COPY, w) ? WARNING_COPY[w] : null;
    if (copy && !notes.includes(copy)) notes.push(copy);
  }
  if (mode === 'review') notes.push(REVIEW_NOTE);
  return {
    mode,
    blocks,
    notes,
    refuseText: mode === 'refuse' ? (warnings.includes('noMathFound') ? REFUSE_NO_MATHS : REFUSE_LOW_CONFIDENCE) : null,
    canUse: mode === 'accept',
    canUseAnyway: mode === 'review',
    verdict: result.verdict,
    warnings,
    model: result.model,
  };
}

export function applyEdit(state, id, text) {
  // A refusal stays a refusal: typing into it must not launder it into a handoff.
  if (state.mode === 'refuse' || !state.blocks.some((b) => b.id === id)) return state;
  return {
    ...state,
    blocks: state.blocks.map((b) => (b.id === id ? { ...b, content: text, edited: true, flagged: false } : b)),
    // A human has now read and changed the text, so there is nothing left to override.
    canUse: true,
    canUseAnyway: false,
  };
}

export function composeHandoff(state) {
  // Text blocks are shown for context but never handed off: only maths the
  // user confirmed goes to the tutor.
  const latex = state.blocks
    .filter((b) => b.kind === 'latex')
    .map((b) => b.content.trim())
    .filter((c) => c !== '');
  if (latex.length === 0) throw new Error('No maths to hand off');
  return `${HANDOFF_PREFIX}\n${latex.join('\n')}`;
}

// Provenance for the stored message. Counts only: the problem text may be a
// child's homework and the transcript already holds what the user confirmed.
export function attachmentFor(result, state) {
  const count = (kind) => state.blocks.filter((b) => b.kind === kind).length;
  return {
    kind: 'vision',
    model: { id: result.model.id, family: result.model.family },
    verdict: result.verdict,
    edited: state.blocks.some((b) => b.edited),
    blocks: { latex: count('latex'), text: count('text') },
  };
}
