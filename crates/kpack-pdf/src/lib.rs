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

use std::fmt;
use std::path::Path;

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

/// Extract text from every page of a PDF, in page order.
///
/// `pdf_bytes` is the raw file content — no filesystem access here, callers
/// own reading the file (mirrors `kpack-core::parse`'s bytes-in
/// convention). `pdfium_lib_path` is the full path to the platform pdfium
/// dynamic library (e.g. `…/pdfium/pdfium.dll` on Windows); it is NOT
/// bundled with this crate — callers resolve it (dev:
/// `tools/fetch-pdfium.mjs`'s output path; shipped app: B4's
/// resources-relative path, mirroring
/// `src-tauri::kpack::bundled_embedder_path`).
///
/// No password support: an encrypted PDF fails with [`PdfError::Parse`]
/// rather than silently returning empty text.
pub fn extract_pages(pdf_bytes: &[u8], pdfium_lib_path: &Path) -> Result<Vec<PageText>, PdfError> {
    let bindings = Pdfium::bind_to_library(pdfium_lib_path)
        .map_err(|e| PdfError::LibraryLoad(e.to_string()))?;
    let pdfium = Pdfium::new(bindings);

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
