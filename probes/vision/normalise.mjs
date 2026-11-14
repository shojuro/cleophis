// The pinned LaTeX normalisation the vision probes score against. It ports
// Stage 0's `normalise` (docs/superpowers/vision-spike/score-spike.py) rule for
// rule and in the same order, applies EQUIVALENCES, canonicalises braces
// (Stage 0's tokenize/render), then removes all whitespace. Pure: no I/O.

// Spellings that mean the same thing; applied after the Stage 0 rules.
export const EQUIVALENCES = [
  [/\\cdot(?![A-Za-z])/g, '\\times'],
  [/\\leq(?![A-Za-z])/g, '\\le'],
  [/\\geq(?![A-Za-z])/g, '\\ge'],
  [/\\frac(\d)(\d)/g, '\\frac{$1}{$2}'],
  [/\\neq(?![A-Za-z])/g, '\\ne'],
];

// Brace canonicalisation, ported from Stage 0's tokenize / strip_ws / render.
const CMDS_ONE = new Set(['sqrt', 'text', 'mathrm', 'overline', 'hat', 'vec', 'bar']);
const CMDS_TWO = new Set(['frac', 'binom']);

// A token is a string (a \command or one character) or { group: [tokens] }.
function tokenize(s) {
  let i = 0;
  const out = [];
  while (i < s.length) {
    const c = s[i];
    if (c === '\\') {
      const m = /^\\([A-Za-z]+|\\|[^\n])/.exec(s.slice(i));
      const tok = m ? m[0] : c;
      out.push(tok);
      i += tok.length;
    } else if (c === '{') {
      let depth = 1;
      let j = i + 1;
      while (j < s.length && depth) {
        if (s[j] === '{') depth += 1;
        else if (s[j] === '}') depth -= 1;
        j += 1;
      }
      out.push({ group: tokenize(s.slice(i + 1, j - 1)) });
      i = j;
    } else if (c === '}') {
      i += 1; // stray closer (a truncated output): ignore
    } else {
      out.push(c);
      i += 1;
    }
  }
  return out;
}

function stripWs(tokens) {
  const out = [];
  for (const t of tokens) {
    if (typeof t === 'object') out.push({ group: stripWs(t.group) });
    else if (!/^\s+$/.test(t)) out.push(t);
  }
  return out;
}

function render(tokens) {
  const out = [];
  let want = 0; // argument groups the previous token still expects
  for (const t of stripWs(tokens)) {
    if (typeof t === 'object') {
      const inner = render(t.group);
      if (want) {
        const prev = out.length ? out[out.length - 1] : '';
        if ((prev === '^' || prev === '_') && inner.length === 1) out.push(inner); // x^{2} -> x^2
        else out.push('{' + inner + '}');
        want -= 1;
      } else {
        out.push(inner); // grouping braces: unwrap
      }
      continue;
    }
    if (t === '^' || t === '_') want = 1;
    else if (t.startsWith('\\') && CMDS_TWO.has(t.slice(1))) want = 2;
    else if (t.startsWith('\\') && CMDS_ONE.has(t.slice(1))) want = 1;
    else {
      const prev = out.length ? out[out.length - 1] : undefined;
      want = want && t.length === 1 && (prev === '^' || prev === '_') ? want - 1 : 0;
    }
    out.push(t);
  }
  return out.join('');
}

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
  return render(tokenize(s)).replace(/\s+/g, '');
}
