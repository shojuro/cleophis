// src/prompt-fingerprint.js — the app's own SHA-256, for one job.
//
// Task 5 made `assembleMessages` refuse to send a supervised prompt whose text
// does not match the catalog's `promptFingerprint`. That check needs a hash,
// and on the send path it needs a SYNCHRONOUS one:
//
//   • `node:crypto` does not exist in a WebView, and there is no bundler here
//     (`frontendDist` is `../src`, served as plain ES modules);
//   • `crypto.subtle.digest` is async, and the check sits inside
//     `assembleMessages`, which is called twice per turn and returns a value;
//     making it async would make the windowing arithmetic around it async too.
//
// So the hash is 60 lines of arithmetic here instead. That is only worth
// anything if it is the same sha256 the pin was cut with, which is why
// `prompt-fingerprint.test.mjs` fuzzes it against `node:crypto` — block
// boundaries, astral-plane UTF-8, and 400 random inputs — and then asserts
// that it reproduces the shipped catalog entry's own pinned value.
//
// Pure and dependency-free on purpose: nothing here touches the DOM, Tauri, or
// the catalog.

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

const rotr = (x, n) => ((x >>> n) | (x << (32 - n))) >>> 0;

/** SHA-256 of `text`'s UTF-8 bytes, lower-case hex. FIPS 180-4, no shortcuts. */
export function sha256Hex(text) {
  const msg = new TextEncoder().encode(String(text ?? ''));
  // One 0x80 byte, then zeros, then the length in bits as a 64-bit big-endian
  // — padded to the next whole 64-byte block that leaves room for both.
  const padded = new Uint8Array(((((msg.length + 8) >> 6) + 1) << 6));
  padded.set(msg);
  padded[msg.length] = 0x80;
  const view = new DataView(padded.buffer);
  const bits = msg.length * 8;
  view.setUint32(padded.length - 8, Math.floor(bits / 0x100000000));
  view.setUint32(padded.length - 4, bits >>> 0);

  const h = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const w = new Uint32Array(64);

  for (let off = 0; off < padded.length; off += 64) {
    for (let i = 0; i < 16; i += 1) w[i] = view.getUint32(off + i * 4);
    for (let i = 16; i < 64; i += 1) {
      const x = w[i - 15];
      const y = w[i - 2];
      const s0 = (rotr(x, 7) ^ rotr(x, 18) ^ (x >>> 3)) >>> 0;
      const s1 = (rotr(y, 17) ^ rotr(y, 19) ^ (y >>> 10)) >>> 0;
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i += 1) {
      const S1 = (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) >>> 0;
      const ch = ((e & f) ^ (~e & g)) >>> 0;
      const t1 = (hh + S1 + ch + K[i] + w[i]) >>> 0;
      const S0 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) >>> 0;
      const maj = ((a & b) ^ (a & c) ^ (b & c)) >>> 0;
      const t2 = (S0 + maj) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0;
      d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    h[0] = (h[0] + a) >>> 0; h[1] = (h[1] + b) >>> 0; h[2] = (h[2] + c) >>> 0; h[3] = (h[3] + d) >>> 0;
    h[4] = (h[4] + e) >>> 0; h[5] = (h[5] + f) >>> 0; h[6] = (h[6] + g) >>> 0; h[7] = (h[7] + hh) >>> 0;
  }

  let out = '';
  for (let i = 0; i < 8; i += 1) out += h[i].toString(16).padStart(8, '0');
  return out;
}

/**
 * The catalog's `promptFingerprint`: the first 12 hex characters of the
 * system prompt's sha256. Short because it is a drift alarm, not a signature
 * — the catalog itself is signed (Task 9 / Phase 3 Task 5).
 */
export function promptFingerprint(text) {
  return sha256Hex(text).slice(0, 12);
}

/**
 * Is this the throw `prompt-assembly.js` makes when the prompt about to be
 * sent is not one the gate can be said to have been run under?
 *
 * Two cases, one answer: the prompt does not match the pin (MISMATCH), or the
 * supervised entry pins nothing to match against (MISSING). Both are a refusal
 * to send, and the send path needs to tell either apart from a transport
 * failure — a fetch that failed is a retry chip, and a prompt that cannot be
 * vouched for is not sent at all.
 */
export function isPromptMismatch(err) {
  return /prompt fingerprint (mismatch|missing)/i.test(String(err?.message ?? err ?? ''));
}
