// The confirmation gate between "the image was read" and "the tutor sees it".
//
// A misread sign silently changes the problem, so nothing reaches the tutor
// until the user has either accepted a confident reading or looked at, and
// edited or overridden, an uncertain one. Pure state + copy; app.js renders it.

export const OPENING_TEXT = 'Opening the photo…';
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
// Plan threshold; Stage 3 calibrates it against real photos. The engine decides
// the verdict with its own copies (ACCEPT_MIN_CONFIDENCE and OPERATOR_MIN_PROB in
// kpack-engine's vision_parse.rs); a node test pins the two sides together.
export const CONFIDENCE_FLAG = 0.9;
// Stricter than the block threshold because an operator flip (+/-, =/≠) changes
// the problem while the block as a whole still reads as confident.
export const OPERATOR_FLAG = 0.97;

// Fails closed: a missing, non-numeric or NaN confidence is "unknown", and
// unknown must be amber, never green. An absent minOperatorProb means the block
// has no operators, which is fine.
const isFlagged = (b) =>
  !(typeof b.confidence === 'number' && Number.isFinite(b.confidence) && b.confidence >= CONFIDENCE_FLAG) ||
  (typeof b.minOperatorProb === 'number' && Number.isFinite(b.minOperatorProb) && b.minOperatorProb < OPERATOR_FLAG);

const hasMaths = (b) => b.kind === 'latex' && typeof b.content === 'string' && b.content.trim() !== '';

// The notes under the blocks: the warnings' copy in wire order, deduped, unknown
// names dropped, then the review note while it is true. That note asks the user
// to check what is amber, so it is there only in review and only while some
// latex block IS amber: from the first render, and after every edit.
function notesFor(mode, warnings, blocks) {
  const notes = [];
  for (const w of warnings) {
    const copy = Object.hasOwn(WARNING_COPY, w) ? WARNING_COPY[w] : null;
    if (copy && !notes.includes(copy)) notes.push(copy);
  }
  if (mode === 'review' && blocks.some((b) => b.kind === 'latex' && b.flagged)) notes.push(REVIEW_NOTE);
  return notes;
}

// What the buttons may do, derived from the blocks so an edit can never leave a
// stale answer. Review unlocks only when a human has put real maths into a latex
// block and no latex block is still amber: editing prose or blanking a block
// proves nothing about the maths.
function gate(mode, blocks) {
  const latex = blocks.filter((b) => b.kind === 'latex');
  let canUse = false;
  if (mode === 'accept') canUse = blocks.some(hasMaths);
  else if (mode === 'review') canUse = latex.some((b) => b.edited && hasMaths(b)) && !latex.some((b) => b.flagged);
  // Use anyway overrides a flag on maths that exists; with no maths left there is nothing to override.
  return { canUse, canUseAnyway: mode === 'review' && !canUse && blocks.some(hasMaths) };
}

export function confirmState(result) {
  const verdict = result.verdict;
  // An unknown verdict string must not read as a pass.
  let mode = verdict === 'refuse' || verdict === 'review' || verdict === 'accept' ? verdict : 'review';
  const warnings = Array.isArray(result.warnings) ? result.warnings : [];
  const blocks = (result.blocks || []).map((b, id) => ({
    id,
    kind: b.kind,
    content: b.content,
    confidence: b.confidence,
    // Only maths can be amber: a text block is read-only and never handed off,
    // so a flag on it would ask for a check the user cannot make and that
    // protects nothing the tutor sees.
    flagged: b.kind === 'latex' && isFlagged(b),
    edited: false,
  }));
  // An accept with amber maths means the engine and this gate disagree about a
  // threshold. Fail closed: the user checks it, as in review. `verdict` below
  // keeps what the engine said.
  if (mode === 'accept' && blocks.some((b) => b.flagged)) mode = 'review';
  // Nothing to hand the tutor, whatever the model's verdict says.
  const noMaths = !blocks.some(hasMaths);
  if (noMaths) mode = 'refuse';
  const notes = notesFor(mode, warnings, blocks);
  return {
    mode,
    blocks,
    notes,
    refuseText: mode === 'refuse' ? (noMaths || warnings.includes('noMathFound') ? REFUSE_NO_MATHS : REFUSE_LOW_CONFIDENCE) : null,
    ...gate(mode, blocks),
    verdict,
    warnings,
    model: result.model,
  };
}

export function applyEdit(state, id, text) {
  // A refusal stays a refusal: typing into it must not launder it into a handoff.
  const target = state.blocks.find((b) => b.id === id);
  if (state.mode === 'refuse' || !target) return state;
  const isMaths = target.kind === 'latex';
  const nonBlank = typeof text === 'string' && text.trim() !== '';
  const blocks = state.blocks.map((b) => {
    if (b.id !== id) return b;
    const next = { ...b, content: text, edited: true };
    // Only real maths in a latex block counts as a human having checked it.
    if (isMaths && nonBlank) next.flagged = false;
    return next;
  });
  const notes = notesFor(state.mode, Array.isArray(state.warnings) ? state.warnings : [], blocks);
  return { ...state, blocks, notes, ...gate(state.mode, blocks) };
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
// `how` is the button pressed ('use' or 'anyway'). `override` is true only for
// "Use anyway", so a correction (edited, then Use this) and an override (Use
// anyway, edited or not) stay apart for Stage 3's calibration.
export function attachmentFor(result, state, how) {
  const count = (kind) => state.blocks.filter((b) => b.kind === kind).length;
  return {
    kind: 'vision',
    model: { id: result.model?.id ?? null, family: result.model?.family ?? null },
    verdict: result.verdict,
    edited: state.blocks.some((b) => b.edited),
    override: how === 'anyway',
    blocks: { latex: count('latex'), text: count('text') },
  };
}
