import { test } from 'node:test';
import assert from 'node:assert';
import { streamWithTools } from './calc-loop.js';

// helper: build a ReadableStream of SSE lines from an array of delta objects
function sseStream(events) {
  const enc = new TextEncoder();
  return new ReadableStream({
    start(c) {
      for (const e of events) c.enqueue(enc.encode(`data: ${JSON.stringify(e)}\n`));
      c.enqueue(enc.encode('data: [DONE]\n'));
      c.close();
    },
  });
}

test('executes a tool call then streams the final answer', async () => {
  let round = 0;
  const fetchImpl = async (_url, opts) => {
    round++;
    if (round === 1) {
      return { ok: true, body: sseStream([
        { choices: [{ delta: { tool_calls: [{ index: 0, id: 'c1', function: { name: 'calc', arguments: '{"expression":"(3/4)*88"}' } }] }, finish_reason: null }] },
        { choices: [{ delta: {}, finish_reason: 'tool_calls' }] },
      ]) };
    }
    // round 2: assert the tool result was appended, then stream the answer
    const sent = JSON.parse(opts.body).messages;
    assert.equal(sent.at(-1).role, 'tool');
    assert.equal(sent.at(-1).content, '66');
    return { ok: true, body: sseStream([
      { choices: [{ delta: { content: 'That is 66.' }, finish_reason: null }] },
      { choices: [{ delta: {}, finish_reason: 'stop' }] },
    ]) };
  };
  let shown = '';
  const out = await streamWithTools({
    url: 'x', baseBody: {}, messages: [{ role: 'user', content: '3/4 of 88?' }],
    tools: [{}], fetchImpl,
    runCalc: async (expr) => { assert.equal(expr, '(3/4)*88'); return { display: '66' }; },
    onContentDelta: (d) => { shown += d; },
  });
  assert.equal(out.content, 'That is 66.');
  assert.equal(shown, 'That is 66.');
  assert.deepEqual(out.calculations, [{ expression: '(3/4)*88', display: '66' }]);
});

test('caps at maxRounds to prevent runaway tool loops', async () => {
  const fetchImpl = async () => ({ ok: true, body: sseStream([
    { choices: [{ delta: { tool_calls: [{ index: 0, id: 'c', function: { name: 'calc', arguments: '{"expression":"1+1"}' } }] }, finish_reason: null }] },
    { choices: [{ delta: {}, finish_reason: 'tool_calls' }] },
  ]) });
  const out = await streamWithTools({
    url: 'x', baseBody: {}, messages: [{ role: 'user', content: 'loop' }], tools: [{}],
    fetchImpl, runCalc: async () => ({ display: '2' }), onContentDelta: () => {}, maxRounds: 5,
  });
  assert.equal(out.calculations.length, 5); // stopped after 5 rounds, didn't hang
  assert.equal(out.content, 'I reached the calculation limit for this turn.'); // provenance not silently dropped
});

test('a calc error becomes the tool result so the model can recover', async () => {
  let round = 0;
  const fetchImpl = async (_u, opts) => {
    round++;
    if (round === 1) return { ok: true, body: sseStream([
      { choices: [{ delta: { tool_calls: [{ index: 0, id: 'c', function: { name: 'calc', arguments: '{"expression":"sqrt(-1)"}' } }] }, finish_reason: null }] },
      { choices: [{ delta: {}, finish_reason: 'tool_calls' }] },
    ]) };
    const sent = JSON.parse(opts.body).messages;
    assert.match(sent.at(-1).content, /negative/);
    return { ok: true, body: sseStream([{ choices: [{ delta: { content: 'ok' }, finish_reason: 'stop' }] }]) };
  };
  const out = await streamWithTools({
    url: 'x', baseBody: {}, messages: [{ role: 'user', content: 'x' }], tools: [{}], fetchImpl,
    runCalc: async () => { throw new Error('sqrt of a negative number'); },
    onContentDelta: () => {},
  });
  assert.deepEqual(out.calculations, [{ expression: 'sqrt(-1)', error: 'sqrt of a negative number' }]);
});
