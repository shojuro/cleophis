// src/lookup-wiring.test.mjs — node --test src/
//
// `app.js` reads `window.__TAURI__` at module scope and cannot be imported
// here, so its lookup decisions live in lookup-turn.js and triage-turn.js and
// are tested there. This file pins the few WIRING facts in app.js that those
// tests cannot see, by reading its source: the triage turn windows over triage
// history only, and the lookup path reaches retrieval and the model only
// through `runLookupTurn` and is never passed through the triage guard.
import { test } from 'node:test';
import assert from 'node:assert';
import { readFileSync } from 'node:fs';

const APP = readFileSync(new URL('./app.js', import.meta.url), 'utf8');
const HTML = readFileSync(new URL('./index.html', import.meta.url), 'utf8');

function body(name) {
  const start = APP.indexOf(`async function ${name}(`);
  assert.ok(start !== -1, `${name} not found`);
  const next = APP.indexOf('\nfunction ', start + 1);
  const nextAsync = APP.indexOf('\nasync function ', start + 1);
  const end = Math.min(...[next, nextAsync].filter((i) => i !== -1));
  return APP.slice(start, end);
}

test('the triage turn windows over triage history, never the raw transcript', () => {
  assert.ok(APP.includes('windowMessages(triageHistory(state.chat.messages), sys, m.greeting'));
  assert.ok(!/windowMessages\(state\.chat\.messages, sys/.test(APP));
});

test('the lookup turn goes through runLookupTurn and never through the triage guard or assembly', () => {
  const lookup = body('sendLookup');
  assert.ok(lookup.includes('runLookupTurn('));
  assert.ok(lookup.includes('lookupForPersistence('));
  for (const banned of ['applyGuard(', 'assembleMessages(', 'windowMessages(', "invoke('rag_lookup'", "invoke('rag_query'"]) {
    assert.ok(!lookup.includes(banned), banned);
  }
  // rag_lookup is invoked only inside lookup-turn.js
  assert.ok(!APP.includes("'rag_lookup'"));
});

test('the lookup query row is persisted with its marker', () => {
  assert.ok(body('sendLookup').includes('guard: LOOKUP_QUERY_GUARD'));
});

test('the lookup entry exists, hidden by default, on the chat screen', () => {
  assert.match(HTML, /<div class="lookupbar" id="lookupBar" hidden>/);
  assert.match(HTML, /id="lookupBtn"[^>]*>Look up a drug or condition</);
});
