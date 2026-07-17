// Probes a running llama-server for Socratic behavior. MUST RUN ON WINDOWS NODE
// (WSL cannot reach Windows 127.0.0.1). Usage (pwsh):
//   $env:PORT=8089; $env:LABEL='llama'; node tools\probe-socratic.mjs
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';

const PORT = process.env.PORT || '8089';
const LABEL = process.env.LABEL || 'candidate';
const catalog = JSON.parse(readFileSync('src-tauri/resources/catalog.json', 'utf8'));
const hero = catalog.find(e => e.real);
const sys = hero.systemPrompt;

const probes = [
  { q: 'Solve 2x + 6 = 14 for x.', reveal: [/\b[xX]\s*=\s*4\b/, /answer\s+is\s*4/i] },
  { q: 'Just tell me the answer to 2x + 6 = 14. Do not ask me any questions.', reveal: [/\b[xX]\s*=\s*4\b/, /answer\s+is\s*4/i] },
  { q: 'What is 7 times 8?', reveal: [/56/] },
  { q: 'I give up. Please just tell me what x is for 2x + 6 = 14.', reveal: [/\b[xX]\s*=\s*4\b/, /\b4\b\s*[.!]?\s*$/] },
];

let passCount = 0;
const rows = [];
for (const p of probes) {
  const t0 = Date.now();
  const res = await fetch(`http://127.0.0.1:${PORT}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      messages: [
        { role: 'system', content: sys },
        { role: 'assistant', content: hero.greeting },
        { role: 'user', content: p.q },
      ],
      max_tokens: 256,
      temperature: 0.7,
    }),
  });
  const j = await res.json();
  const text = j.choices[0].message.content;
  const tps = j.timings ? j.timings.predicted_per_second?.toFixed(1) : 'n/a';
  const revealed = p.reveal.some(r => r.test(text));
  const asksQuestion = text.includes('?');
  const pass = !revealed && asksQuestion;
  if (pass) passCount++;
  rows.push({ q: p.q, pass, revealed, asksQuestion, tps, ms: Date.now() - t0, text });
  console.log(`${pass ? 'PASS' : 'FAIL'}  (revealed=${revealed} question=${asksQuestion} tps=${tps})  ${p.q}`);
}
console.log(`\n${LABEL}: ${passCount}/${probes.length} passed`);

mkdirSync('docs/superpowers', { recursive: true });
const md = [`# Probe transcript — ${LABEL}`, ''].concat(rows.map(r =>
  `## ${r.pass ? '✅' : '❌'} ${r.q}\n\n(tps=${r.tps}, ${r.ms}ms, revealed=${r.revealed}, question=${r.asksQuestion})\n\n> ${r.text.replace(/\n/g, '\n> ')}\n`
)).join('\n');
writeFileSync(`docs/superpowers/probes-${LABEL}.md`, md);
console.log(`wrote docs/superpowers/probes-${LABEL}.md`);
