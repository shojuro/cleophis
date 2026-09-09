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
import { assembleMessages } from './prompt-assembly.js';

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
