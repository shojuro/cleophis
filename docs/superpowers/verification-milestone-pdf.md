# Verification — §3b Real Document Parsers, PDF-first

Branch `feat/rag-pdf`. §3a shipped the visible RAG loop MD/TXT-only; the
product finding that followed was blunt — the ICP (students/teachers)
predominantly upload PDFs (books, papers, assignments), so §3b leads with
PDF rather than rounding out the parser set breadth-first. **Scope: PDF
text-layer extraction end to end** — pick a `.pdf` in the builder, get a
pack with page-numbered citations, chat against it exactly like MD/TXT.
DOCX/EPUB/HTML, OCR for scanned PDFs, and the §2 curated-pipeline gate
calibration are out of scope, deferred below.

## What shipped

| Slice | What | Commits |
|---|---|---|
| B1 | `kpack-pdf` crate — `pdfium-render` 0.9.3 (default-features off, `pdfium_7881` + `thread_safe`), runtime dynamic bind to a bundled `pdfium.dll`; `extract_pages(bytes, lib_path) -> Vec<(page:u32, text:String)>`; ABI-pinned to `chromium/7881` | 6d431aa |
| B2 | `kpack-core` seam — `SourceInput.content: SourceContent{Raw(String)\|Prebuilt(Document)}`; pure `document_from_pages` (page → `p.N` locators, no outline/heading parsing, zero schema change) | 900c1df |
| B3 | `src-tauri` PDF build branch — bytes → `kpack_pdf::extract_pages` → scanned/image-only detection → `document_from_pages` → `SourceContent::Prebuilt`; plain-language error surfacing; FE picker accepts `.pdf` | ca903eb |
| B4 | Bundle `pdfium.dll` into the MSI (`tauri.conf.json` resources); lock Prebuilt-hash build determinism as a test | (this) |

## The loop the user now has

1. **Pick** a `.pdf` in the Packs builder, same file-picker flow as MD/TXT.
2. **Extract** — `kpack-pdf` pulls text per page via pdfium; each page's
   paragraphs become blocks with a `p.N` locator (mirrors `parse_txt`'s
   blank-line paragraph split, just page-scoped instead of line-scoped).
3. **Build** — the page-locatored `Document` flows through `build_pack`
   exactly like a parsed MD/TXT document (chunk → embed → format →
   manifest) — `kpack-core` never learns it came from a PDF.
4. **Chat** — attach the pack and ask a question: citations resolve to
   `[n] Title · locator` where `locator` is `p.N`, same rendering path as
   §3a's MD/TXT citations.
5. **Refusal, not silence** — a scanned/image-only PDF (no extractable text
   layer) is refused up front with a clear "no extractable text (OCR not
   supported yet)" message, before any pack write — never a silently empty
   or half-built pack.

## Load-bearing / notable

- **pdfium adds NO build toolchain.** `pdfium-render`'s bindings are pure
  Rust with a runtime `libloading` dynamic bind to a prebuilt `pdfium.dll`
  — `cargo build -p kpack-pdf` succeeds with no cmake/bindgen/cc in the dep
  tree. A sharp contrast to `llama.cpp`'s native build requirement; PDF
  support cost zero toolchain friction.
- **The DLL is small enough to stay well under the installer cap.** Measured
  7,211,520 bytes (7.21 MB) uncompressed, ~3.7 MB compressed inside the MSI
  — the installer lands around 148-150 MB, comfortably under the ~200 MB
  cap the §3b scouting set as the constraint to respect.
- **`kpack-core` stays pure.** PDF extraction is entirely native and lives
  outside this dependency-free crate, in `kpack-pdf`. The only thing
  `kpack-core` learns about a PDF is "pages of text with page numbers" —
  the `SourceContent::Prebuilt(Document)` seam (B2) means `build_pack`
  chunks/embeds a Prebuilt `Document` exactly like a parsed one, no PDF-
  specific code path inside the crate at all.
- **The Raw MD/TXT path is byte-identical to pre-§3b.** `SourceContent::Raw`
  is the exact K8/§3a behavior, unchanged; `Prebuilt` is purely additive —
  confirmed in the B2 review and re-confirmed here by the new determinism
  test, which only touches the `Prebuilt` arm.
- **Prebuilt-hash determinism is now a locked invariant, not an assumption.**
  The B2 review flagged that the existing `t2` determinism test only
  exercised `Raw` sources; `document_content_text`/`document_from_pages`
  were deterministic by inspection but untested. The new
  `prebuilt_document_build_is_deterministic` test builds the same
  `SourceContent::Prebuilt(document_from_pages(...))` document through
  `build_pack` (mock embedder) twice and asserts byte-identical `docs.sha256`
  and byte-identical stored int8 embeddings across both builds — the same
  proof technique `t2` uses for `Raw`, applied to the path B2 left uncovered.
- **The DLL now ships in the installer.** B3 added the runtime path
  resolution (`bundled_pdfium_path` = `resources_root(app).join("pdfium/pdfium.dll")`,
  mirroring `bundled_embedder_path`), but a real install had no file at
  that path until this slice's `tauri.conf.json` change — `"resources/pdfium/":
  "resources/pdfium/"` in `bundle.resources`, mirroring the existing
  `resources/embedders/` entry exactly.

## Reviews

Every slice reviewed: B1 (opus-level, independently re-verified — rebuilt
with `libclang` unset, re-hashed the DLL, ran the ignored real-pdfium test,
web-confirmed the ABI pin: `pdfium-render`'s `pdfium_7881` feature ↔ the
fetched `chromium/7881` binary are the same PDFium 151.0.7881.0 build by
construction), B2 (sonnet — Raw path byte-identical, no locator collision
across chunk boundaries, hash determinism deterministic-by-inspection and
documented as non-comparable to a Raw byte-hash), B3 (sonnet — scanned
detection refuses before any pack write, plain-language error surfacing,
MD/TXT unchanged, `p.N` locator proven to survive into a retrieved
citation). All **Approved**. The real-PDF E2E
(`build_from_pdf_produces_a_page_locator_citation`) passes against the real
pdfium DLL and the real embedder, not just the mock.

## Acceptance E2E (user, on the MSI) — the gate

Because §3b adds a second native dependency shipping inside the packaged
app, the acceptance test is a hands-on run on a freshly built MSI (B5):

1. Pick a text-layer `.pdf` in the Packs builder — it builds successfully
   with page-numbered citations, same UI flow as MD/TXT.
2. Attach the pack to a chat; an in-corpus question returns a cited answer
   whose citation locator is `p.N`.
3. Pick a scanned/image-only PDF — the builder refuses it with the
   "no extractable text (OCR not supported yet)" message, no empty pack
   left behind.
4. An MD/TXT pack built in the same session behaves exactly as §3a —
   unaffected by the PDF path existing alongside it.

## Honest carve-outs (documented, not hidden)

- **Text-layer PDFs only.** Scanned/image-only PDFs need OCR — a later
  milestone — and are detected and reported up front, never silently
  turned into an empty or broken pack.
- **Multi-column and complex layouts can interleave.** pdfium returns text
  in content-stream draw order, not guaranteed visual reading order — a
  two-column academic paper's paragraphs may not chunk in the order a human
  reader would scan the page.
- **A maliciously malformed PDF could in theory crash the native pdfium
  library** — an inherent FFI risk, uncatchable from the Rust side. Judged
  low in-scope threat: pack PDFs are local, user-picked files, not
  untrusted network input. Subprocess sandboxing is a possible future
  hardening, not built here.

## Deferred (own milestones, by design)

- **DOCX/EPUB/HTML** — pure-Rust parsers, no native dependency; the fast
  follow-on once PDF (the highest-value, native-dependency format) is done.
- **OCR for scanned PDFs** — the carve-out above, a real milestone of its
  own (model choice, on-device inference cost).
- **PDF outline/bookmark → `section_path`.** v1 uses page locators only
  (`p.N`) with an empty section path — no heading-tree extraction from the
  PDF's outline in this slice.
- **§2** the curated server pipeline + the real per-pack gate calibration
  that replaces the §1 placeholder thresholds.
- The **contract-trained LoRA adapter** (parked hot-swap track) — unrelated
  to §3b, still pending its own milestone.
