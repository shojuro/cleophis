import { test } from 'node:test';
import assert from 'node:assert/strict';

import { normalise, EQUIVALENCES } from './normalise.mjs';

test('\\dfrac and \\tfrac become \\frac', () => {
  assert.equal(normalise('\\dfrac{1}{2}'), '\\frac{1}{2}');
  assert.equal(normalise('\\tfrac{1}{2}'), '\\frac{1}{2}');
});

test('<loc_N> tokens are stripped', () => {
  assert.equal(normalise('<loc_12>x<loc_300>'), 'x');
});

test('a leading label is stripped', () => {
  assert.equal(normalise('Solve: 5x=35'), '5x=35');
  assert.equal(normalise('S o l v e for x :\n5x = 35'), '5x=35');
});

test('a label line on its own is stripped', () => {
  assert.equal(normalise('x+y=7\nSystem:\nx-y=1'), 'x+y=7x-y=1');
});

test('array wrappers, \\end (even truncated), \\left \\right \\displaystyle vanish', () => {
  assert.equal(normalise('\\begin{array}{l} x+y=7 \\\\ x-y=1 \\end{array}'), 'x+y=7\\\\x-y=1');
  assert.equal(normalise('x=1 \\end{'), 'x=1');
  assert.equal(normalise('\\displaystyle \\left(\\frac{1}{2}\\right)^x'), '(\\frac{1}{2})^x');
});

test('thin spaces, \\text{..} and \\colon', () => {
  assert.equal(normalise('a\\,b\\;c\\!d'), 'abcd');
  assert.equal(normalise('\\text{Solve} 5x'), '5x');
  assert.equal(normalise('x=1\\colon y'), 'x=1:y');
});

test('whitespace and newlines are removed', () => {
  assert.equal(normalise('3x - 7 =\n 2x + 5'), '3x-7=2x+5');
});

test('equivalences: \\le/\\leq, \\ge/\\geq, \\ne/\\neq, \\cdot/\\times, \\frac12', () => {
  assert.equal(normalise('a\\le b'), normalise('a\\leq b'));
  assert.equal(normalise('a\\ge b'), normalise('a\\geq b'));
  assert.equal(normalise('a\\ne b'), normalise('a\\neq b'));
  assert.equal(normalise('2\\cdot a'), normalise('2\\times a'));
  assert.equal(normalise('\\frac12'), '\\frac{1}{2}');
  assert.equal(normalise('\\frac34 x'), '\\frac{3}{4}x');
});

test('EQUIVALENCES is the pinned list of [RegExp, string] pairs', () => {
  assert.equal(EQUIVALENCES.length, 5);
  for (const [re, to] of EQUIVALENCES) {
    assert.ok(re instanceof RegExp);
    assert.equal(typeof to, 'string');
  }
});

test('idempotent', () => {
  const s = '\\dfrac12 x \\leq 3';
  assert.equal(normalise(normalise(s)), normalise(s));
});

test('brace canonicalisation: single-char ^/_ arguments are bare', () => {
  assert.equal(normalise('x^{2}'), normalise('x^2'));
  assert.equal(normalise('a_{1}+a_{2}'), 'a_1+a_2');
  assert.equal(normalise('2^{n+1}'), '2^{n+1}');
});

test('brace canonicalisation: \\frac12 equals \\frac{1}{2}', () => {
  assert.equal(normalise('\\frac12'), normalise('\\frac{1}{2}'));
  assert.equal(normalise('\\frac{1}{2}'), '\\frac{1}{2}');
});

test('brace canonicalisation: a non-argument grouping brace is unwrapped', () => {
  assert.equal(normalise('{3x}-7'), normalise('3x-7'));
  assert.equal(normalise('{3x}-7'), '3x-7');
});

test('brace canonicalisation: \\sqrt keeps its braces', () => {
  assert.equal(normalise('\\sqrt{x + 1} = 3'), '\\sqrt{x+1}=3');
});

test('a truncated or stray-brace input still normalises without throwing', () => {
  assert.doesNotThrow(() => normalise('\\frac{1}{2'));
  assert.doesNotThrow(() => normalise('x}+{'));
  assert.doesNotThrow(() => normalise('x\\'));
  assert.equal(normalise('x}+1'), 'x+1');
});

test('equivalence rewrites stop at a word boundary', () => {
  assert.equal(normalise('a \\cdots b'), 'a\\cdotsb');
  assert.equal(normalise('a \\leqslant b'), 'a\\leqslantb');
  assert.equal(normalise('a \\geqslant b'), 'a\\geqslantb');
  assert.equal(normalise('a \\neqq b'), 'a\\neqqb');
  assert.equal(normalise('a\\leq b'), normalise('a\\le b'));
  assert.equal(normalise('2\\cdot 3'), normalise('2\\times 3'));
  assert.equal(normalise('2\\cdot3'), '2\\times3');
});
