// The pinned LaTeX normalisation the vision probes score against. It ports
// Stage 0's `normalise` (docs/superpowers/vision-spike/score-spike.py) rule for
// rule and in the same order, then applies EQUIVALENCES, then removes all
// whitespace. Pure: no I/O.

// Spellings that mean the same thing; applied after the Stage 0 rules.
export const EQUIVALENCES = [
  [/\\cdot/g, '\\times'],
  [/\\leq/g, '\\le'],
  [/\\geq/g, '\\ge'],
  [/\\frac(\d)(\d)/g, '\\frac{$1}{$2}'],
  [/\\neq/g, '\\ne'],
];

export function normalise(latex) {
  let s = String(latex ?? '');
  s = s.replace(/<loc_\d+>/g, ' ');
  s = s.replace(/\\begin\{array\}\s*\{[^}]*\}/g, ' ');
  // tolerant: a token cap may cut "\end{array}" short
  s = s.replace(/\\end(\s*\{[^}]*\}?)?/g, ' ');
  s = s.replace(/\\(left|right|displaystyle)\b/g, ' ');
  s = s.replace(/\\[,;!]/g, ' ');
  s = s.replaceAll('\\dfrac', '\\frac').replaceAll('\\tfrac', '\\frac');
  s = s.replace(/\\text\s*\{[^}]*\}/g, ' ');
  s = s.replace(/\\colon/g, ':');
  // a label such as "Solve for x :" or "S o l v e :" before the maths
  s = s.trim().replace(/^[\sA-Za-z]*:\s*/, '');
  s = s.replace(/^[\sA-Za-z]*:\s*$/gm, ''); // a label line on its own
  s = s.replaceAll('\n', ' ');
  for (const [re, to] of EQUIVALENCES) s = s.replace(re, to);
  return s.replace(/\s+/g, '');
}
