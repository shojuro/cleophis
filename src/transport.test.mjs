import { test } from 'node:test';
import assert from 'node:assert';
import { createTransport, isAndroid } from './transport.js';

// A stand-in for Tauri's `Channel`: a class whose `onmessage` the Rust side
// pushes into. The fake `invoke` below plays the part of the inference thread.
class FakeChannel {
  constructor() {
    this.onmessage = null;
  }
  emit(event, data) {
    this.onmessage?.({ event, data });
  }
}

function recordingInvoke(script = {}) {
  const calls = [];
  const invoke = async (cmd, args) => {
    calls.push({ cmd, args });
    return script[cmd] ? script[cmd](args) : undefined;
  };
  invoke.calls = calls;
  return invoke;
}

function desktop({ fetchImpl, streamImpl, invoke } = {}) {
  return createTransport({
    mobile: false,
    invoke: invoke ?? recordingInvoke(),
    Channel: FakeChannel,
    fetchImpl: fetchImpl ?? (async () => ({ ok: true, json: async () => ({}) })),
    streamImpl: streamImpl ?? (async () => ({ content: '', calculations: [] })),
  });
}

function mobile({ invoke, newRequestId } = {}) {
  return createTransport({
    mobile: true,
    invoke: invoke ?? recordingInvoke(),
    Channel: FakeChannel,
    newRequestId: newRequestId ?? (() => 'req-1'),
  });
}

/* ---------------- the property D-1 exists to guarantee ---------------- */

test('a desktop transport never invokes the mobile chat commands', async () => {
  // Rust refuses these on desktop by returning an error that names the
  // platform, but that is a backstop for a bug. The mechanism is that the
  // desktop transport has no code path that reaches them, and this is the test
  // that says so (decision D-1, tightening 2).
  const invoke = recordingInvoke();
  const t = desktop({ invoke });

  await t.streamTurn({ port: 1234, messages: [], tools: [], baseBody: {}, onDelta: () => {} });
  await t.complete({ port: 1234, messages: [], maxTokens: 24, temperature: 0.3 });

  const mobileCommands = invoke.calls.filter((c) => c.cmd.startsWith('chat_'));
  assert.deepEqual(mobileCommands, [], 'desktop transport invoked a mobile-only command');
});

test('the transport is chosen once and reports which half is live', () => {
  assert.equal(desktop().kind, 'desktop');
  assert.equal(mobile().kind, 'mobile');
});

test('platform detection keys on Android and nothing else', () => {
  assert.equal(isAndroid('Mozilla/5.0 (Linux; Android 13; SM-A226B) AppleWebKit/537.36'), true);
  assert.equal(isAndroid('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36'), false);
  // WebKitGTK on the Linux desktop says Linux but never Android.
  assert.equal(isAndroid('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36'), false);
  assert.equal(isAndroid(undefined), false);
});

/* ---------------- desktop: the sidecar calls, unchanged ---------------- */

test('desktop streams through the sidecar URL built from the engine port', async () => {
  let seen = null;
  const t = desktop({
    streamImpl: async (req) => {
      seen = req;
      return { content: 'hi', calculations: [{ expression: '1+1', display: '2' }] };
    },
  });

  const out = await t.streamTurn({
    port: 5599,
    chatId: 7,
    messages: [{ role: 'user', content: 'hello' }],
    tools: ['CALC'],
    baseBody: { max_tokens: 700, temperature: 0.7, cache_prompt: true },
    runCalc: () => {},
    onDelta: () => {},
  });

  assert.equal(seen.url, 'http://127.0.0.1:5599/v1/chat/completions');
  assert.deepEqual(seen.baseBody, { max_tokens: 700, temperature: 0.7, cache_prompt: true });
  assert.deepEqual(seen.tools, ['CALC']);
  // The engine-side loop is the JS one on desktop, so `runCalc` must arrive.
  assert.equal(typeof seen.runCalc, 'function');
  // `onDelta` is renamed to the loop's parameter name, not dropped.
  assert.equal(typeof seen.onContentDelta, 'function');
  assert.deepEqual(out, { content: 'hi', calculations: [{ expression: '1+1', display: '2' }] });
});

test('desktop completion posts a non-streaming body and returns the raw text', async () => {
  let url = null;
  let body = null;
  const t = desktop({
    fetchImpl: async (u, opts) => {
      url = u;
      body = JSON.parse(opts.body);
      return { ok: true, json: async () => ({ choices: [{ message: { content: ' A Title ' } }] }) };
    },
  });

  const raw = await t.complete({
    port: 4242,
    messages: [{ role: 'user', content: 'x' }],
    maxTokens: 24,
    temperature: 0.3,
  });

  assert.equal(url, 'http://127.0.0.1:4242/v1/chat/completions');
  assert.equal(body.stream, false);
  assert.equal(body.max_tokens, 24);
  assert.equal(body.cache_prompt, false);
  // Returned verbatim: every bit of title sanitising stays with the caller.
  assert.equal(raw, ' A Title ');
});

test('a non-ok desktop completion yields null rather than throwing', async () => {
  const t = desktop({ fetchImpl: async () => ({ ok: false }) });
  assert.equal(await t.complete({ port: 1, messages: [], maxTokens: 1, temperature: 0 }), null);
});

/* ---------------- mobile: invoke + Channel ---------------- */

test('mobile streams deltas and resolves from the done event', async () => {
  const deltas = [];
  let channel = null;
  const invoke = recordingInvoke({
    chat_stream: (args) => {
      channel = args.onEvent;
      // The inference thread's view of one turn.
      channel.emit('delta', { text: 'Hel' });
      channel.emit('delta', { text: 'lo' });
      channel.emit('done', {
        content: 'Hello',
        calculations: [{ expression: '2*3', display: '6' }],
      });
    },
  });

  const out = await mobile({ invoke }).streamTurn({
    chatId: 7,
    messages: [{ role: 'user', content: 'hi' }],
    onDelta: (d) => deltas.push(d),
  });

  assert.deepEqual(deltas, ['Hel', 'lo']);
  assert.deepEqual(out, { content: 'Hello', calculations: [{ expression: '2*3', display: '6' }] });

  // The chat id must reach Rust: it is the KV-reuse key, and without it every
  // turn opens a cold session (task 1.5).
  const stream = invoke.calls.find((c) => c.cmd === 'chat_stream');
  assert.equal(stream.args.chatId, 7);
  assert.equal(stream.args.requestId, 'req-1');
});

test('mobile surfaces an error event as a rejection', async () => {
  const invoke = recordingInvoke({
    chat_stream: (args) => args.onEvent.emit('error', { message: 'the engine is not loaded' }),
  });

  await assert.rejects(
    mobile({ invoke }).streamTurn({ chatId: null, messages: [], onDelta: () => {} }),
    /the engine is not loaded/,
  );
});

test('mobile surfaces a refused invoke as a rejection', async () => {
  // The turn failing before it began — engine gone, or a desktop build that
  // somehow got here and hit the Rust refusal.
  const invoke = recordingInvoke({
    chat_stream: () => {
      throw 'in-process chat commands are mobile-only';
    },
  });

  await assert.rejects(
    mobile({ invoke }).streamTurn({ chatId: 1, messages: [], onDelta: () => {} }),
    /mobile-only/,
  );
});

test('aborting a mobile turn cancels it by request id', async () => {
  const ac = new AbortController();
  let channel = null;
  const invoke = recordingInvoke({
    chat_stream: (args) => {
      channel = args.onEvent;
    },
  });

  const turn = mobile({ invoke }).streamTurn({
    chatId: 3,
    messages: [],
    onDelta: () => {},
    signal: ac.signal,
  });

  ac.abort();
  const cancel = invoke.calls.find((c) => c.cmd === 'chat_cancel');
  assert.ok(cancel, 'abort did not reach chat_cancel');
  assert.equal(cancel.args.requestId, 'req-1');

  // Cancel is cooperative: the turn still ends through the normal path, with
  // whatever text had streamed. It does NOT reject.
  channel.emit('done', { content: 'partial', calculations: [] });
  assert.deepEqual(await turn, { content: 'partial', calculations: [] });
});

test('a mobile completion is deliberately unkeyed', async () => {
  const invoke = recordingInvoke({ chat_complete: () => 'A Title' });
  const raw = await mobile({ invoke }).complete({
    port: 1234,
    messages: [{ role: 'user', content: 'x' }],
    maxTokens: 24,
    temperature: 0.3,
  });

  assert.equal(raw, 'A Title');
  const call = invoke.calls.find((c) => c.cmd === 'chat_complete');
  // Auto-title is not a continuation of the chat it is about; keying it would
  // evict the prefix that chat's next real turn wants (task 1.5).
  assert.equal(call.args.chatId, undefined);
});
