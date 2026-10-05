// Tests for the context-window policy (D-4).
//
// The load-bearing test is `the same conversation windows differently at 2048
// and 4096`. It is the one that would have caught the shipped bug, in which
// the arithmetic was right and the window it was given was another platform's.

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  windowMessages,
  engineWindow,
  estTokens,
  REPLY_RESERVE,
  CTX_SAFETY,
  FALLBACK_N_CTX,
} from './context-window.js';

const msg = (role, content) => ({ role, content });
/// A message of roughly `n` tokens, given the ~3.5 chars/token estimate.
const sized = (role, n) => msg(role, 'x'.repeat(Math.max(1, (n - 4) * 3.5)));

test('a short conversation is sent whole and drops nothing', () => {
  const msgs = [msg('user', 'hello'), msg('assistant', 'hi'), msg('user', 'how are you?')];
  const { sent, droppedCount } = windowMessages(msgs, 'sys', 'greeting', 4096);
  assert.equal(droppedCount, 0);
  assert.deepEqual(sent, msgs);
});

test('the oldest turns are the ones dropped — truncate-oldest, per D-4', () => {
  const msgs = Array.from({ length: 40 }, (_, i) => sized(i % 2 ? 'assistant' : 'user', 200));
  const { sent, droppedCount } = windowMessages(msgs, 'sys', 'greeting', 2048);
  assert.ok(droppedCount > 0, 'expected this to overflow a 2048 window');
  // What survives is a contiguous SUFFIX: the newest turns, in order.
  assert.deepEqual(sent, msgs.slice(droppedCount));
  assert.equal(sent[sent.length - 1], msgs[msgs.length - 1]);
});

test('THE REGRESSION: the same conversation windows differently at 2048 and 4096', () => {
  // This is D-4 in one assertion. Before the fix the frontend always used
  // 4096, so on the floor tier it kept roughly twice the history the engine
  // could hold and the decode failed. Identical input, different windows, and
  // the smaller window MUST keep strictly less.
  const msgs = Array.from({ length: 60 }, (_, i) => sized(i % 2 ? 'assistant' : 'user', 120));
  const small = windowMessages(msgs, 'sys', 'greeting', 2048);
  const large = windowMessages(msgs, 'sys', 'greeting', 4096);
  assert.ok(
    small.sent.length < large.sent.length,
    `2048 kept ${small.sent.length} and 4096 kept ${large.sent.length} — a window that changes nothing is the bug`,
  );
  assert.ok(small.droppedCount > large.droppedCount);
});

test('what is kept actually fits the window it was given', () => {
  for (const nCtx of [2048, 4096]) {
    const msgs = Array.from({ length: 80 }, (_, i) => sized(i % 2 ? 'assistant' : 'user', 90));
    const sys = 'a system prompt of some length';
    const greeting = 'a greeting';
    const { sent } = windowMessages(msgs, sys, greeting, nCtx);
    const used = sent.reduce((n, m) => n + estTokens(m.content), 0)
      + estTokens(sys) + estTokens(greeting);
    assert.ok(
      used + REPLY_RESERVE + CTX_SAFETY <= nCtx,
      `at n_ctx=${nCtx}: kept ${used} tokens of history, which does not leave room for the reply`,
    );
  }
});

test('a larger system prompt shrinks the history budget', () => {
  // Grounded turns swap in a much bigger system prompt (contract + sources),
  // and its length has to come out of the history budget or the sources push
  // the whole request over.
  const msgs = Array.from({ length: 40 }, (_, i) => sized(i % 2 ? 'assistant' : 'user', 100));
  const small = windowMessages(msgs, 'short system', 'greeting', 4096);
  const big = windowMessages(msgs, 'x'.repeat(4000), 'greeting', 4096);
  assert.ok(big.sent.length < small.sent.length);
});

test('the final message is never dropped, even when it alone overflows', () => {
  // Dropping it would mean answering a question the user did not ask.
  const huge = msg('user', 'x'.repeat(200000));
  const { sent, droppedCount } = windowMessages([sized('user', 500), huge], 'sys', 'greeting', 2048);
  assert.equal(sent.length, 1);
  assert.equal(sent[0], huge);
  assert.equal(droppedCount, 1);
});

test('an empty conversation is handled without special-casing', () => {
  const { sent, droppedCount } = windowMessages([], 'sys', 'greeting', 2048);
  assert.deepEqual(sent, []);
  assert.equal(droppedCount, 0);
});

test('windowMessages never mutates the conversation it is given — storage is not truncated', () => {
  // D-4's other half: the request is trimmed, the transcript is not. This
  // asserts the function cannot be the thing that breaks that.
  const msgs = Array.from({ length: 50 }, (_, i) => sized('user', 200));
  const before = msgs.slice();
  windowMessages(msgs, 'sys', 'greeting', 2048);
  assert.equal(msgs.length, before.length);
  assert.deepEqual(msgs, before);
});

test('engineWindow reads the engine and falls back only when it says nothing', () => {
  assert.equal(engineWindow({ nCtx: 2048 }), 2048);
  assert.equal(engineWindow({ nCtx: 4096 }), 4096);
  // A backend predating EngineInfo.nCtx reports no window at all.
  assert.equal(engineWindow({ port: 0, status: 'Ready' }), FALLBACK_N_CTX);
  assert.equal(engineWindow(null), FALLBACK_N_CTX);
  assert.equal(engineWindow(undefined), FALLBACK_N_CTX);
});

test('engineWindow rejects nonsense rather than propagating it into the budget', () => {
  // A zero or negative window would make the budget negative and drop the
  // entire history on every turn — a silent, total memory loss.
  for (const bad of [0, -1, NaN, Infinity, '2048', {}]) {
    assert.equal(engineWindow({ nCtx: bad }), FALLBACK_N_CTX, `bad window: ${String(bad)}`);
  }
});
