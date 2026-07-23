# Verification — OCR for Scanned PDFs (§3b follow-on)

Branch `feat/pdf-ocr`. §3b shipped text-layer PDF ingestion and refused
scanned/image-only PDFs up front. This milestone makes those scanned PDFs
usable: render each image-only page → OCR it with a bundled **Tesseract**
subprocess → feed the same page-locatored (`p.N`) `build_pack` path. **Scope:
printed English, Windows-first.** Handwriting, non-English, and macOS are
deferred.

## What shipped

| Task | What | Commit |
|---|---|---|
| 1 | `kpack-pdf::render_page(bytes, page_index, dpi, lib) -> PNG` — pdfium page render (reuses the runtime-bound pdfium singleton; `extract_pages` untouched). Fixed the plan's two API errors (feature `image_025`, `i32` `PdfPageIndex`/`Pixels`). | ad93302 |
| 2 | `src-tauri/src/ocr.rs` — Tesseract as a bundled subprocess (mirrors the `llama-server` pattern): `ocr_available` (fail-closed gate), `ocr_png` → `ocr_png_with_paths` (per-page 60s timeout, kill+reap on timeout, output to a FILE not the stdout pipe, unique pid+`AtomicU64` temp, RAII cleanup, lossy UTF-8 read). | ecb7952 (+2 review fixes) |
| 3 | `tools/fetch-tesseract.mjs` + bundle — pins the UB-Mannheim installer 5.5.0 (+ sha) and `eng.traineddata` (commit-pinned), extracts a PE-import-minimal 35-file DLL closure via 7-Zip into `resources/ocr/`; `tauri.conf.json` bundles it; gitignored. **Fails CLOSED on a hash mismatch** (it fetches a spawned executable). | a22f0e9 (+ hardening) |
| 4 | `kpack.rs` orchestration — partition text vs image pages; if OCR available, OCR image pages one at a time (render→OCR→drop, soft cap warn>150/refuse>500, per-page degrade), merge in page order → `document_from_pages`; fail-closed refuse only when FULLY image-only + no OCR (a partial scan still builds from its text pages); progress `"ocr"`/`"ocr-notice"` phases; the Cancel button is honored during OCR. | 6f5007a (+ fixes) |
| 5 | Tests — `ocr_fill_pages` unit tests (merge-order, degrade-on-`None`, cancel), the real-tesseract `#[ignore]` `ocr_png_reads_printed_english` (PASSES: reads "alpha"), and a genuinely image-only `scanned.pdf` fixture (PNG IDAT reused as the PDF image stream, no text operator) driving the full `#[ignore]` E2E `scanned_pdf_ocrs_into_a_page_locator_citation` (PASSES: real pdfium→tesseract→embedder → a `p.N` citation). | cde0df6 |

## The loop the user now has

1. **Pick** a scanned/image-only `.pdf` in the Packs builder — same picker as text PDFs.
2. **Render + OCR** — each page with no text layer is rendered at 300 DPI and OCR'd by the bundled tesseract; a live "OCR page N/M…" progress line shows, and **Cancel works during this phase**.
3. **Merge + Build** — OCR'd text is merged with any real text pages in page order, then flows through `build_pack` exactly like §3b — `p.N` locators, same citations.
4. **Refusal, not silence** — a fully image-only PDF on an install without OCR is refused up front (never a silent empty pack); a partially-scanned PDF still builds from its text pages.

## Load-bearing / notable

- **Tesseract adds NO build toolchain** — a bundled prebuilt binary spawned as a subprocess, exactly like `llama-server` and the runtime-bound pdfium DLL. Nothing links it.
- **Subprocess isolation** — a tesseract hang is killed by the per-page timeout (and reaped, no zombie); a crash can't take the app down.
- **Pipe-deadlock avoided by design** — output goes to a FILE, not tesseract's `stdout` pseudo-target, so a verbose/garbage page can't block on a full OS pipe buffer and masquerade as a timeout.
- **`kpack-core` stays pure** — OCR is entirely in `kpack-pdf` (render) + `src-tauri` (subprocess); the format core only ever sees "pages of text with page numbers".
- **Fail-closed hash on the fetch** — `fetch-tesseract.mjs` aborts on a sha256 mismatch (it fetches an executable).

## Reviews

Every task reviewed (spec + quality); Tasks 2, 4 went through fix loops (subprocess zombie/pipe/UTF-8; a partial-scan regression). **Final whole-branch review (opus): ready to merge, no Critical/blocking** — verified the units compose end-to-end and the non-OCR paths are unchanged; its one Important (Cancel dead during OCR) was fixed (commit d313ee4).

## The bug the acceptance test caught (fix `6ab0e6b`)

The user's first real-world run refused **every** scanned PDF ("OCR couldn't read
any text") — while render + tesseract worked on the identical files in every
isolated test. Root cause (systematic-debugging → an in-app diagnostic log):
Tauri's `resource_dir()` returns Windows **verbatim** paths (`\\?\C:\…`), and
tesseract appends `/eng.traineddata` (a forward slash) to `--tessdata-dir`.
Windows never normalizes `\\?\` paths, so the `/` is a literal character →
`Error opening data file … Failed loading language 'eng'` → every page empty →
refusal. It bit **only** the packaged app because only `resource_dir()` yields
verbatim paths; every dev/test/E2E path is plain and tolerates the `/` — which
is precisely why the whole test suite and every repro passed. Fix:
`strip_verbatim()` de-prefixes `\\?\`/`\\?\UNC\` before tesseract sees the path;
tesseract's stderr is now captured (was nulled) so a failure reports its real
reason. Regression tests: a `strip_verbatim` unit test + a real-tesseract
`#[ignore]` test that feeds a `\\?\` tessdata dir and asserts it still reads
(red before, green after). **General lesson:** the app resolves several bundled
subprocess tools (tesseract, llama-server, pdfium) through `resource_dir()` —
de-verbatim any resource path handed to one as a command-line argument.

## Acceptance E2E (user, on the MSI) — the gate

1. Pick a **scanned/image-only** printed-English PDF → it builds a pack with page-numbered citations, OCR progress shown (no longer refused).
2. Attach it → an in-corpus question returns a **cited** answer with a `p.N` locator.
3. A **text-layer** PDF still builds via the fast text path (no OCR) — unchanged.
4. A blank/garbage image PDF (OCR finds nothing) → refused cleanly, no empty pack.

## Honest carve-outs (deferred)

- **Printed English only** — handwriting and other languages are later milestones.
- **Windows-first** — the fetch script + path resolution are per-OS but only Windows is wired/verified in v1; macOS is a follow-on.
- **Build prerequisite:** `fetch-tesseract.mjs` needs a system **7-Zip** to unpack the NSIS installer (fails loudly with install instructions if absent) — the pipeline's other fetch scripts need none.
- Multi-column scans inherit pdfium's content-stream order caveat.

## Backlog (from the final review's triage)

- Harden `fetch-pdfium.mjs`/`fetch-embedder.mjs` to fail-closed on hash mismatch too (repo-wide; prioritize pdfium — also a loaded native lib).
- `fetch-tesseract.mjs` platform gate → extensible `switch` for the macOS branch.
- `ocr_available` could also check `tessdata/eng.traineddata` (a partial-fetch install fails closed but mis-messages the cause).
- Cosmetic: OCR→embedding progress-bar reset; PNG has no `pHYs` DPI chunk (tesseract warns, works at 300-DPI pixels); `render_page`'s `PdfError::Parse` wording (never surfaces in the OCR path).
