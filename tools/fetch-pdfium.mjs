// Fetches the prebuilt pdfium dynamic library (Windows x64, non-V8/non-XFA
// — text extraction only, no JS/forms engine) and extracts
// `bin/pdfium.dll` into src-tauri/resources/pdfium/. Run with node from
// tools/ (or via pwsh if node is Windows-side):
//   node fetch-pdfium.mjs
//
// Source (§3b B1, confirmed reachable as of this task — 200, tarball
// 3733154 bytes): bblanchon/pdfium-binaries GitHub Release tag
// `chromium/7881`, asset `pdfium-win-x64.tgz`. That tag is PINNED
// deliberately: it's the exact pdfium build number `crates/kpack-pdf`'s
// `pdfium-render = "=0.9.3"` dependency targets via its `pdfium_7881`
// Cargo feature (see kpack-pdf/Cargo.toml's comment) — pdfium-render's FFI
// bindings are generated per Chromium build, so the fetched .dll and the
// crate's compiled-in bindings must always name the same build number.
// Bumping either one is a deliberate, re-verified event, not independent
// drift.
//
// `pdfium-win-x64.tgz` is the NON-V8 build (no `-v8-` in the asset name —
// bblanchon publishes a separate `pdfium-v8-win-x64.tgz` for the
// JS/XFA-enabled variant). §3b only needs text extraction, so the smaller,
// dependency-free non-V8 build is the right one — using the V8 build would
// roughly 3x the shipped size for a capability this app doesn't use.
//
// EXPECTED_SHA256 is the digest recorded (measured directly, not assumed)
// the first time this script was run against the pinned release. A
// mismatch on a later run means the upstream asset changed under the same
// tag — worth a human look, but not fatal, so this only warns, not exits 1
// (mirrors tools/fetch-embedder.mjs's convention).
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';

const RELEASE_TAG = 'chromium/7881';
const ASSET = 'pdfium-win-x64.tgz';
const URL = `https://github.com/bblanchon/pdfium-binaries/releases/download/${RELEASE_TAG}/${ASSET}`;
const TAR_ENTRY = 'bin/pdfium.dll';
const EXPECTED_SHA256 = '79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118';

const destDir = '../src-tauri/resources/pdfium';
mkdirSync(destDir, { recursive: true });
const dest = `${destDir}/pdfium.dll`;

console.log('fetching', URL);
const res = await fetch(URL);
if (!res.ok) {
  console.error('download failed:', res.status, res.statusText);
  process.exit(1);
}
const tgz = Buffer.from(await res.arrayBuffer());
console.log('downloaded', ASSET, `${(tgz.length / 1e6).toFixed(2)} MB compressed`);

const tar = gunzipSync(tgz);
const dll = extractTarEntry(tar, TAR_ENTRY);
if (!dll) {
  console.error(`entry "${TAR_ENTRY}" not found in ${ASSET} — release layout may have changed`);
  process.exit(1);
}

writeFileSync(dest, dll);
console.log('wrote', dest, `${dll.length} bytes uncompressed (${(dll.length / 1e6).toFixed(2)} MB)`);

const digest = createHash('sha256').update(dll).digest('hex');
console.log('sha256', digest);
if (digest !== EXPECTED_SHA256) {
  console.warn(`warning: sha256 differs from the recorded EXPECTED_SHA256 (${EXPECTED_SHA256}) — upstream may have changed, verify before trusting this file`);
}

// Minimal ustar reader: this tarball has no long-name (pax/GNU) entries —
// bblanchon's release archives use short, flat paths — so plain 512-byte
// header blocks are all that's needed. Reads sequential
// {header, data-padded-to-512} records until the name matches or the
// buffer runs out; returns undefined if `name` is never found.
function extractTarEntry(buf, name) {
  let offset = 0;
  while (offset + 512 <= buf.length) {
    const header = buf.subarray(offset, offset + 512);
    // Two consecutive zero-filled blocks mark end-of-archive.
    if (header.every((b) => b === 0)) break;

    const entryName = header.subarray(0, 100).toString('utf8').replace(/\0.*$/s, '');
    const sizeOctal = header.subarray(124, 136).toString('utf8').replace(/\0.*$/s, '').trim();
    const size = parseInt(sizeOctal, 8) || 0;
    const dataStart = offset + 512;

    if (entryName === name) {
      return buf.subarray(dataStart, dataStart + size);
    }

    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  return undefined;
}
