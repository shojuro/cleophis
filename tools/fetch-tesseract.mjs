// Fetches a portable Windows tesseract (binary + runtime DLLs) and the
// English `eng.traineddata` (fast variant) into src-tauri/resources/ocr/,
// mirroring fetch-pdfium.mjs's download -> verify -> extract shape (see
// that file for the sibling resources/pdfium/ pipeline; the sha256-verify
// idiom below is copied from it verbatim). Run with node from tools/:
//   node fetch-tesseract.mjs
// v1 only implements win32 (this app currently only ships an MSI target);
// other platforms print "not yet supported" and exit. This must be run
// under a *Windows* node (process.platform === 'win32') — e.g. Windows
// Terminal/PowerShell node, or from WSL via the interop path to the
// Windows-side node.exe — because it shells out to a native 7z.exe to
// unpack the NSIS installer (see "Why 7-Zip" below).
//
// PREREQUISITE: a working 7-Zip install (7z.exe reachable via PATH, or at
// the standard `C:\Program Files\7-Zip\7z.exe` / `...(x86)\7-Zip\7z.exe`).
// See "Why 7-Zip" below for why this script deliberately does NOT try to
// silently auto-fetch/bootstrap one.
//
// ---------------------------------------------------------------------
// Source (investigated for this task): a portable, pinnable Windows
// tesseract build. Two real options were found; the second was rejected:
//
//   1. UB-Mannheim's official Windows installer, published as a GitHub
//      release asset on tesseract-ocr/tesseract (the UB-Mannheim wiki
//      links this exact URL as "the" current 64-bit installer — confirmed
//      by fetching https://raw.githubusercontent.com/wiki/UB-Mannheim/
//      tesseract/Home.md directly). It's an NSIS self-extracting archive,
//      not a portable zip — but NSIS's container format is decoded by
//      7-Zip's core engine (not a loadable plugin: confirmed by listing
//      the archive with 7-Zip 26.02 and seeing `Type = Nsis` handled with
//      no external Formats/*.dll present), so it extracts deterministically
//      with no installer execution, no registry/filesystem side effects,
//      and no interactive UI. CHOSEN — official upstream, single pinnable
//      asset.
//   2. conda-forge's `tesseract` win-64 package (anaconda.org). Also
//      pinnable by URL+sha256 (the API resolves e.g.
//      conda-forge/tesseract/5.5.2/win-64/tesseract-5.5.2-hfa586c3_0.conda
//      to a fixed sha256), but the win-64 package does NOT bundle its
//      runtime DLLs — it *depends* on ~13 separate conda-forge packages
//      (leptonica, libarchive, libjpeg-turbo, openjpeg, libtiff, ...),
//      each independently versioned/pinned, in a zstd-compressed `.conda`
//      container Node can't read without another dependency. Chasing 13
//      transitive package pins is strictly MORE fragile than extracting
//      one official installer once. REJECTED.
//
// EXPECTED_INSTALLER_SHA256 is the digest measured directly against the
// pinned release asset the first time this script was written. Mirrors
// fetch-pdfium.mjs: a mismatch warns (upstream asset changed under the
// same tag/name — worth a human look) but doesn't hard-fail.
//
// ---------------------------------------------------------------------
// Why 7-Zip, and why this script requires a pre-installed one rather than
// silently fetching/bootstrapping its own copy:
//
// Node has no built-in NSIS decoder (unlike gzip/tar, which fetch-pdfium.
// mjs parses by hand — NSIS's block format plus its LZMA-solid compression
// is a much bigger lift than tar+gzip, not "fast, Node + config only").
// Two auto-bootstrap paths were tried and both were rejected:
//   - ip7z/7zip's standalone console-only Windows build (`7za.exe`/
//     `7zr.exe`, from the "7z-extra" package): ships a reduced format set
//     (7z/zip/gzip/bzip2/tar only) and fails with "Cannot open the file as
//     archive" on the NSIS installer — confirmed by testing.
//   - Extracting the full `7z.exe`+`7z.dll` (the engine that DOES read
//     NSIS) out of the official 7-Zip Windows MSI via `msiexec /a`
//     (the documented extract-only "administrative install", no real
//     install/registry/UI): the MSI-as-archive part is clean, but
//     `msiexec /a /qn` reliably HUNG in testing (no log output, no
//     process progress) rather than completing — a real, reproduced
//     failure, not a hypothetical one.
// Given both auto-bootstrap paths are fragile/broken, this script instead
// requires 7-Zip to already be present (extremely common on Windows dev
// boxes; free; `winget install 7zip.7zip` if not) and fails LOUDLY with
// install instructions if it isn't, rather than silently doing something
// unreliable. This is the one new external-tool prerequisite this
// pipeline has (fetch-pdfium.mjs/fetch-embedder.mjs need none) — flagged
// here and in the task report for visibility.
//
// ---------------------------------------------------------------------
// REQUIRED_FILES is the minimal runtime dependency closure for
// tesseract.exe, computed by walking the PE import tables of tesseract.
// exe and (transitively) every DLL it imports, starting from the full
// ~140-file installer contents and excluding standard Windows system
// DLLs. This is why it's much smaller than "everything in the
// installer": libicu*/cairo/pango/harfbuzz/freetype/fontconfig/graphite2/
// fribidi/datrie/thai etc. are only needed by the training tools
// (text2image, lstmtraining, ...), which this app never runs — tesseract.
// exe's own closure doesn't touch them. Validated for real, not just by
// import-table inspection: this exact file set was extracted, dropped
// next to a fetched eng.traineddata, and both `tesseract.exe --version`
// and a real `tesseract.exe test.png out -l eng --tessdata-dir tessdata`
// OCR pass succeeded standalone (no other files present).
//
// ---------------------------------------------------------------------
// eng.traineddata (fast) comes from tesseract-ocr/tessdata_fast, pinned
// to the exact commit that added it — 923915d4ced2a7235221788285785a29c
// 4a42d4a, "Initial import to github" (2017-09-14) — which is also the
// *only* commit that has ever touched that file, so this is a genuinely
// immutable reference, not just a branch snapshot at fetch-time.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const TESSERACT_RELEASE_TAG = '5.5.0';
const TESSERACT_ASSET = 'tesseract-ocr-w64-setup-5.5.0.20241111.exe';
const TESSERACT_URL = `https://github.com/tesseract-ocr/tesseract/releases/download/${TESSERACT_RELEASE_TAG}/${TESSERACT_ASSET}`;
const EXPECTED_INSTALLER_SHA256 = 'f3fc4236425b690c8be756f35793f77394ee004be0a6460a440c754d892f68bc';

const TRAINEDDATA_COMMIT = '923915d4ced2a7235221788285785a29c4a42d4a';
const TRAINEDDATA_URL = `https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/${TRAINEDDATA_COMMIT}/eng.traineddata`;
const EXPECTED_TRAINEDDATA_SHA256 = '7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2';

// tesseract.exe's full transitive runtime DLL closure (see header comment
// for how this was derived and validated).
const REQUIRED_FILES = [
  'tesseract.exe',
  'libLerc.dll',
  'libarchive-13.dll',
  'libb2-1.dll',
  'libbrotlicommon.dll',
  'libbrotlidec.dll',
  'libbz2-1.dll',
  'libcrypto-3-x64.dll',
  'libcurl-4.dll',
  'libdeflate.dll',
  'libexpat-1.dll',
  'libgcc_s_seh-1.dll',
  'libgif-7.dll',
  'libiconv-2.dll',
  'libidn2-0.dll',
  'libintl-8.dll',
  'libjbig-0.dll',
  'libjpeg-8.dll',
  'libleptonica-6.dll',
  'liblz4.dll',
  'liblzma-5.dll',
  'libopenjp2-7.dll',
  'libpng16-16.dll',
  'libpsl-5.dll',
  'libsharpyuv-0.dll',
  'libssh2-1.dll',
  'libstdc++-6.dll',
  'libtesseract-5.dll',
  'libtiff-6.dll',
  'libunistring-5.dll',
  'libwebp-7.dll',
  'libwebpmux-3.dll',
  'libwinpthread-1.dll',
  'libzstd.dll',
  'zlib1.dll',
];

async function main() {
  if (process.platform !== 'win32') {
    console.error(`fetch-tesseract: platform "${process.platform}" not yet supported in v1 (only win32 is implemented) — run this under a Windows node.`);
    process.exit(1);
  }

  const sevenZip = find7Zip();
  if (!sevenZip) {
    console.error([
      'fetch-tesseract: 7-Zip (7z.exe) not found on PATH or in the standard install locations.',
      'This script needs it to unpack the NSIS-format tesseract installer (see the "Why 7-Zip" comment at the top of this file for why it isn\'t auto-installed).',
      'Install it (winget install 7zip.7zip, or https://www.7-zip.org/) and re-run.',
    ].join('\n'));
    process.exit(1);
  }
  console.log('using 7-Zip at', sevenZip);

  const destDir = '../src-tauri/resources/ocr';
  const tessdataDir = `${destDir}/tessdata`;
  mkdirSync(destDir, { recursive: true });
  mkdirSync(tessdataDir, { recursive: true });

  const workDir = mkdtempSync(join(tmpdir(), 'fetch-tesseract-'));
  try {
    const installerPath = join(workDir, TESSERACT_ASSET);
    // FAIL-CLOSED (stricter than the DRY sibling fetch scripts, which warn):
    // this download is an EXECUTABLE that gets spawned as a subprocess
    // (src-tauri/src/ocr.rs), so a sha256 mismatch must abort, never proceed.
    const installerBuf = await fetchAndVerify(TESSERACT_URL, EXPECTED_INSTALLER_SHA256, 'tesseract installer', { hardFail: true });
    writeFileSync(installerPath, installerBuf);

    console.log('extracting', REQUIRED_FILES.length, 'files from', TESSERACT_ASSET, '...');
    execFileSync(sevenZip, ['x', installerPath, `-o${destDir}`, '-y', ...REQUIRED_FILES], { stdio: 'inherit' });

    for (const name of REQUIRED_FILES) {
      const p = `${destDir}/${name}`;
      if (!existsSync(p)) {
        console.error(`expected file "${name}" missing after extraction — installer layout may have changed`);
        process.exit(1);
      }
    }
    console.log('wrote', REQUIRED_FILES.length, 'files to', destDir, `(${(sumSizes(destDir, REQUIRED_FILES) / 1e6).toFixed(1)} MB)`);

    const traineddataBuf = await fetchAndVerify(TRAINEDDATA_URL, EXPECTED_TRAINEDDATA_SHA256, 'eng.traineddata', { hardFail: true });
    const traineddataPath = `${tessdataDir}/eng.traineddata`;
    writeFileSync(traineddataPath, traineddataBuf);
    console.log('wrote', traineddataPath, `${(traineddataBuf.length / 1e6).toFixed(2)} MB`);

    console.log('smoke test: tesseract.exe --version');
    execFileSync(`${destDir}/tesseract.exe`, ['--version'], { stdio: 'inherit' });
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

/** Fetches `url`, sha256-verifies against `expectedSha256` (warns or exits
 * 1 on mismatch per `hardFail`), and returns the bytes. Copied from
 * fetch-pdfium.mjs's download+verify idiom. */
async function fetchAndVerify(url, expectedSha256, label, { hardFail }) {
  console.log('fetching', label, '<-', url);
  const res = await fetch(url);
  if (!res.ok) {
    console.error(`download failed (${label}):`, res.status, res.statusText);
    process.exit(1);
  }
  const buf = Buffer.from(await res.arrayBuffer());
  console.log('downloaded', label, `${(buf.length / 1e6).toFixed(2)} MB`);

  const digest = createHash('sha256').update(buf).digest('hex');
  console.log('sha256', digest);
  if (digest !== expectedSha256) {
    const msg = `sha256 mismatch for ${label}: expected ${expectedSha256}, got ${digest} — upstream may have changed, verify before trusting this file`;
    if (hardFail) {
      console.error('error:', msg);
      process.exit(1);
    }
    console.warn('warning:', msg);
  }
  return buf;
}

/** Locates a usable 7z.exe: PATH first (via `where`), then the two
 * standard 7-Zip install locations. Returns undefined if none found. */
function find7Zip() {
  try {
    const out = execFileSync('where', ['7z.exe'], { encoding: 'utf8' });
    const first = out.split(/\r?\n/).find((l) => l.trim().length > 0);
    if (first) return first.trim();
  } catch {
    // not on PATH — fall through to well-known install locations
  }
  const candidates = [
    process.env.ProgramFiles && join(process.env.ProgramFiles, '7-Zip', '7z.exe'),
    process.env['ProgramFiles(x86)'] && join(process.env['ProgramFiles(x86)'], '7-Zip', '7z.exe'),
  ].filter(Boolean);
  return candidates.find((p) => existsSync(p));
}

function sumSizes(dir, names) {
  return names.reduce((sum, n) => sum + statSync(`${dir}/${n}`).size, 0);
}

await main();
