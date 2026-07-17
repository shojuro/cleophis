// Streams a candidate GGUF into models-lab/. Run with WSL node from tools/:
//   node fetch-model.mjs llama   |   node fetch-model.mjs phi
// Optional: HF_TOKEN env var for gated/rate-limited repos.
import { createWriteStream, mkdirSync, statSync } from 'node:fs';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const CANDIDATES = {
  llama: {
    file: 'Llama-3.2-3B-Instruct-Q4_K_M.gguf',
    urls: [
      'https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf',
      'https://huggingface.co/unsloth/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf',
    ],
  },
  phi: {
    file: 'Phi-3.5-mini-instruct-Q4_K_M.gguf',
    urls: [
      'https://huggingface.co/bartowski/Phi-3.5-mini-instruct-GGUF/resolve/main/Phi-3.5-mini-instruct-Q4_K_M.gguf',
      'https://huggingface.co/MaziyarPanahi/Phi-3.5-mini-instruct-GGUF/resolve/main/Phi-3.5-mini-instruct.Q4_K_M.gguf',
    ],
  },
};

const key = process.argv[2];
const cand = CANDIDATES[key];
if (!cand) { console.error('usage: node fetch-model.mjs llama|phi'); process.exit(1); }
mkdirSync('../models-lab', { recursive: true });
const dest = `../models-lab/${cand.file}`;
const headers = process.env.HF_TOKEN ? { Authorization: `Bearer ${process.env.HF_TOKEN}` } : {};

let ok = false;
for (const url of cand.urls) {
  console.log('trying', url);
  const res = await fetch(url, { headers });
  if (!res.ok) { console.log('  ->', res.status, res.statusText); continue; }
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
  ok = true;
  break;
}
if (!ok) { console.error('all mirrors failed — set HF_TOKEN or add a mirror URL'); process.exit(1); }
console.log('wrote', dest, `${(statSync(dest).size / 1e9).toFixed(2)} GB`);
