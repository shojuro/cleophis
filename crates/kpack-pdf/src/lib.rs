//! `kpack-pdf` — PDF text extraction, page by page, via `pdfium-render`
//! (a Rust wrapper around `pdfium`, Chromium's PDF engine).
//!
//! Isolated from `kpack-core` on purpose, mirroring `kpack-embed`'s split:
//! this crate is pure bytes-in, text-out with no knowledge of the `.kpack`
//! `Document` format. Turning [`extract_pages`]'s output into a buildable
//! `Document` is §3b slice B2's job, living in `kpack-core` as a helper
//! that takes `Vec<PageText>` as plain data — `kpack-core` never depends on
//! this crate, so the format core stays free of pdfium's native-library
//! dependency.
//!
//! ## Runtime-bound, not build-time-linked (the key difference from
//! `kpack-embed`)
//! `pdfium-render` binds a **prebuilt pdfium dynamic library at runtime**
//! (`Pdfium::bind_to_library`, via `libloading` under the hood) rather than
//! linking pdfium into the compiled binary. That means this crate compiles
//! with plain `cargo build` — no libclang, no cmake, no native toolchain at
//! all (contrast `kpack-embed`'s `llama-cpp-2`, which builds llama.cpp from
//! C++ via `bindgen` + `cmake`). Only [`extract_pages`], when actually
//! called, needs a real `pdfium.dll` (or platform equivalent) present on
//! disk at `pdfium_lib_path` — fetched by `tools/fetch-pdfium.mjs`, never
//! committed to the repo (gitignored, like `kpack-embed`'s bundled GGUF).
//!
//! ## Page ordering
//! Extracted text follows pdfium's own **content-stream draw order**, not
//! necessarily visual reading order — a PDF with multi-column layout or
//! out-of-order content-stream operators can yield text that doesn't read
//! linearly within a page. That's inherent to any PDF text extractor (PDF
//! has no built-in concept of "reading order"); this crate does no
//! reordering, trimming, or filtering beyond what pdfium itself returns.
//!
//! ## Binding pdfium exactly once per process (the part that's easy to get
//! silently wrong)
//! `Pdfium::bind_to_library` performs a **process-global, one-time** native
//! library load — a second call anywhere in the same process (a second
//! `extract_pages`, whether a second PDF, a second pack build, or a retry)
//! errors `PdfiumLibraryBindingsAlreadyInitialized` rather than returning a
//! second handle. [`pdfium`] (mirrors `kpack-embed::bge::shared_backend`'s
//! `OnceLock` pattern for `LlamaBackend::init()`, the exact same
//! process-global-singleton shape) binds the library at most once and
//! caches the bound `Pdfium` — or the bind error — for every later call to
//! reuse. A caller passing a *different* `pdfium_lib_path` on a later call
//! than the one that won the race is a no-op: the first-bound library keeps
//! being used, which is the only sane behavior given the binding is
//! inherently process-global (there's no way to rebind to a different
//! library after the fact).

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use pdfium_render::prelude::*;

/// One page's extracted text. `page` is **1-based** (the first page is
/// `1`), matching the `.kpack` locator convention (`kpack-core::tree`'s
/// `"p.N"` block locators) rather than pdfium's internal 0-based page
/// index — B2's `Document`-from-pages helper can use `page` directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

/// Errors from [`extract_pages`]. Hand-rolled (no `thiserror`, matching
/// `kpack-core`'s `EmbedError`/`format::Error` style) — small on purpose.
/// Both variants carry a plain-language, user-safe message (never pdfium's
/// raw internal error `Debug` output) suitable for surfacing directly in
/// the UI (B3) when a PDF fails to extract.
#[derive(Debug)]
pub enum PdfError {
    /// The pdfium dynamic library itself couldn't be bound — missing file,
    /// wrong architecture, or a `pdfium.dll` too old/new for this crate's
    /// pinned bindings. Not the PDF's fault.
    LibraryLoad(String),
    /// The library bound fine, but the PDF bytes themselves couldn't be
    /// parsed or a page's text couldn't be read — corrupt file, a password
    /// this crate doesn't supply, or an unsupported PDF feature.
    Parse(String),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfError::LibraryLoad(msg) => write!(f, "could not load the PDF engine: {msg}"),
            PdfError::Parse(msg) => write!(f, "could not read this PDF: {msg}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// Process-wide, lazily-bound `Pdfium` handle. See the module doc's
/// "Binding pdfium exactly once per process" section for why this can't
/// just be `Pdfium::bind_to_library` inside `extract_pages` directly. The
/// `thread_safe` Cargo feature (enabled in `Cargo.toml`) is what makes
/// `Pdfium` itself `Send + Sync` and therefore legal to hold in a `static`
/// at all — without it this wouldn't compile.
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// Bind (once) or reuse the cached process-wide pdfium handle. The bind
/// error, if any, is cached too — every caller after the first sees the
/// same [`PdfError::LibraryLoad`] rather than re-attempting (and
/// re-failing differently on) a doomed bind.
fn pdfium(pdfium_lib_path: &Path) -> Result<&'static Pdfium, PdfError> {
    let result = PDFIUM.get_or_init(|| {
        Pdfium::bind_to_library(pdfium_lib_path)
            .map(Pdfium::new)
            .map_err(|e| e.to_string())
    });
    result
        .as_ref()
        .map_err(|msg| PdfError::LibraryLoad(msg.clone()))
}

/// Extract text from every page of a PDF, in page order.
///
/// `pdf_bytes` is the raw file content — no filesystem access here, callers
/// own reading the file (mirrors `kpack-core::parse`'s bytes-in
/// convention). `pdfium_lib_path` is the full path to the platform pdfium
/// dynamic library (e.g. `…/pdfium/pdfium.dll` on Windows); it is NOT
/// bundled with this crate — callers resolve it (dev:
/// `tools/fetch-pdfium.mjs`'s output path; shipped app: B4's
/// resources-relative path, mirroring
/// `src-tauri::kpack::bundled_embedder_path`). Only the FIRST call in a
/// process actually binds the library at this path — see [`pdfium`].
///
/// No password support: an encrypted PDF fails with [`PdfError::Parse`]
/// rather than silently returning empty text.
pub fn extract_pages(pdf_bytes: &[u8], pdfium_lib_path: &Path) -> Result<Vec<PageText>, PdfError> {
    let pdfium = pdfium(pdfium_lib_path)?;

    let document = pdfium
        .load_pdf_from_byte_slice(pdf_bytes, None)
        .map_err(|e| PdfError::Parse(e.to_string()))?;

    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let text = page
            .text()
            .map_err(|e| PdfError::Parse(format!("page {}: {e}", index + 1)))?
            .all();
        pages.push(PageText {
            page: (index as u32) + 1,
            text,
        });
    }

    Ok(pages)
}

/// Render one 0-based page to PNG bytes at `dpi`. Single-page on purpose:
/// the caller renders → OCRs → drops one page before the next, bounding
/// memory to a single full-page bitmap. Reuses the process-global pdfium
/// binding (same [`pdfium`] OnceLock as [`extract_pages`]).
pub fn render_page(
    pdf_bytes: &[u8],
    page_index: usize,
    dpi: u16,
    pdfium_lib_path: &Path,
) -> Result<Vec<u8>, PdfError> {
    let pdfium = pdfium(pdfium_lib_path)?;
    let document = pdfium
        .load_pdf_from_byte_slice(pdf_bytes, None)
        .map_err(|e| PdfError::Parse(e.to_string()))?;

    let page = document
        .pages()
        // `PdfPageIndex` is `i32` in this pdfium-render version (not `u16`
        // as a page count would suggest) — `page_index` stays `usize` at
        // this fn's public boundary and converts only at the call site.
        .get(page_index as i32)
        .map_err(|e| PdfError::Parse(format!("page {}: {e}", page_index + 1)))?;

    // PDF page dimensions are in points (1/72 inch); target pixel width = points * dpi/72.
    let scale = dpi as f32 / 72.0;
    let target_w = (page.width().value * scale).round().clamp(1.0, 10_000.0) as u16;

    // `Pixels` (the type `set_target_width` takes) is `i32` here, not
    // `u16` — `target_w` stays `u16` (its clamp range comfortably fits)
    // and converts only at this call.
    let config = PdfRenderConfig::new().set_target_width(target_w.into());
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| PdfError::Parse(format!("render page {}: {e}", page_index + 1)))?;
    let image = bitmap
        .as_image()
        .map_err(|e| PdfError::Parse(format!("image page {}: {e}", page_index + 1)))?;

    let mut png = Vec::new();
    image
        .into_rgb8()
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| PdfError::Parse(format!("encode page {}: {e}", page_index + 1)))?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE for whoever runs the `#[ignore]`d real-pdfium tests below: they
    // need the WINDOWS cargo, not the Linux one, even from a WSL2 shell.
    // `pdfium.dll` (fetched by `tools/fetch-pdfium.mjs`) is a Windows PE32+
    // binary — a Linux process's dynamic loader cannot open it at all
    // (`libloading`/`dlopen` fails with "invalid ELF header"; this is a
    // hard OS-level format mismatch, not something any Rust code here can
    // work around). `cargo test -p kpack-pdf` (non-ignored) is fine on
    // Linux — only these real-library tests need
    // `C:\Users\JM505 Computers\.cargo\bin\cargo.exe` via the WSL2 pwsh
    // interop pattern.

    // Pure, no pdfium involved — the plain-language `Display` wording is
    // this crate's contract with B3's UI surfacing, so it's worth locking
    // down independent of the real-library `#[ignore]`d test below.
    #[test]
    fn pdf_error_display_is_plain_language() {
        let lib_err = PdfError::LibraryLoad("file not found".to_string());
        assert!(lib_err.to_string().contains("PDF engine"));

        let parse_err = PdfError::Parse("bad xref table".to_string());
        assert!(parse_err.to_string().contains("could not read"));
    }

    // Regression test for the field bug this fix addresses: a second
    // `extract_pages` call in the same process used to error
    // `PdfiumLibraryBindingsAlreadyInitialized` because `bind_to_library`
    // was called fresh every time — pdfium's library binding is a
    // process-global one-time init. The original smoke test never caught
    // this because it only ever called `extract_pages` once per process.
    // Needs the real dll (`node tools/fetch-pdfium.mjs` first) and the
    // fixture PDF, so it's `#[ignore]`d like the other real-library tests.
    #[test]
    #[ignore]
    fn extract_pages_can_be_called_twice_in_one_process() {
        let pdfium_lib_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("src-tauri")
            .join("resources")
            .join("pdfium")
            .join("pdfium.dll");
        assert!(
            pdfium_lib_path.exists(),
            "pdfium.dll missing: {} — run `node tools/fetch-pdfium.mjs` first",
            pdfium_lib_path.display()
        );

        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample.pdf");
        let pdf_bytes = std::fs::read(&fixture_path)
            .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", fixture_path.display()));

        let first = extract_pages(&pdf_bytes, &pdfium_lib_path);
        assert!(first.is_ok(), "first extract_pages call failed: {first:?}");

        let second = extract_pages(&pdf_bytes, &pdfium_lib_path);
        assert!(
            second.is_ok(),
            "second extract_pages call in the same process failed (this is the \
             PdfiumLibraryBindingsAlreadyInitialized regression): {second:?}"
        );

        assert_eq!(first.unwrap(), second.unwrap(), "both calls should extract identical pages");
    }

    #[test]
    #[ignore] // needs the real pdfium.dll (`node tools/fetch-pdfium.mjs`) + the fixture PDF
    fn render_page_produces_a_decodable_png() {
        let pdfium_lib_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..").join("..").join("src-tauri")
            .join("resources").join("pdfium").join("pdfium.dll");
        assert!(pdfium_lib_path.exists(), "pdfium.dll missing — run `node tools/fetch-pdfium.mjs`");
        let pdf_bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("sample.pdf"),
        ).expect("read fixture");

        let png = render_page(&pdf_bytes, 0, 300, &pdfium_lib_path).expect("render page 0");
        // It is a real PNG (8-byte signature) and decodes to a positive-size image.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        let img = image::load_from_memory(&png).expect("decode png");
        assert!(img.width() > 0 && img.height() > 0, "empty render");
    }
}
