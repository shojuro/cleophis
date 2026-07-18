// Streams the bge-base-en-v1.5 Q8_0 GGUF embedder into
// src-tauri/resources/embedders/. Run with WSL node from tools/:
//   node fetch-embedder.mjs
// Optional: HF_TOKEN env var for gated/rate-limited repos.
//
// Source (K4b, confirmed reachable via HTTP HEAD as of this task — 200,
// content-length 117974304 bytes): CompendiumLabs/bge-base-en-v1.5-gguf,
// mirrored by ChristianAzinn/bge-base-en-v1.5-gguf as a fallback (same
// bytes, different remote filename — this script always writes the local
// name below regardless of which mirror served it). Note: despite the
// ~60MB estimate in the original task brief, the real bge-base-en-v1.5
// Q8_0 GGUF is ~118MB (bge-base is a 109M-param model; Q8_0 is ~1
// byte/weight plus per-block scale overhead) — recording the real number
// here since the estimate was off.
//
// EXPECTED_SHA256 is the digest recorded the first time this script was
// run against the primary mirror (K4b). A mismatch on a later run means
// the upstream file changed or a fallback mirror served different bytes —
// worth a human look, but not fatal (HF repos do occasionally get
// re-uploaded with e.g. metadata fixes), so this only warns, not exits 1.
import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream, mkdirSync, statSync } from 'node:fs';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const FILE = 'bge-base-en-v1.5-q8_0.gguf';
const URLS = [
  'https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5-q8_0.gguf',
  'https://huggingface.co/ChristianAzinn/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5.Q8_0.gguf',
];
const EXPECTED_SHA256 = 'ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c';

mkdirSync('../src-tauri/resources/embedders', { recursive: true });
const dest = `../src-tauri/resources/embedders/${FILE}`;
const headers = process.env.HF_TOKEN ? { Authorization: `Bearer ${process.env.HF_TOKEN}` } : {};

let ok = false;
for (const url of URLS) {
  console.log('trying', url);
  const res = await fetch(url, { headers });
  if (!res.ok) { console.log('  ->', res.status, res.statusText); continue; }
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
  ok = true;
  break;
}
if (!ok) { console.error('all mirrors failed — set HF_TOKEN or add a mirror URL'); process.exit(1); }
console.log('wrote', dest, `${(statSync(dest).size / 1e6).toFixed(1)} MB`);

const hash = createHash('sha256');
await pipeline(createReadStream(dest), hash);
const digest = hash.digest('hex');
console.log('sha256', digest);
if (digest !== EXPECTED_SHA256) {
  console.warn(`warning: sha256 differs from the recorded EXPECTED_SHA256 (${EXPECTED_SHA256}) — upstream may have changed, verify before trusting this file`);
}
