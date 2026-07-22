//! On-device OCR for scanned PDF pages via a BUNDLED tesseract binary run as
//! a subprocess — the same "ship a prebuilt binary in resources/, spawn it,
//! never link it" pattern as `llama-server` (`inference.rs`) and the pdfium
//! dynamic library. Isolation: a tesseract hang is killed by the per-page
//! timeout; a tesseract crash can't take down the app.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tauri::AppHandle;

/// Bundled tesseract lives in `resources/ocr/` alongside `resources/pdfium/`.
const OCR_RELATIVE_DIR: &str = "ocr";
/// Per-page OCR timeout — a runaway/looping tesseract on one page is killed
/// and that page degrades to empty rather than hanging the whole build.
const PER_PAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Monotonic counter for scratch temp-file names (see `ocr_png_with_paths`).
/// `Instant::now().elapsed()` (the brief's original shorthand) is near-zero
/// and NOT unique enough to avoid collisions between calls in the same
/// process within the same build loop; a `fetch_add`'d counter combined with
/// the pid is.
static NEXT_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum OcrError {
    /// The bundled tesseract binary isn't present/runnable — OCR is off.
    Unavailable,
    /// Tesseract ran but failed (non-zero exit, timeout, unreadable output).
    Failed(String),
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OcrError::Unavailable => write!(f, "OCR is not available on this install"),
            OcrError::Failed(m) => write!(f, "OCR failed: {m}"),
        }
    }
}
impl std::error::Error for OcrError {}

fn tesseract_binary_name() -> &'static str {
    if cfg!(windows) { "tesseract.exe" } else { "tesseract" }
}

/// The bundled tesseract executable — mirrors `kpack::bundled_pdfium_path`.
pub fn bundled_tesseract_path(app: &AppHandle) -> PathBuf {
    crate::inference::resources_root(app)
        .join(OCR_RELATIVE_DIR)
        .join(tesseract_binary_name())
}

/// The bundled `tessdata` directory holding `eng.traineddata`.
pub fn bundled_tessdata_dir(app: &AppHandle) -> PathBuf {
    crate::inference::resources_root(app).join(OCR_RELATIVE_DIR).join("tessdata")
}

/// True iff the bundled tesseract binary is present — the app gates the OCR
/// path on this and falls back to the scanned-PDF refusal when false.
pub fn ocr_available(app: &AppHandle) -> bool {
    bundled_tesseract_path(app).is_file()
}

/// OCR one rendered page (PNG bytes) → recognized English text. Resolves the
/// bundled tesseract binary + tessdata dir for `app` and delegates to
/// `ocr_png_with_paths` for the actual subprocess work.
pub fn ocr_png(app: &AppHandle, png_bytes: &[u8]) -> Result<String, OcrError> {
    let bin = bundled_tesseract_path(app);
    if !bin.is_file() {
        return Err(OcrError::Unavailable);
    }
    let tessdata = bundled_tessdata_dir(app);
    ocr_png_with_paths(&bin, &tessdata, png_bytes)
}

/// The real OCR logic, parameterized on already-resolved paths instead of an
/// `AppHandle` — this is what lets the real-tesseract test below exercise it
/// directly against the dev `resources/ocr/` tree without needing a mocked
/// Tauri app. `ocr_png` is a thin `AppHandle`-resolving wrapper around this.
///
/// Writes the PNG to a unique scratch temp, runs `tesseract <tmp> stdout -l
/// eng --tessdata-dir <dir>`, returns stdout. Temp is removed on every path
/// (success, error, or panic) via the `TempFile` RAII guard.
fn ocr_png_with_paths(bin: &Path, tessdata: &Path, png_bytes: &[u8]) -> Result<String, OcrError> {
    if !bin.is_file() {
        return Err(OcrError::Unavailable);
    }
    // Unique temp under the OS temp dir; process id + a monotonic, strictly-
    // increasing per-process counter keep concurrent/rapid-fire calls from
    // colliding on the same filename.
    let seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!(
        "cleophis-ocr-{}-{}.png",
        std::process::id(),
        seq
    ));
    let _guard = TempFile(tmp.clone());
    std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(png_bytes))
        .map_err(|e| OcrError::Failed(format!("write temp: {e}")))?;

    let mut child = Command::new(bin)
        .arg(&tmp)
        .arg("stdout")
        .arg("-l").arg("eng")
        .arg("--tessdata-dir").arg(tessdata)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| OcrError::Failed(format!("spawn tesseract: {e}")))?;

    // Bounded wait: poll try_wait, kill on timeout.
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(OcrError::Failed(format!("tesseract exit {status}")));
                }
                break;
            }
            Ok(None) => {
                if start.elapsed() > PER_PAGE_TIMEOUT {
                    let _ = child.kill();
                    return Err(OcrError::Failed("timed out".into()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(OcrError::Failed(format!("wait: {e}"))),
        }
    }
    let out = child.wait_with_output().map_err(|e| OcrError::Failed(format!("output: {e}")))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// RAII cleanup for the scratch PNG — removed even on an early return/panic.
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_error_display_is_plain_language() {
        assert!(OcrError::Unavailable.to_string().to_lowercase().contains("ocr"));
        assert!(OcrError::Failed("boom".into()).to_string().contains("boom"));
    }

    #[test]
    fn tesseract_binary_name_is_os_correct() {
        // Windows ships tesseract.exe; other OSes a bare `tesseract`.
        let name = tesseract_binary_name();
        #[cfg(windows)]
        assert_eq!(name, "tesseract.exe");
        #[cfg(not(windows))]
        assert_eq!(name, "tesseract");
    }

    /// Real-tesseract smoke test — exercises `ocr_png_with_paths` directly
    /// against the dev `resources/ocr/` tree, no `AppHandle`/mock needed
    /// (mirrors `kpack-pdf`'s `pdf_smoke.rs` real-artifact `#[ignore]`
    /// pattern). NOT runnable yet: the bundled tesseract binary ships in a
    /// later task (`node tools/fetch-tesseract.mjs`), so this is deferred —
    /// see the OCR task report.
    #[test]
    #[ignore] // needs the bundled tesseract (`node tools/fetch-tesseract.mjs`) + fixture
    fn ocr_png_reads_printed_english() {
        let ocr_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join(OCR_RELATIVE_DIR);
        let bin = ocr_dir.join(tesseract_binary_name());
        let tessdata = ocr_dir.join("tessdata");

        let png = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("scanned-eng.png"),
        )
        .expect("fixture missing: tests/fixtures/scanned-eng.png");

        let text = ocr_png_with_paths(&bin, &tessdata, &png).expect("ocr_png_with_paths failed");
        assert!(
            text.to_lowercase().contains("alpha"),
            "expected recognized text to contain \"alpha\", got: {text:?}"
        );
    }
}
