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

/// Strip a Windows `\\?\` **verbatim** (extended-length) path prefix.
///
/// Tauri's `resource_dir()` returns verbatim paths (`\\?\C:\Program
/// Files\...`). Tesseract builds its data-file path by appending
/// `/eng.traineddata` — with a FORWARD slash — to whatever `--tessdata-dir`
/// receives, and Windows performs **no normalization** on `\\?\` paths, so a
/// forward slash in one is a literal filename character, not a separator:
/// opening `\\?\C:\...\tessdata/eng.traineddata` fails with "Error opening
/// data file" → `Failed loading language 'eng'` → every scanned page OCRs to
/// nothing → the whole PDF is refused ("OCR couldn't read any text"). This was
/// the shipped root cause; it only ever bit the packaged app because only
/// `resource_dir()` yields verbatim paths — every dev/test path is plain, and
/// a plain `C:\...` path *does* tolerate the forward slash. De-verbatim'ing the
/// path before handing it to tesseract is the fix. No-op on plain paths and on
/// non-Windows.
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share` → `\\server\share`
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p.to_path_buf()
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

/// True iff OCR is actually runnable on this install — the app gates the OCR
/// path on this and falls back to the scanned-PDF refusal when false.
pub fn ocr_available(app: &AppHandle) -> bool {
    ocr_available_paths(&bundled_tesseract_path(app), &bundled_tessdata_dir(app))
}

/// The availability decision on already-resolved paths (AppHandle-free, so it's
/// unit-testable). BOTH the binary AND its `eng.traineddata` must be present —
/// checking the language data too, not just the binary, is deliberate: a
/// partial fetch/bundle that lands `tesseract.exe` but not
/// `tessdata/eng.traineddata` would otherwise pass this gate, then fail EVERY
/// page with "Failed loading language 'eng'" and refuse the whole PDF with the
/// generic "OCR couldn't read any text". The honest answer in that case is "OCR
/// isn't available on this install" (the up-front scanned-PDF refusal), which
/// this second check delivers.
fn ocr_available_paths(bin: &Path, tessdata_dir: &Path) -> bool {
    bin.is_file() && tessdata_dir.join("eng.traineddata").is_file()
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
/// Writes the PNG to a unique scratch temp, runs `tesseract <tmp.png>
/// <out_base> -l eng --tessdata-dir <dir>` (tesseract writes `<out_base>.txt`),
/// and returns that file's contents. Both temps are removed on every path
/// (success, error, or panic) via `TempFile` RAII guards.
///
/// Output goes to a FILE, not tesseract's `stdout` pseudo-target, on purpose:
/// draining stdout would mean reading the pipe concurrently with the poll
/// loop, and NOT draining it (as a naive `try_wait` loop does) deadlocks a
/// verbose page against the OS pipe buffer — a fast-but-wordy scan (esp. a
/// noisy image tesseract hallucinates lots of garbage text from) would block
/// in `write()` and masquerade as a genuine `PER_PAGE_TIMEOUT` hang. A file
/// sidesteps pipe capacity entirely.
///
/// `pub(crate)` (not private) so `kpack`'s real-dependency `#[ignore]`d
/// tests can build an `ocr_page` closure over it directly, exactly like
/// this module's own `ocr_png_reads_printed_english` test does — those
/// tests have no `AppHandle` either (see `build_personal_pack_with_embedder`'s
/// doc comment for why it's `AppHandle`-free), so they resolve the bundled
/// tesseract/tessdata paths the same manual way, from `CARGO_MANIFEST_DIR`.
pub(crate) fn ocr_png_with_paths(bin: &Path, tessdata: &Path, png_bytes: &[u8]) -> Result<String, OcrError> {
    // De-verbatim BOTH paths before they reach tesseract — see `strip_verbatim`
    // for the full root-cause writeup. The binary path spawns fine either way;
    // it is the `--tessdata-dir` value that breaks, but normalizing both keeps
    // the whole invocation on plain paths.
    let bin_owned = strip_verbatim(bin);
    let tessdata_owned = strip_verbatim(tessdata);
    let bin: &Path = &bin_owned;
    let tessdata: &Path = &tessdata_owned;
    if !bin.is_file() {
        return Err(OcrError::Unavailable);
    }
    // Unique stem under the OS temp dir; process id + a monotonic, strictly-
    // increasing per-process counter keep concurrent/rapid-fire calls from
    // colliding on the same filename.
    let seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let stem = format!("cleophis-ocr-{}-{}", std::process::id(), seq);
    let tmp_png = std::env::temp_dir().join(format!("{stem}.png"));
    let out_base = std::env::temp_dir().join(&stem); // tesseract appends ".txt"
    let out_txt = std::env::temp_dir().join(format!("{stem}.txt"));
    let err_txt = std::env::temp_dir().join(format!("{stem}.err"));
    // Guards constructed BEFORE the write so both temps are cleaned up even if
    // the PNG write or the spawn fails.
    let _png_guard = TempFile(tmp_png.clone());
    let _txt_guard = TempFile(out_txt.clone());
    let _err_guard = TempFile(err_txt.clone());
    std::fs::File::create(&tmp_png)
        .and_then(|mut f| f.write_all(png_bytes))
        .map_err(|e| OcrError::Failed(format!("write temp: {e}")))?;

    // stderr → a FILE (not a pipe: a pipe could deadlock a verbose page; see
    // this fn's doc comment). Captured so a non-zero exit reports tesseract's
    // ACTUAL reason ("Failed loading language 'eng'", etc.) instead of a bare
    // exit code — the difference between a diagnosable failure and a black box.
    let err_file = std::fs::File::create(&err_txt)
        .map_err(|e| OcrError::Failed(format!("create stderr temp: {e}")))?;
    let mut cmd = Command::new(bin);
    cmd.arg(&tmp_png)
        .arg(&out_base)
        .arg("-l").arg("eng")
        .arg("--tessdata-dir").arg(tessdata)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(err_file));
    // No console flash: tesseract is a console app spawned once PER PAGE from a
    // GUI-subsystem process; without CREATE_NO_WINDOW each page pops (and
    // closes) a console window — a strobe of windows across a multi-page scan.
    // Mirrors `inference::spawn_server`'s handling of llama-server.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| OcrError::Failed(format!("spawn tesseract: {e}")))?;

    // Bounded wait: poll try_wait; on timeout, kill AND reap. `kill()` alone
    // leaves a zombie until app exit — `Child`'s Drop does not `wait()`.
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let why = std::fs::read_to_string(&err_txt)
                        .unwrap_or_default()
                        .replace('\n', " · ");
                    return Err(OcrError::Failed(format!("tesseract exit {status}: {}", why.trim())));
                }
                break;
            }
            Ok(None) => {
                if start.elapsed() > PER_PAGE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait(); // reap the killed child — no zombie
                    return Err(OcrError::Failed("timed out".into()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(OcrError::Failed(format!("wait: {e}"))),
        }
    }
    // Lossy (not read_to_string): a noisy page whose OCR text has a few
    // invalid bytes should still contribute its readable text (degrade per
    // page, never hard-fail) — a genuine read error (missing file) still fails.
    std::fs::read(&out_txt)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| OcrError::Failed(format!("read ocr output: {e}")))
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
    fn ocr_available_requires_both_binary_and_traineddata() {
        // Unique scratch tree so the check sees exactly what we place.
        let seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("cleophis-ocr-avail-{}-{seq}", std::process::id()));
        let ocr = root.join("ocr");
        let tessdata = ocr.join("tessdata");
        std::fs::create_dir_all(&tessdata).unwrap();
        let bin = ocr.join(tesseract_binary_name());
        let eng = tessdata.join("eng.traineddata");

        // Neither present.
        assert!(!ocr_available_paths(&bin, &tessdata), "nothing present");
        // Binary only — the exact partial-bundle that used to pass the old gate
        // and then fail every page with "Failed loading language 'eng'".
        std::fs::write(&bin, b"x").unwrap();
        assert!(!ocr_available_paths(&bin, &tessdata), "binary without traineddata must be unavailable");
        // traineddata only.
        std::fs::remove_file(&bin).unwrap();
        std::fs::write(&eng, b"x").unwrap();
        assert!(!ocr_available_paths(&bin, &tessdata), "traineddata without binary must be unavailable");
        // Both present -> available.
        std::fs::write(&bin, b"x").unwrap();
        assert!(ocr_available_paths(&bin, &tessdata), "both present -> available");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn strip_verbatim_de_prefixes_windows_extended_paths() {
        // The exact shape Tauri's resource_dir() produced in the shipped app —
        // the `\\?\` prefix that made tesseract fail to open eng.traineddata.
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\Program Files\Cleophis\resources\ocr\tessdata")),
            PathBuf::from(r"C:\Program Files\Cleophis\resources\ocr\tessdata")
        );
        // UNC verbatim -> plain UNC.
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\tessdata")),
            PathBuf::from(r"\\server\share\tessdata")
        );
        // Plain paths pass straight through (the common case, and non-Windows).
        assert_eq!(strip_verbatim(Path::new(r"C:\x\tessdata")), PathBuf::from(r"C:\x\tessdata"));
        assert_eq!(strip_verbatim(Path::new("/usr/share/tessdata")), PathBuf::from("/usr/share/tessdata"));
    }

    /// Regression for the shipped "OCR couldn't read any text" on ALL real
    /// scanned PDFs (found via an in-app diagnostic log): `resource_dir()` hands
    /// a `\\?\`-verbatim tessdata dir, and tesseract's `--tessdata-dir` +
    /// `/eng.traineddata` concatenation cannot open a verbatim path
    /// ("Error opening data file … Failed loading language 'eng'"), so every
    /// page OCRs to nothing. `ocr_png_with_paths` must de-verbatim it. Real
    /// bundled tesseract + fixture; Windows-only (verbatim prefixes are a
    /// Windows concept) and `#[ignore]`d like the other real-dependency tests —
    /// run with `cargo test -p cleophis ocr::tests::ocr_png_survives_a_verbatim_tessdata_path -- --ignored`.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn ocr_png_survives_a_verbatim_tessdata_path() {
        let ocr_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join(OCR_RELATIVE_DIR);
        let bin = ocr_dir.join(tesseract_binary_name());
        let tessdata_plain = ocr_dir.join("tessdata");
        // Reconstruct the packaged app's verbatim path exactly.
        let tessdata_verbatim = PathBuf::from(format!(r"\\?\{}", tessdata_plain.display()));
        let png = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("scanned-eng.png"),
        )
        .expect("fixture missing: tests/fixtures/scanned-eng.png");
        let text = ocr_png_with_paths(&bin, &tessdata_verbatim, &png)
            .expect("a verbatim \\\\?\\ tessdata path must still OCR after the fix");
        assert!(
            text.to_lowercase().contains("alpha"),
            "expected \"alpha\" from a verbatim-path OCR, got: {text:?}"
        );
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
    /// pattern). Runnable now (Task 5, D2): the bundled tesseract binary +
    /// `tessdata/eng.traineddata` ship in `resources/ocr/`
    /// (`node tools/fetch-tesseract.mjs`), and the fixture
    /// `tests/fixtures/scanned-eng.png` is a real 300-DPI raster of
    /// `crates/kpack-pdf/tests/fixtures/sample.pdf` page 1 (whose known text
    /// is "...page one alpha"), rendered via `kpack_pdf::render_page` —
    /// see `examples/gen_ocr_fixture.rs`'s history for how it was made.
    /// `#[ignore]`d like the other real-dependency tests in this app (not
    /// run by a plain `cargo test`); run explicitly with `cargo test -p
    /// cleophis ocr::tests::ocr_png_reads_printed_english -- --ignored`.
    #[test]
    #[ignore] // real bundled tesseract + fixture — see doc comment above
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
