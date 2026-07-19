//! Real-pdfium extraction smoke test — binds the actual `pdfium.dll` and
//! extracts from a small, committed, text-based PDF fixture, proving the
//! whole real path works: page count, page order, and known text landing
//! on the right page. Mirrors `kpack-embed`'s `bge_smoke.rs`'s real-
//! artifact, `#[ignore]`d pattern — no mocks.
//!
//! Requires: `node tools/fetch-pdfium.mjs` (writes `pdfium.dll` this test
//! reads) — no Cargo feature gate needed, since `kpack-pdf` has no
//! build-toolchain split (see `src/lib.rs`'s module doc). Run (Windows,
//! after the dll is fetched):
//!   cargo test -p kpack-pdf -- --ignored --nocapture

use std::path::Path;

use kpack_pdf::extract_pages;

#[test]
#[ignore]
fn real_pdf_extraction_smoke() {
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

    let pages = extract_pages(&pdf_bytes, &pdfium_lib_path).expect("extract_pages failed");

    assert_eq!(pages.len(), 2, "fixture PDF has 2 pages");

    // 1-based, in order.
    assert_eq!(pages[0].page, 1);
    assert_eq!(pages[1].page, 2);

    assert!(
        pages[0].text.contains("alpha"),
        "page 1 text missing known substring \"alpha\": {:?}",
        pages[0].text
    );
    assert!(
        pages[1].text.contains("bravo"),
        "page 2 text missing known substring \"bravo\": {:?}",
        pages[1].text
    );

    eprintln!(
        "b1: extracted {} pages, page 1 = {:?}, page 2 = {:?}",
        pages.len(),
        pages[0].text,
        pages[1].text
    );
}
