// src/prompt-assembly.test.mjs — node --test src/
//
// On device, a supervised entry must see EXACTLY the context it was gated
// under: the catalog systemPrompt as the only system content (fingerprint
// pinned), no "no sources attached" note, no greeting as an assistant turn,
// no calc preamble (Rust side, Task 9). Anything else is a different gate
// wearing the same name.
import { test } from 'node:test';
import assert from 'node:assert';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import {
  LOOKUP_N_CTX, LOOKUP_REPLY_TOKENS, LookupBudgetError, assembleLookupMessages, assembleMessages, lookupBudget,
} from './prompt-assembly.js';
import { estTokens } from './context-window.js';

const promptFingerprint = (s) => createHash('sha256').update(String(s ?? '')).digest('hex').slice(0, 12);
const NOTE = ' No documents are attached to this conversation, so you have no sources to cite.';

test('a tutor entry is assembled exactly as before: note appended, greeting as an assistant turn', () => {
  const entry = { supervised: false, systemPrompt: 'You are a tutor.', greeting: 'Hi!' };
  const { system, messages } = assembleMessages({ entry, groundedPrompt: null, sent: [{ role: 'user', content: 'q' }], ungroundedNote: NOTE });
  assert.strictEqual(system, 'You are a tutor.' + NOTE);
  assert.deepStrictEqual(messages.map((m) => m.role), ['system', 'assistant', 'user']);
});

test('a grounded tutor turn uses the grounded prompt', () => {
  const entry = { supervised: false, systemPrompt: 'You are a tutor.', greeting: 'Hi!' };
  const { system } = assembleMessages({ entry, groundedPrompt: 'GROUNDED', sent: [], ungroundedNote: NOTE });
  assert.strictEqual(system, 'GROUNDED');
});

test('a supervised entry sends the pinned prompt verbatim and no greeting turn', () => {
  const entry = { supervised: true, systemPrompt: 'You are a triage assistant.', greeting: 'Hello', promptFingerprint: promptFingerprint('You are a triage assistant.') };
  const { system, messages } = assembleMessages({ entry, groundedPrompt: 'GROUNDED', sent: [{ role: 'user', content: 'q' }], ungroundedNote: NOTE });
  assert.strictEqual(system, 'You are a triage assistant.');
  assert.deepStrictEqual(messages.map((m) => m.role), ['system', 'user']);
});

test('a supervised entry whose prompt does not match its own fingerprint throws', () => {
  const entry = { supervised: true, systemPrompt: 'edited', greeting: '', promptFingerprint: 'deadbeefcafe' };
  assert.throws(() => assembleMessages({ entry, groundedPrompt: null, sent: [], ungroundedNote: NOTE, fingerprint: promptFingerprint }), /fingerprint/);
});

test('PARITY: the triage catalog entry\'s systemPrompt matches its pinned fingerprint', () => {
  const catalog = JSON.parse(readFileSync(new URL('../src-tauri/resources/catalog.triage.json', import.meta.url), 'utf8'));
  const entry = catalog.find((e) => e.id === 'med-triage');
  assert.ok(entry, 'catalog.triage.json has a med-triage entry (Task 9)');
  assert.strictEqual(promptFingerprint(entry.systemPrompt), entry.promptFingerprint);
});

test('a supervised entry that pins NO fingerprint is a loud error, never a silent send', () => {
  // A prompt nobody can check is not the prompt the gate was run under. It
  // throws in the same shape as a mismatch, so the send path surfaces both the
  // same way — see `isPromptMismatch` in prompt-fingerprint.js.
  const entry = { supervised: true, systemPrompt: 'You are a triage assistant.', greeting: '' };
  assert.throws(
    () => assembleMessages({ entry, groundedPrompt: null, sent: [], ungroundedNote: NOTE, fingerprint: promptFingerprint }),
    /prompt fingerprint missing/,
  );
});

test('history reaches the model as {role, content} and nothing else, on both paths', () => {
  // `sent` is a window over the app's own message objects. They carry the row
  // id, replayed citations/calculations and — on a supervised turn — the whole
  // guard verdict including `rawReply`. None of that is prompt content.
  const history = [
    { role: 'user', content: 'q' },
    { role: 'assistant', content: 'a', id: 9, citations: [{ docTitle: 'd' }], calculations: [{ expression: '1+1' }], guard: { rawReply: 'a, within 2 days', detectorsSha: 'abc' } },
  ];
  for (const entry of [
    { supervised: false, systemPrompt: 'tutor', greeting: 'Hi!' },
    { supervised: true, systemPrompt: 'triage', greeting: 'Hi!' },
  ]) {
    const { messages } = assembleMessages({ entry, groundedPrompt: null, sent: history, ungroundedNote: NOTE });
    for (const m of messages) assert.deepStrictEqual(Object.keys(m), ['role', 'content'], `${entry.supervised}`);
    assert.strictEqual(JSON.stringify(messages).includes('rawReply'), false);
    assert.strictEqual(JSON.stringify(messages).includes('docTitle'), false);
  }
  // ...and the caller's own array is untouched.
  assert.strictEqual(history[1].id, 9);
});

/* ---------------- Phase 1h M6: the lookup mode ---------------- */

const GROUNDED = 'You answer strictly from the numbered sources.\n\nThese sources are excerpts from: Paracetamol.\n\n[1] (Paracetamol, Dosage): Take 500mg.';

test('lookup: the Rust-assembled grounded prompt is the system turn and the query the ONLY user turn', () => {
  const { system, messages } = assembleLookupMessages({ groundedPrompt: GROUNDED, query: 'paracetamol' });
  assert.strictEqual(system, GROUNDED);
  assert.deepStrictEqual(messages, [
    { role: 'system', content: GROUNDED },
    { role: 'user', content: 'paracetamol' },
  ]);
});

test('lookup: no history, no greeting, no catalog systemPrompt can reach the lookup assembly', () => {
  // It takes no entry and no history at all: the arguments are the whole input.
  const out = assembleLookupMessages({
    groundedPrompt: GROUNDED, query: 'q',
    entry: { systemPrompt: 'TRIAGE PROMPT', greeting: 'Hi' },
    sent: [{ role: 'user', content: 'earlier triage turn' }],
  });
  const wire = JSON.stringify(out.messages);
  assert.ok(!wire.includes('TRIAGE PROMPT') && !wire.includes('earlier triage turn') && !wire.includes('Hi'));
  assert.strictEqual(out.messages.length, 2);
});

test('lookup: a missing grounded prompt is refused, never replaced by the triage prompt', () => {
  for (const groundedPrompt of [null, undefined, '', '   ']) {
    assert.throws(() => assembleLookupMessages({ groundedPrompt, query: 'q' }), /grounded prompt/);
  }
});

test('lookup budget: system + user + 320 <= 2048, by the app\'s own estimator', () => {
  assert.strictEqual(LOOKUP_N_CTX, 2048);
  assert.strictEqual(LOOKUP_REPLY_TOKENS, 320);
  const b = lookupBudget({ system: GROUNDED, query: 'paracetamol' });
  assert.strictEqual(b.tokens, estTokens(GROUNDED) + estTokens('paracetamol') + 320);
  assert.strictEqual(b.ok, true);
  const { tokens } = assembleLookupMessages({ groundedPrompt: GROUNDED, query: 'paracetamol' });
  assert.ok(tokens <= 2048);
});

test('lookup budget: exactly at the limit passes, one token over is a hard error', () => {
  // estTokens(s) = ceil(len / 3.5) + 4. Query 'q' costs 5. So the system may
  // cost 2048 - 320 - 5 = 1723 tokens: ceil(len/3.5) = 1719 -> len 6016 fits,
  // len 6017 (ceil = 1720) is one over.
  const atLimit = 'x'.repeat(6016);
  assert.strictEqual(lookupBudget({ system: atLimit, query: 'q' }).tokens, 2048);
  assert.doesNotThrow(() => assembleLookupMessages({ groundedPrompt: atLimit, query: 'q' }));
  const over = 'x'.repeat(6017);
  assert.strictEqual(lookupBudget({ system: over, query: 'q' }).ok, false);
  assert.throws(() => assembleLookupMessages({ groundedPrompt: over, query: 'q' }), (e) => {
    assert.ok(e instanceof LookupBudgetError);
    assert.strictEqual(e.tokens, 2049);
    return true;
  });
});

test('triage mode is untouched: the fingerprint throw still guards the supervised prompt', () => {
  const entry = { supervised: true, systemPrompt: 'drifted', promptFingerprint: 'deadbeefdead', greeting: 'g' };
  assert.throws(() => assembleMessages({ entry, sent: [], fingerprint: promptFingerprint }), /prompt fingerprint mismatch/);
});

test('lookup budget: the reply tokens are a parameter, defaulting to 320', () => {
  assert.strictEqual(lookupBudget({ system: 'x', query: 'q', replyTokens: 1000 }).tokens, estTokens('x') + estTokens('q') + 1000);
  const atLimit = 'x'.repeat(6016);
  assert.throws(() => assembleLookupMessages({ groundedPrompt: atLimit, query: 'q', replyTokens: 321 }), LookupBudgetError);
});
