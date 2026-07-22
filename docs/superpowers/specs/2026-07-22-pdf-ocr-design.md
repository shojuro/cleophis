# OCR for Scanned PDFs — Design (§3b follow-on)

> Adds on-device OCR so scanned/image-only PDFs — refused up front today —
> become usable packs, using the same page-locatored `build_pack` path §3b
> already ships for text-layer PDFs.

## Goal

Pick a scanned (image-only) `.pdf` in the Packs builder → get a pack with
page-numbered (`p.N`) citations → chat against it exactly like a text-layer PDF.
The **only** new capability is turning page images into text; everything
downstream (chunk → embed → format → manifest → retrieve → cite) is unchanged.

## Scope (v1)

- **Printed English text only.** Scanned books, papers, photocopied handouts.
- **Windows-first.** Bundle + verify the tesseract binary on Windows (matches the
  shipping MSI + the demo). The fetch script and path resolution are written
  per-OS so macOS is a clean follow-on, but v1 verifies Windows only.
- Engine: **Tesseract, invoked as a bundled subprocess** — the same
  "bundle a prebuilt binary + spawn a managed subprocess" pattern the app
  already uses for `llama-server`.

## Non-goals (explicit, deferred)

- **Handwriting** (a different, harder neural problem).
- **Non-English / multilingual** OCR (per-language data or a bigger model).
- **macOS binary** in v1 (structured for, not shipped in, v1).
- **In-process libtesseract** (would add a leptonica/tesseract build toolchain —
  the exact friction §3b avoided with runtime-bound pdfium).
- **Layout/column reordering, table structure, OCR confidence gating** — v1
  takes tesseract's text output per page as-is (page-scoped, like the text
  layer). A confidence threshold is a later refinement.
- **Reusing OCR for text-layer PDFs** — text-layer extraction is unchanged and
  always preferred; OCR only runs on pages with no extractable text.

## Architecture

The scanned-detection point that refuses today (`src-tauri/src/kpack.rs:433-436,
584-595` — per-page empty-text check, already handling partially-scanned PDFs)
becomes the OCR fallback. Three units, each with one responsibility:

### Unit 1 — `kpack-pdf`: add page rendering

`kpack-pdf` currently does text extraction only. Add rendering, keeping the crate
pure (bytes-in → bytes-out, no `.kpack`/`Document` knowledge, same
runtime-bound pdfium singleton):

```rust
/// Render ONE 0-based page to PNG bytes at `dpi`. Reuses the process-global
/// pdfium binding (same OnceLock as extract_pages). Errors are the same
/// PdfError family. `extract_pages` is UNCHANGED and byte-identical.
///
/// Single-page (not batch) on purpose: the orchestration renders → OCRs → drops
/// one page before the next, so at most one full-page bitmap/PNG is resident —
/// a hard memory bound on a 300-page scanned book. The per-call PDF-document
/// open is negligible against tesseract's ~1-3 s/page.
pub fn render_page(
    pdf_bytes: &[u8],
    page_index: usize,
    dpi: u16,
    pdfium_lib_path: &Path,
) -> Result<Vec<u8>, PdfError>;   // PNG bytes
```

- Enable pdfium-render's render feature (base render/bitmap API + PNG encode; the
  crate's `image` feature or the bitmap `as_rgba_bytes` + the `image` crate to
  encode PNG — chosen at implementation time, whichever keeps the dep set
  smallest). The `=0.9.3` version pin and `pdfium_7881` ABI pin are preserved.
- `dpi` default **300** (the OCR standard: accuracy vs. render time/memory).
- Renders one requested (image-only) page, never the whole doc.

### Unit 2 — `src-tauri/src/ocr.rs`: the Tesseract subprocess (new)

```rust
/// Resolve the bundled tesseract binary + tessdata dir. Mirrors
/// bundled_pdfium_path / bundled_embedder_path exactly.
fn bundled_tesseract_path(app: &AppHandle) -> PathBuf;   // resources/ocr/tesseract(.exe)
fn bundled_tessdata_dir(app: &AppHandle) -> PathBuf;     // resources/ocr/tessdata

/// OCR one rendered page. Writes png to a scratch temp, runs
/// `tesseract <tmp.png> stdout -l eng --tessdata-dir <dir>` with a per-page
/// timeout, returns stdout text. Cleans up the temp on every path.
/// Returns Err on: missing binary, spawn failure, timeout, non-zero exit.
pub fn ocr_png(app: &AppHandle, png_bytes: &[u8]) -> Result<String, OcrError>;

/// True iff the bundled tesseract binary is present + executable — the app
/// decides "OCR available?" fail-closed before offering the OCR path.
pub fn ocr_available(app: &AppHandle) -> bool;
```

- Subprocess isolation contains a tesseract hang (the per-page timeout kills it)
  and any tesseract crash — neither can take down the app.
- Language pinned `-l eng`. Temp files under the app scratch dir, unique-named,
  removed in a drop-guard.

### Unit 3 — `kpack.rs`: orchestration (modify the scanned branch)

Replace the "empty text → refuse" branch with:

1. `extract_pages` → per-page text (unchanged).
2. Partition pages: **text pages** (non-empty) vs **image pages** (empty/whitespace).
3. If there are image pages AND `ocr_available`:
   - **Soft cap:** image-page count > 150 → emit an up-front warning event with a
     rough time estimate, then proceed. > 500 → **refuse** with a clear runaway
     message (no accidental 30-minute build).
   - For each image page, one at a time: `render_page(idx, 300)` → PNG →
     `ocr_png` → text → **drop the PNG** before the next (memory bound: one
     page resident). Emit a `build-progress` event per page ("OCR page N/M…").
     A page whose render or OCR fails/times out **degrades to empty** (skipped) —
     never fails the whole build (same per-page degradation the parsers use).
4. Merge: text-layer text for text pages + OCR text for image pages, in page
   order → `document_from_pages` (UNCHANGED — still yields `p.N` locators).
5. If the merged document has **no** usable text even after OCR (every page blank
   or all OCR failed, or OCR unavailable) → refuse, message distinguishing
   "no text and OCR unavailable" from "OCR ran but found nothing".

Downstream (`build_pack`, retrieval, citation rendering) is untouched.

## Data flow

```
PDF bytes
  → kpack_pdf::extract_pages ─→ text pages ───────────────────────────┐
                             └→ image pages → for each (one at a time):
                                   kpack_pdf::render_page → PNG
                                     → ocr::ocr_png (subprocess, progress) → text → drop PNG
                                     └──────────────────────────────────────┤
                                                                            ▼
                                        merge in page order → document_from_pages
                                                                            ▼
                                        build_pack (chunk → embed → format → manifest)  [unchanged]
```

## Bundling

- **Fetch:** `tools/fetch-tesseract.mjs`, mirroring `tools/fetch-pdfium.mjs` —
  downloads the prebuilt Windows tesseract binary + `eng.traineddata` (fast
  variant) into `resources/ocr/` (`tesseract.exe`, `tessdata/eng.traineddata`).
  Gitignored, never committed (like `pdfium.dll` and the bundled GGUF). Per-OS
  branch present; only the Windows branch is exercised in v1.
- **Bundle:** `tauri.conf.json` `bundle.resources` += `"resources/ocr/":
  "resources/ocr/"`, exactly like the existing `resources/pdfium/` +
  `resources/embedders/` entries.
- **Size:** tesseract.exe ~5 MB + `eng.traineddata` (fast) ~15 MB ≈ +20 MB →
  installer ~170 MB, comfortably under the ~200 MB cap the project respects.

## UX / progress

- Per-page `build-progress` during OCR ("OCR page 12/40…") — the event
  mechanism already exists for pack builds; OCR just adds a phase.
- Soft cap warning (>150 pages) carries a rough estimate ("~40 scanned pages —
  this may take a couple of minutes").

## Error handling / degradation

| Condition | Behavior |
|---|---|
| tesseract binary missing | `ocr_available` false → refuse with today's "OCR not available" message (fail-closed; never a silent empty pack) |
| One page's OCR fails/times out | that page degrades to empty text, build continues |
| Whole doc yields no text after OCR | refuse, message distinguishes unavailable-vs-ran-empty |
| >500 image pages | refuse (runaway guard) |
| Malformed PDF crashes pdfium during render | same in-process risk §3b already documented for extraction; tesseract itself is subprocess-isolated |

## Testing

- `kpack-pdf`: `render_page` produces a decodable PNG of expected dimensions for
  a known text-layer PDF page (real pdfium, `#[ignore]` like the extraction test).
- `ocr.rs`: `ocr_png` on a checked-in scanned-text PNG returns the expected words
  (real tesseract, `#[ignore]`); `ocr_available` false when the binary is absent.
- `kpack.rs`: unit tests for the text/image page partition + merge-in-page-order
  logic (mockable, no native deps); soft-cap/runaway thresholds.
- E2E (real deps): a scanned test PDF → OCR → pack whose retrieved citation
  carries a `p.N` locator (mirrors §3b's `build_from_pdf_produces_a_page_locator_citation`).

## Acceptance E2E (user, on the MSI) — the gate

1. Pick a **scanned/image-only** text PDF in the Packs builder → it builds a pack
   with page-numbered citations (no longer refused), progress shown.
2. Attach it → an in-corpus question returns a **cited** answer with a `p.N` locator.
3. A **text-layer** PDF still builds via the fast text path (no OCR, unchanged).
4. A blank/garbage image PDF (OCR finds nothing) → refused cleanly, no empty pack.

## Decisions (locked with user, 2026-07-22)

- Scope: **printed English**, v1 (defer handwriting + multilingual).
- Engine: **Tesseract subprocess** (over in-process libtesseract / neural ONNX) —
  fits the bundle-a-binary + managed-subprocess pattern, no build toolchain,
  crash-isolated, proven for printed English.
- Platform: **Windows-first**, macOS structured as a follow-on.
- Large docs: **progress + soft cap** (warn >150 pages, refuse >500).

## Risks / notes

- OCR text is noisy vs. a clean text layer — retrieval quality on OCR'd packs is
  inherently lower; acceptable for v1 (the alternative today is *no pack at all*).
- Multi-column scans inherit the same content-order caveat as text-layer PDFs.
- Render memory: 300 DPI full-page bitmaps are a few MB each; render + OCR one
  page at a time (don't hold all PNGs) to bound memory on large docs.
