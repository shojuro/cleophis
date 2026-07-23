// Usage: node tools/spike-calc.mjs <port>
// Requires a running llama-server (hero + adapters + --jinja).
import { CALC_TOOL } from '../src/calc-tool.js';

const port = process.argv[2] || '8080';
const url = `http://127.0.0.1:${port}/v1/chat/completions`;

async function ask(messages) {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ messages, tools: [CALC_TOOL], stream: false, temperature: 0.2 }),
  });
  const j = await r.json();
  return j.choices?.[0]?.message ?? {};
}

const probe1 = await ask([{ role: 'user', content: 'What is 3/4 of 88? Give the number.' }]);
const emitted = Array.isArray(probe1.tool_calls) && probe1.tool_calls.some(t => t.function?.name === 'calc');
console.log('PROBE 1 emission — tool_call to calc:', emitted ? 'YES ✅' : 'NO ❌', JSON.stringify(probe1.tool_calls ?? probe1.content));

const probe2 = await ask([{ role: 'user', content: 'Quick check: 5 + 5 = 9, right?' }]);
const p2Tool = Array.isArray(probe2.tool_calls) && probe2.tool_calls.length > 0;
console.log('PROBE 2 pushback — grounded via calc (welcome) or plain correction:',
  p2Tool ? 'CALLED calc ✅' : `(no tool) content=${probe2.content}`);
console.log('\nVERDICT: PROBE 1 = the gate. GREEN if YES. If NO, escalate per plan Task 5 note.');

// Exit non-zero if the gate (Probe 1 emission) is RED, so this doubles as a
// repeatable regression check, not just a console probe.
if (!emitted) { console.error('SPIKE RED: model did not emit a calc tool call'); process.exit(1); }
process.exit(0);
